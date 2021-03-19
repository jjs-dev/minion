use crate::{
    linux::{
        fd::Fd,
        ipc::Socket,
        jail_common::{self, LinuxSharedItem, SharedItemFlags},
        pipe::{setup_pipe, LinuxReadPipe},
        uid_alloc::UidAllocator,
        util::Pid,
        zygote, Error,
    },
    ExitCode, Sandbox, SandboxOptions, SharedItem,
};
use parking_lot::Mutex;
use std::{
    fmt::{self, Debug},
    io::Read,
    os::unix::io::RawFd,
    sync::{
        atomic::{AtomicBool, Ordering::SeqCst},
        Arc,
    },
};

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
            _ => return Err(Error::SandboxMisbehavior { cause: None }),
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

#[repr(C)]
pub struct LinuxSandbox {
    id: String,
    options: SandboxOptions,
    zygote_sock: Mutex<Socket>,
    zygote_pid: Pid,
    state: SandboxState,
    watchdog_chan: Mutex<LinuxReadPipe>,
    cgroup_driver: Arc<crate::linux::cgroup::Driver>,
    dealloc_uid: Option<(Arc<UidAllocator>, u32)>,
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
            id: &self.id,
            options: &self.options,
            zygote_sock: self.zygote_sock.lock().inner().as_raw(),
            zygote_pid: self.zygote_pid,
            watchdog_chan: self.watchdog_chan.lock().inner().as_raw(),
            state: self.state.snapshot(),
        };

        h.fmt(f)
    }
}

impl Sandbox for LinuxSandbox {
    type Error = Error;

    fn id(&self) -> String {
        self.id.clone()
    }

    fn check_cpu_tle(&self) -> Result<bool, Error> {
        self.poll_state()?;
        Ok(self.state.was_cpu_tle.load(SeqCst))
    }

    fn check_real_tle(&self) -> Result<bool, Error> {
        self.poll_state()?;
        Ok(self.state.was_wall_tle.load(SeqCst))
    }

    fn kill(&self) -> Result<(), Error> {
        jail_common::kill_sandbox(self.zygote_pid, &self.id, &self.cgroup_driver)
            .map_err(|err| Error::Io { cause: err })?;
        Ok(())
    }

    fn resource_usage(&self) -> Result<crate::ResourceUsageData, Error> {
        let cpu_usage = self.cgroup_driver.get_cpu_usage(&self.id)?;
        let memory_usage = self.cgroup_driver.get_memory_usage(&self.id)?;
        Ok(crate::ResourceUsageData {
            memory: memory_usage,
            time: Some(cpu_usage),
        })
    }
}

fn convert_shared_item(item: SharedItem) -> Result<LinuxSharedItem, Error> {
    let mut flags = SharedItemFlags { recursive: false };
    for f in &item.flags {
        match f.as_str() {
            "recursive" => {
                flags.recursive = true;
            }
            _ => return Err(Error::InvalidSharedItemFlag { flag: f.clone() }),
        }
    }

    Ok(LinuxSharedItem {
        src: item.src,
        dest: item.dest,
        kind: item.kind,
        flags,
    })
}

pub(crate) struct ExtendedJobQuery {
    pub(crate) job_query: jail_common::JobQuery,
    pub(crate) stdin: Option<Fd>,
    pub(crate) stdout: Option<Fd>,
    pub(crate) stderr: Option<Fd>,
}

impl LinuxSandbox {
    fn poll_state(&self) -> Result<(), Error> {
        let mut chan = self.watchdog_chan.lock();
        for _ in 0..5 {
            let mut buf = [0; 4];
            let num_read = chan.read(&mut buf).or_else(|err| {
                if let Some(errno) = err.raw_os_error() {
                    if errno == libc::EAGAIN {
                        return Ok(0);
                    }
                }
                Err(err)
            })?;
            if num_read == 0 {
                break;
            }
            for ch in &buf[..num_read] {
                self.state.process_flag(*ch)?;
            }
        }

        Ok(())
    }

    pub(in crate::linux) unsafe fn create(
        options: SandboxOptions,
        settings: &crate::linux::Settings,
        cgroup_driver: Arc<crate::linux::cgroup::Driver>,
        uid_alloc: Arc<UidAllocator>,
    ) -> Result<LinuxSandbox, Error> {
        let jail_id = jail_common::gen_jail_id();
        let (tx, rx) = setup_pipe()?;
        rx.inner()
            .fcntl(nix::fcntl::FcntlArg::F_SETFL(nix::fcntl::OFlag::O_NONBLOCK))?;

        let shared_items = options
            .shared_items
            .iter()
            .cloned()
            .map(convert_shared_item)
            .collect::<Result<Vec<_>, _>>()?;

        let (uid_alloc, sandbox_uid) = if settings.rootless {
            (None, nix::unistd::Uid::effective().as_raw())
        } else {
            (
                Some(uid_alloc.clone()),
                uid_alloc.allocate().ok_or(Error::UidExhausted)?,
            )
        };

        tracing::debug!(
            unique = uid_alloc.is_some(),
            uid = sandbox_uid,
            "Selected sandbox_uid"
        );

        let jail_options = jail_common::JailOptions {
            max_alive_process_count: options.max_alive_process_count,
            memory_limit: options.memory_limit,
            cpu_time_limit: options.cpu_time_limit,
            real_time_limit: options.real_time_limit,
            isolation_root: options.isolation_root.clone(),
            shared_items,
            jail_id: jail_id.clone(),
            watchdog_chan: tx.into_inner().into_raw(),
            allow_mount_ns_failure: settings.allow_unsupported_mount_namespace,
            sandbox_uid,
        };
        let startup_info = zygote::start_zygote(jail_options, &cgroup_driver)?;

        let sandbox = LinuxSandbox {
            id: jail_id,
            options,
            zygote_sock: Mutex::new(startup_info.socket),
            zygote_pid: startup_info.zygote_pid,
            state: SandboxState {
                was_cpu_tle: AtomicBool::new(false),
                was_wall_tle: AtomicBool::new(false),
            },
            watchdog_chan: Mutex::new(rx),
            cgroup_driver,
            dealloc_uid: uid_alloc.map(|uid_alloc| (uid_alloc, sandbox_uid)),
        };

        Ok(sandbox)
    }

    pub(crate) unsafe fn spawn_job(
        &self,
        query: ExtendedJobQuery,
    ) -> Result<(jail_common::JobStartupInfo, Fd), Error> {
        let q = jail_common::Query::Spawn(query.job_query.clone());

        let mut sock = self.zygote_sock.lock();

        // note that we ignore errors, because zygote can be already killed for some reason
        sock.send(&q).ok();

        let fds = vec![query.stdin, query.stdout, query.stderr]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        sock.send_fds(&fds)?;
        let job_startup_info = sock.recv()?;
        let fd = sock.recv_fds(1)?;
        Ok((job_startup_info, fd.into_iter().next().unwrap()))
    }

    pub(crate) fn get_exit_code(&self, pid: Pid) -> ExitCode {
        let q = jail_common::Query::GetExitCode(jail_common::GetExitCodeQuery { pid });
        let mut sock = self.zygote_sock.lock();
        sock.send(&q).ok();
        match sock.recv::<i32>() {
            Ok(ec) => ExitCode(ec.into()),
            Err(_) => crate::ExitCode::KILLED,
        }
    }
}

impl Drop for LinuxSandbox {
    #[tracing::instrument(skip(self), fields(id = self.id.as_str()))]
    fn drop(&mut self) {
        // Reclaim UID
        if let Some((uid_alloc, sandbox_uid)) = self.dealloc_uid.take() {
            tracing::debug!(uid = sandbox_uid, "Freeing sandbox_uid");
            uid_alloc.deallocate(sandbox_uid);
        }
        // Kill all processes.
        if let Err(err) = self.kill() {
            panic!("unable to kill sandbox: {}", err);
        }
        // Remove cgroups.
        if std::env::var("MINION_DEBUG_KEEP_CGROUPS").is_err() {
            self.cgroup_driver
                .drop_cgroup(&self.id, &["pids", "memory", "cpuacct"]);
        }
    }
}
