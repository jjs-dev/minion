use crate::linux::{
    jail_common::{JobQuery, Query},
    util::{IpcSocketExt, Pid, StraceLogger},
    zygote::{setup, spawn_job, JobOptions, SetupData, Stdio, ZygoteOptions},
    Error,
};
use nix::sys::{
    select::FdSet,
    signal::{SigSet, Signal},
    signalfd::SfdFlags,
    wait::{WaitPidFlag, WaitStatus},
};
use std::{
    io::Write,
    os::unix::io::{AsRawFd, RawFd},
};

pub(crate) struct ReturnCode(i32);

impl ReturnCode {
    const OK: ReturnCode = ReturnCode(0);
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
    notify: Option<RawFd>,
    /// Exit code, if child has finished.
    exit_code: Option<i32>,
}

pub(crate) struct Zygote<'a, 'b> {
    tasks: Vec<Task>,
    options: &'a mut ZygoteOptions<'b>,
    setup_data: &'a SetupData,
}
impl Zygote<'_, '_> {
    fn process_spawn_query(&mut self, options: &JobQuery) -> Result<(), Error> {
        let mut logger = StraceLogger::new();
        writeln!(logger, "got Spawn request").ok();
        // Now we do some preprocessing.
        let env: Vec<_> = options.environment.clone();

        let mut child_fds = self
            .options
            .sock
            .recv_struct::<u64, [RawFd; 3]>()
            .unwrap()
            .1
            .unwrap();
        for f in child_fds.iter_mut() {
            let old = *f;
            let new = nix::unistd::dup(old).unwrap();
            nix::unistd::close(old).unwrap();
            *f = new;
        }
        let child_stdio = Stdio::from_fd_array(child_fds);

        let job_options = JobOptions {
            exe: options.image_path.clone(),
            argv: options.argv.clone(),
            env,
            stdio: child_stdio,
            pwd: options.pwd.clone().into_os_string(),
        };

        writeln!(logger, "JobOptions are fetched").ok();
        let startup_info = spawn_job(
            job_options,
            self.setup_data,
            self.options.jail_options.jail_id.clone(),
        )?;
        writeln!(logger, "Job started, storing Task.").ok();
        let (notify, event) = if crate::linux::check::pidfd_supported() {
            (
                None,
                crate::linux::util::pidfd_open(startup_info.pid).unwrap(),
            )
        } else {
            let (pipe_r, pipe_w) = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC).unwrap();

            (Some(pipe_w), pipe_r)
        };

        self.tasks.push(Task {
            exit_code: None,
            pid: startup_info.pid,
            notify,
        });
        writeln!(logger, "Sending startup_info back.").ok();
        self.options.sock.send(&startup_info)?;
        self.options.sock.send_slice(b"0", Some(&[event])).unwrap();

        Ok(())
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
            self.options.sock.send(&code)?;
            return Ok(());
        }
        let wait_status = nix::sys::wait::waitpid(
            Some(nix::unistd::Pid::from_raw(pid)),
            Some(nix::sys::wait::WaitPidFlag::WNOHANG),
        )?;
        match wait_status {
            WaitStatus::Exited(_, exit_code) => self.options.sock.send(&exit_code)?,
            other => unreachable!("unexpected WaitStatus: {:?}", other),
        };
        Ok(())
    }

    fn reap_child(&mut self) -> Result<bool, Error> {
        let wait_status = nix::sys::wait::waitpid(None, Some(WaitPidFlag::WNOHANG))?;

        match wait_status {
            WaitStatus::Exited(pid, exit_code) => {
                self.tasks
                    .iter_mut()
                    .filter(|task| task.pid == pid.as_raw())
                    .for_each(|task| {
                        let prev = task.exit_code.replace(exit_code);
                        assert!(prev.is_none());
                        if let Some(notify) = task.notify {
                            nix::unistd::write(notify, b"J").unwrap();
                        }
                    });
                Ok(true)
            }
            WaitStatus::StillAlive => Ok(false),
            other => unreachable!("unexpected wait status: {:?}", other),
        }
    }

    fn reap_children(&mut self) -> Result<(), Error> {
        while self.reap_child()? {}
        Ok(())
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
            Query::Spawn(ref opts) => self.process_spawn_query(opts)?,
            Query::Exit => return Ok(Some(ReturnCode::OK)),
            Query::GetExitCode(query) => self.process_get_exit_code_query(query.pid)?,
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
        let sock_fd = self.options.sock.as_raw_fd();
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
    let setup_data = setup::setup(
        &options.jail_options,
        &mut options.sock,
        options.cgroup_driver,
    )?;
    let mut zygote = Zygote {
        options: &mut options,
        tasks: Vec::new(),
        setup_data: &setup_data,
    };
    if crate::linux::check::pidfd_supported() {
        zygote.run_loop_pidfd()
    } else {
        zygote.run_loop_legacy()
    }
}
