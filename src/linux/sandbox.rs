use crate::{
    linux::{
        jail_common,
        pipe::setup_pipe,
        util::{IpcSocketExt, Pid},
        zygote, Error,
    },
    ExitCode, Sandbox, SandboxOptions,
};
use std::{
    fmt::{self, Debug},
    os::unix::io::{AsRawFd, RawFd},
    sync::{
        atomic::{AtomicBool, Ordering::SeqCst},
        Arc, Mutex,
    },
};
use tiny_nix_ipc::Socket;

/// Bits which are reported by time watcher
#[derive(Debug)]
#[repr(C)]
struct SandboxState {
    /// CPU time limit was exceeded
    was_cpu_tle: AtomicBool,
    /// Wall-clock time limit was exceeded
    was_wall_tle: AtomicBool,
}

impl SandboxState {
    fn process_flag(&self, ch: u8) -> Result<(), Error> {
        match ch {
            b'c' => {
                self.was_cpu_tle.store(true, SeqCst);
            }
            b'r' => {
                self.was_wall_tle.store(true, SeqCst);
            }
            _ => return Err(Error::Sandbox),
        }
        Ok(())
    }

    fn snapshot(&self) -> Self {
        SandboxState {
            was_cpu_tle: AtomicBool::new(self.was_cpu_tle.load(SeqCst)),
            was_wall_tle: AtomicBool::new(self.was_wall_tle.load(SeqCst)),
        }
    }
}

#[derive(Clone)]
pub struct LinuxSandbox(Arc<LinuxSandboxInner>);

#[repr(C)]
struct LinuxSandboxInner {
    id: String,
    options: SandboxOptions,
    zygote_sock: Mutex<Socket>,
    zygote_pid: Pid,
    state: SandboxState,
    watchdog_chan: RawFd,
    cgroup_driver: Arc<crate::linux::cgroup::Driver>,
}

#[derive(Debug)]
struct LinuxSandboxDebugHelper<'a> {
    id: &'a str,
    options: &'a SandboxOptions,
    zygote_sock: RawFd,
    zygote_pid: Pid,
    state: SandboxState,
    watchdog_chan: RawFd,
}

impl Debug for LinuxSandbox {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let h = LinuxSandboxDebugHelper {
            id: &self.0.id,
            options: &self.0.options,
            zygote_sock: self.0.zygote_sock.lock().unwrap().as_raw_fd(),
            zygote_pid: self.0.zygote_pid,
            watchdog_chan: self.0.watchdog_chan,
            state: self.0.state.snapshot(),
        };

        h.fmt(f)
    }
}

impl Sandbox for LinuxSandbox {
    type Error = Error;

    fn id(&self) -> String {
        self.0.id.clone()
    }

    fn check_cpu_tle(&self) -> Result<bool, Error> {
        self.poll_state()?;
        Ok(self.0.state.was_cpu_tle.load(SeqCst))
    }

    fn check_real_tle(&self) -> Result<bool, Error> {
        self.poll_state()?;
        Ok(self.0.state.was_wall_tle.load(SeqCst))
    }

    fn kill(&self) -> Result<(), Error> {
        jail_common::kill_sandbox(self.0.zygote_pid, &self.0.id, &self.0.cgroup_driver)
            .map_err(|err| Error::Io { source: err })?;
        Ok(())
    }

    fn resource_usage(&self) -> Result<crate::ResourceUsageData, Error> {
        let cpu_usage = self.0.cgroup_driver.get_cpu_usage(&self.0.id);
        let memory_usage = self.0.cgroup_driver.get_memory_usage(&self.0.id);
        Ok(crate::ResourceUsageData {
            memory: memory_usage,
            time: Some(cpu_usage),
        })
    }
}

pub(crate) struct ExtendedJobQuery {
    pub(crate) job_query: jail_common::JobQuery,
    pub(crate) stdin: RawFd,
    pub(crate) stdout: RawFd,
    pub(crate) stderr: RawFd,
}

impl LinuxSandbox {
    fn poll_state(&self) -> Result<(), Error> {
        for _ in 0..5 {
            let mut buf = [0; 4];
            let num_read = nix::unistd::read(self.0.watchdog_chan, &mut buf).or_else(|err| {
                if let Some(errno) = err.as_errno() {
                    if errno as i32 == libc::EAGAIN {
                        return Ok(0);
                    }
                }
                Err(err)
            })?;
            if num_read == 0 {
                break;
            }
            for ch in &buf[..num_read] {
                self.0.state.process_flag(*ch)?;
            }
        }

        Ok(())
    }

    pub(in crate::linux) unsafe fn create(
        options: SandboxOptions,
        settings: &crate::linux::Settings,
        cgroup_driver: Arc<crate::linux::cgroup::Driver>,
    ) -> Result<LinuxSandbox, Error> {
        let jail_id = jail_common::gen_jail_id();
        let mut read_end = 0;
        let mut write_end = 0;
        setup_pipe(&mut read_end, &mut write_end)?;
        nix::fcntl::fcntl(
            read_end,
            nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK),
        )?;

        let jail_options = jail_common::JailOptions {
            max_alive_process_count: options.max_alive_process_count,
            memory_limit: options.memory_limit,
            cpu_time_limit: options.cpu_time_limit,
            real_time_limit: options.real_time_limit,
            isolation_root: options.isolation_root.clone(),
            exposed_paths: options.exposed_paths.clone(),
            jail_id: jail_id.clone(),
            watchdog_chan: write_end,
            allow_mount_ns_failure: settings.allow_unsupported_mount_namespace,
        };
        let startup_info = zygote::start_zygote(jail_options, &cgroup_driver)?;

        let inner = LinuxSandboxInner {
            id: jail_id,
            options,
            zygote_sock: Mutex::new(startup_info.socket),
            zygote_pid: startup_info.zygote_pid,
            watchdog_chan: read_end,
            state: SandboxState {
                was_cpu_tle: AtomicBool::new(false),
                was_wall_tle: AtomicBool::new(false),
            },
            cgroup_driver,
        };

        Ok(LinuxSandbox(Arc::new(inner)))
    }

    pub(crate) unsafe fn spawn_job(
        &self,
        query: ExtendedJobQuery,
    ) -> Result<(jail_common::JobStartupInfo, RawFd), Error> {
        let q = jail_common::Query::Spawn(query.job_query.clone());

        let mut sock = self.0.zygote_sock.lock().unwrap();

        // note that we ignore errors, because zygote can be already killed for some reason
        sock.send(&q).ok();

        let fds = [query.stdin, query.stdout, query.stderr];
        let empty: u64 = 0xDEAD_F00D_B17B_00B5;
        sock.send_struct(&empty, Some(&fds)).ok();
        let job_startup_info = sock.recv()?;
        let fd = sock
            .recv_into_buf::<[RawFd; 1]>(1)
            .map_err(|_| Error::Sandbox)?
            .2
            .ok_or(Error::Sandbox)?;
        Ok((job_startup_info, fd[0]))
    }

    pub(crate) fn get_exit_code(&self, pid: Pid) -> ExitCode {
        let q = jail_common::Query::GetExitCode(jail_common::GetExitCodeQuery { pid });
        let mut sock = self.0.zygote_sock.lock().unwrap();
        sock.send(&q).ok();
        match sock.recv::<i32>() {
            Ok(ec) => ExitCode(ec.into()),
            Err(_) => crate::ExitCode::KILLED,
        }
    }
}

impl Drop for LinuxSandbox {
    fn drop(&mut self) {
        match Arc::get_mut(&mut self.0) {
            // we are last Sandbox handle, so we can drop it
            Some(_) => (),
            // there are other handles, so we must not do anyhing
            None => return,
        };
        // Kill all processes.
        if let Err(err) = self.kill() {
            panic!("unable to kill sandbox: {}", err);
        }
        // Remove cgroups.
        if std::env::var("MINION_DEBUG_KEEP_CGROUPS").is_err() {
            self.0
                .cgroup_driver
                .drop_cgroup(&self.0.id, &["pids", "memory", "cpuacct"]);
        }

        // Close handles
        nix::unistd::close(self.0.watchdog_chan).ok();
    }
}
