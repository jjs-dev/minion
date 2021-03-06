use crate::linux::{
    fd::Fd,
    jail_common::{JobQuery, Query, ResourceUsageInformation},
    util::{Pid, StraceLogger},
    zygote::{setup, spawn_job, JobOptions, Stdio, ZygoteOptions},
    Error,
};
use nix::sys::{
    select::FdSet,
    signal::{SigSet, Signal},
    signalfd::SfdFlags,
    wait::{WaitPidFlag, WaitStatus},
};
use std::{io::Write, mem::MaybeUninit};

pub(crate) struct ReturnCode(i32);

impl ReturnCode {
    const BAD_QUERY: ReturnCode = ReturnCode(0xBAD);

    pub(crate) fn get(self) -> i32 {
        self.0
    }
}

/// We keep track of all running and not-yet-awaited tasks.
/// We are only interested in task main process.
struct Task {
    /// Main process pid (**local to sandbox pid_ns**).
    pid: Pid,
    /// Writable file descriptor. When task finishes
    /// we write something to this file.
    /// In pidfd mode equals to None.
    notify: Option<Fd>,
    /// Exit code, if child has finished.
    exit_code: Option<i32>,
}

pub(crate) struct Zygote<'a, 'b> {
    tasks: Vec<Task>,
    options: &'a mut ZygoteOptions<'b>,
    resource_group_enter_handle: crate::linux::limits::OpaqueEnterHandle,
}
impl Zygote<'_, '_> {
    fn process_spawn_query(&mut self, options: &JobQuery) {
        let mut logger = StraceLogger::new();
        writeln!(logger, "got Spawn request").ok();
        // Now we do some preprocessing.

        let mut child_fds = self
            .options
            .sock
            .recv_fds(3 + options.extra_fds.len())
            .unwrap();
        let (mut stdio_fds, mut extra_fds) = {
            child_fds.rotate_left(3);
            let c = child_fds.pop().unwrap();
            let b = child_fds.pop().unwrap();
            let a = child_fds.pop().unwrap();
            ([a, b, c], child_fds)
        };
        for f in stdio_fds.iter_mut() {
            *f = f
                .duplicate_with_inheritance()
                .expect("failed to duplicate child stdio fd");
        }
        for f in extra_fds.iter_mut() {
            *f = f
                .duplicate_with_inheritance()
                .expect("failed to duplicate child extra fd");
        }
        let child_stdio = Stdio::from_fd_array(stdio_fds);

        assert_eq!(extra_fds.len(), options.extra_fds.len());

        let job_options = JobOptions {
            exe: options.image_path.clone(),
            argv: options.argv.clone(),
            env: options.environment.clone(),
            stdio: child_stdio,
            pwd: options.pwd.clone().into_os_string(),
            extra: options.extra_fds.iter().copied().zip(extra_fds).collect(),
        };

        writeln!(logger, "JobOptions are fetched").ok();
        let startup_info = spawn_job(
            job_options,
            self.options.jail_options.jail_id.clone(),
            self.options.jail_options.sandbox_uid.is_some(),
            self.resource_group_enter_handle.clone(),
            &self.options.jail_options.seccomp,
        )
        .expect("failed to create child");
        writeln!(logger, "Job started, storing Task.").ok();
        let (notify, event) = if crate::linux::check::pidfd_supported() {
            (
                None,
                Fd::new(
                    crate::linux::util::pidfd_open(startup_info.pid).expect("failed to open pidfd"),
                ),
            )
        } else {
            let (pipe_r, pipe_w) =
                nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).expect("failed to create a pipe");

            (Some(Fd::new(pipe_w)), Fd::new(pipe_r))
        };

        self.tasks.push(Task {
            exit_code: None,
            pid: startup_info.pid,
            notify,
        });
        writeln!(logger, "Sending startup_info back.").ok();
        self.options
            .sock
            .send(&startup_info)
            .expect("failed to send startup info");
        self.options
            .sock
            .send_fds(&[event])
            .expect("failed to send notification fd");
    }

    fn process_get_exit_code_query(&mut self, pid: Pid) -> Result<(), Error> {
        let task = {
            let pos = self
                .tasks
                .iter()
                .position(|t| t.pid == pid)
                .expect("unknown pid");
            self.tasks.swap_remove(pos)
        };
        if let Some(code) = task.exit_code {
            self.options.sock.send::<i64>(&(code as i64))?;
            return Ok(());
        }
        let wait_status = nix::sys::wait::waitpid(
            Some(nix::unistd::Pid::from_raw(pid)),
            Some(WaitPidFlag::WNOHANG),
        )?;
        match wait_status {
            WaitStatus::Exited(_, exit_code) => {
                self.options.sock.send::<i64>(&(exit_code as i64))?
            }
            WaitStatus::Signaled(_, signal, _coredump) => self
                .options
                .sock
                .send::<i64>(&(signal as i64 + crate::ExitCode::SIGNALLED.0))?,
            other => unreachable!("unexpected WaitStatus: {:?}", other),
        };
        Ok(())
    }

    fn process_exited_child(&mut self, pid: nix::unistd::Pid, exit_code: i32) {
        self.tasks
            .iter_mut()
            .filter(|task| task.pid == pid.as_raw())
            .for_each(|task| {
                let prev = task.exit_code.replace(exit_code);
                assert!(prev.is_none());
                if let Some(notify) = task.notify.as_mut() {
                    notify.write(b"J").expect("failed to send notification");
                }
            });
    }

    fn reap_child(&mut self) -> Result<bool, Error> {
        let wait_status = nix::sys::wait::waitpid(None, Some(WaitPidFlag::WNOHANG))?;

        match wait_status {
            WaitStatus::Exited(pid, exit_code) => {
                self.process_exited_child(pid, exit_code);
                Ok(true)
            }
            WaitStatus::StillAlive => Ok(false),
            WaitStatus::Signaled(pid, signal, _coredump) => {
                let exit_code = crate::ExitCode::SIGNALLED.0 as i32 + signal as i32;
                self.process_exited_child(pid, exit_code);
                Ok(true)
            }
            other => unreachable!("unexpected wait status: {:?}", other),
        }
    }

    fn reap_children(&mut self) -> Result<(), Error> {
        while self.reap_child()? {}
        Ok(())
    }

    fn process_resource_usage_query(&mut self) -> Result<(), Error> {
        unsafe {
            let mut usage = MaybeUninit::uninit();
            if libc::getrusage(libc::RUSAGE_CHILDREN, usage.as_mut_ptr()) == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            let usage = usage.assume_init();

            let resp = ResourceUsageInformation {
                // NOT total usage, but max usage
                memory: usage.ru_maxrss as u64,
                cpu: parse_timeval(usage.ru_utime) + parse_timeval(usage.ru_stime),
            };
            self.options.sock.send(&resp)?;
            Ok(())
        }
    }

    fn handle_one_request(&mut self) -> Result<Option<ReturnCode>, Error> {
        let mut logger = StraceLogger::new();
        let query: Query = match self.options.sock.recv() {
            Ok(q) => {
                writeln!(logger, "zygote: new request").ok();
                q
            }
            Err(err) => {
                writeln!(logger, "zygote: got unprocessable query: {}", err).ok();
                return Ok(Some(ReturnCode::BAD_QUERY));
            }
        };
        match query {
            Query::Spawn(ref opts) => self.process_spawn_query(opts),
            Query::GetExitCode(query) => self.process_get_exit_code_query(query.pid)?,
            Query::GetResourceUsage => self.process_resource_usage_query()?,
        };
        Ok(None)
    }

    fn run_loop_pidfd(&mut self) -> Result<ReturnCode, Error> {
        loop {
            if let Some(ret) = self.handle_one_request()? {
                break Ok(ret);
            }
        }
    }

    fn run_loop_legacy(&mut self) -> Result<ReturnCode, Error> {
        let mut sigset = SigSet::empty();
        sigset.add(Signal::SIGCHLD);
        let sig_fd = nix::sys::signalfd::signalfd(-1, &sigset, SfdFlags::SFD_CLOEXEC)?;
        let sock_fd = self.options.sock.inner().as_raw();
        loop {
            let mut fdset_read = FdSet::new();
            if crate::linux::check::pidfd_supported() {
                fdset_read.insert(sig_fd);
            }
            fdset_read.insert(sock_fd);

            nix::sys::select::select(None, &mut fdset_read, None, None, None)?;
            if fdset_read.contains(sig_fd) {
                self.reap_children()?;
            }
            if fdset_read.contains(sock_fd) {
                if let Some(ret) = self.handle_one_request()? {
                    break Ok(ret);
                }
            }
        }
    }
}

pub(crate) fn entry(mut options: ZygoteOptions<'_>) -> Result<ReturnCode, Error> {
    setup::setup(&options.jail_options, &mut options.uid_mapping_done)?;
    let resource_group_enter_handle = options.resource_group_enter_handle.clone();
    let mut zygote = Zygote {
        options: &mut options,
        tasks: Vec::new(),
        resource_group_enter_handle,
    };
    if crate::linux::check::pidfd_supported() {
        zygote.run_loop_pidfd()
    } else {
        zygote.run_loop_legacy()
    }
}

fn parse_timeval(tv: libc::timeval) -> u64 {
    (tv.tv_usec + tv.tv_sec * 1_000_000_000) as u64
}
