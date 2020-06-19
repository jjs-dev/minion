use crate::{
    linux::{
        jail_common,
        pipe::setup_pipe,
        util::{ExitCode, Handle, IpcSocketExt, Pid},
        zygote,
    },
    Sandbox, SandboxOptions,
};
use std::{
    fmt::{self, Debug},
    os::unix::io::AsRawFd,
    sync::{
        atomic::{AtomicBool, Ordering::SeqCst},
        Arc, Mutex,
    },
    time::Duration,
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
    fn process_flag(&self, ch: u8) -> crate::Result<()> {
        match ch {
            b'c' => {
                self.was_cpu_tle.store(true, SeqCst);
            }
            b'r' => {
                self.was_wall_tle.store(true, SeqCst);
            }
            _ => return Err(crate::Error::Sandbox),
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
    watchdog_chan: Handle,
}

#[derive(Debug)]
struct LinuxSandboxDebugHelper<'a> {
    id: &'a str,
    options: &'a SandboxOptions,
    zygote_sock: Handle,
    zygote_pid: Pid,
    state: SandboxState,
    watchdog_chan: Handle,
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
    fn id(&self) -> String {
        self.0.id.clone()
    }

    fn check_cpu_tle(&self) -> crate::Result<bool> {
        self.poll_state()?;
        Ok(self.0.state.was_cpu_tle.load(SeqCst))
    }

    fn check_real_tle(&self) -> crate::Result<bool> {
        self.poll_state()?;
        Ok(self.0.state.was_wall_tle.load(SeqCst))
    }

    fn kill(&self) -> crate::Result<()> {
        jail_common::sandbox_kill_all(self.0.zygote_pid, Some(&self.0.id))
            .map_err(|err| crate::Error::Io { source: err })?;
        Ok(())
    }

    fn resource_usage(&self) -> crate::Result<crate::ResourceUsageData> {
        let cpu_usage = zygote::cgroup::get_cpu_usage(&self.0.id);
        let memory_usage = zygote::cgroup::get_memory_usage(&self.0.id);
        Ok(crate::ResourceUsageData {
            memory: memory_usage,
            time: Some(cpu_usage),
        })
    }
}

pub(crate) struct ExtendedJobQuery {
    pub(crate) job_query: jail_common::JobQuery,
    pub(crate) stdin: Handle,
    pub(crate) stdout: Handle,
    pub(crate) stderr: Handle,
}

impl LinuxSandbox {
    fn poll_state(&self) -> crate::Result<()> {
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

    pub(crate) unsafe fn create(options: SandboxOptions) -> crate::Result<LinuxSandbox> {
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
        };
        let startup_info = zygote::start_zygote(jail_options)?;

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
        };

        Ok(LinuxSandbox(Arc::new(inner)))
    }

    pub(crate) unsafe fn spawn_job(
        &self,
        query: ExtendedJobQuery,
    ) -> Option<jail_common::JobStartupInfo> {
        let q = jail_common::Query::Spawn(query.job_query.clone());

        let mut sock = self.0.zygote_sock.lock().unwrap();

        // note that we ignore errors, because zygote can be already killed for some reason
        sock.send(&q).ok();

        let fds = [query.stdin, query.stdout, query.stderr];
        let empty: u64 = 0xDEAD_F00D_B17B_00B5;
        sock.send_struct(&empty, Some(&fds)).ok();
        sock.recv().ok()
    }

    pub(crate) unsafe fn poll_job(&self, pid: Pid, timeout: Option<Duration>) -> Option<ExitCode> {
        let q = jail_common::Query::Poll(jail_common::PollQuery { pid, timeout });
        let mut sock = self.0.zygote_sock.lock().unwrap();
        sock.send(&q).ok();
        match sock.recv::<Option<i32>>() {
            Ok(x) => x.map(Into::into),
            Err(_) => Some(crate::EXIT_CODE_KILLED),
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
            zygote::cgroup::drop(&self.0.id, &["pids", "memory", "cpuacct"]);
        }

        // Close handles
        nix::unistd::close(self.0.watchdog_chan).ok();
    }
}
