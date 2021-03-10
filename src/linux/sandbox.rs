mod watchdog;

use self::watchdog::{watchdog, Event};
use crate::{
    linux::{
        fd::Fd,
        ipc::Socket,
        jail_common::{self, LinuxSharedItem, SharedItemFlags},
        uid_alloc::UidAllocator,
        util::Pid,
        zygote, Error,
    },
    ExitCode, ResourceUsageData, Sandbox, SandboxOptions, SharedItem,
};
use parking_lot::Mutex;
use std::{
    convert::TryInto,
    fmt::Debug,
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
    fn process_flag(&self, ev: Event) {
        match ev {
            Event::CpuTle => {
                self.was_cpu_tle.store(true, SeqCst);
            }
            Event::RealTle => {
                self.was_wall_tle.store(true, SeqCst);
            }
            Event::Heartbeat => {}
        }
    }
}

#[derive(Debug)]
struct ZygoteInfo {
    sock: Socket,
    pid: Pid,
}

impl Drop for ZygoteInfo {
    fn drop(&mut self) {
        // We will kill zygote, and
        // kernel will kill all other processes by itself.
        send_term_signals(self.pid);
    }
}

#[repr(C)]
#[derive(Debug)]
pub struct LinuxSandbox {
    id: String,
    options: SandboxOptions,
    zygote: Arc<Mutex<Option<ZygoteInfo>>>,
    state: SandboxState,
    watchdog_chan: crossbeam_channel::Receiver<Event>,
    driver: Arc<crate::linux::limits::Driver>,
    dealloc_uid: Option<(Arc<UidAllocator>, u32)>,
}

impl Sandbox for LinuxSandbox {
    type Error = Error;

    fn id(&self) -> String {
        self.id.clone()
    }

    fn check_cpu_tle(&self) -> Result<bool, Error> {
        self.poll_state();
        Ok(self.state.was_cpu_tle.load(SeqCst))
    }

    fn check_real_tle(&self) -> Result<bool, Error> {
        self.poll_state();
        Ok(self.state.was_wall_tle.load(SeqCst))
    }

    fn kill(&self) -> Result<(), Error> {
        self.zygote.lock().take();
        Ok(())
    }

    fn resource_usage(&self) -> Result<ResourceUsageData, Error> {
        let usage = self.driver.resource_usage(&self.id)?;
        Ok(ResourceUsageData {
            time: Some(usage.time),
            memory: usage.memory,
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
    fn poll_state(&self) {
        while let Ok(ev) = self.watchdog_chan.try_recv() {
            self.state.process_flag(ev);
        }
    }

    fn with_zygote<R>(&self, f: impl FnOnce(&mut ZygoteInfo) -> R) -> Option<R> {
        let mut z = self.zygote.lock();
        let z = &mut *z;
        z.as_mut().map(f)
    }

    pub(in crate::linux) fn create(
        options: SandboxOptions,
        settings: &crate::linux::Settings,
        driver: Arc<crate::linux::limits::Driver>,
        uid_alloc: Arc<UidAllocator>,
    ) -> Result<LinuxSandbox, Error> {
        let jail_id = jail_common::gen_jail_id();

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
            allow_mount_ns_failure: settings.allow_unsupported_mount_namespace,
            sandbox_uid,
        };
        let startup_info = zygote::start_zygote(jail_options, &driver)?;

        let (watchdog_tx, watchdog_rx) = crossbeam_channel::unbounded();
        let sandbox = LinuxSandbox {
            id: jail_id.clone(),
            options: options.clone(),
            zygote: Arc::new(Mutex::new(Some(ZygoteInfo {
                sock: startup_info.socket,
                pid: startup_info.zygote_pid,
            }))),
            state: SandboxState {
                was_cpu_tle: AtomicBool::new(false),
                was_wall_tle: AtomicBool::new(false),
            },
            watchdog_chan: watchdog_rx,
            driver: driver.clone(),
            dealloc_uid: uid_alloc.map(|uid_alloc| (uid_alloc, sandbox_uid)),
        };
        tokio::task::spawn(watchdog(
            jail_id,
            options
                .cpu_time_limit
                .as_nanos()
                .try_into()
                .expect("too big cpu time limit"),
            options
                .real_time_limit
                .as_nanos()
                .try_into()
                .expect("too big real time limit"),
            watchdog_tx,
            driver,
            sandbox.zygote.clone(),
        ));

        Ok(sandbox)
    }

    pub(crate) unsafe fn spawn_job(
        &self,
        query: ExtendedJobQuery,
    ) -> Result<(jail_common::JobStartupInfo, Fd), Error> {
        let q = jail_common::Query::Spawn(query.job_query.clone());

        self.with_zygote(|zyg| {
            zyg.sock.send(&q)?;

            let fds = vec![query.stdin, query.stdout, query.stderr]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>();
            zyg.sock.send_fds(&fds)?;
            let job_startup_info = zyg.sock.recv()?;
            let fd = zyg.sock.recv_fds(1)?;
            Ok((job_startup_info, fd.into_iter().next().unwrap()))
        })
        .unwrap_or(Err(Error::SandboxGone))
    }

    pub(crate) fn get_exit_code(&self, pid: Pid) -> ExitCode {
        self.with_zygote(|zyg| {
            let q = jail_common::Query::GetExitCode(jail_common::GetExitCodeQuery { pid });
            zyg.sock.send(&q).ok();
            match zyg.sock.recv::<i64>() {
                Ok(ec) => ExitCode(ec),
                Err(_) => crate::ExitCode::KILLED,
            }
        })
        .unwrap_or(crate::ExitCode::KILLED)
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
            tracing::error!("unable to kill sandbox: {}", err);
        }
        // Remove cgroups.
        if std::env::var("MINION_DEBUG_KEEP_CGROUPS").is_err() {
            if let Err(e) = self.driver.delete_group(&self.id) {
                tracing::error!("failed to delete cgroup: {:#}", e);
            }
        }
    }
}

fn send_term_signals(target_pid: Pid) {
    // TODO: maybe SIGKILL is enough?
    for &sig in &[
        nix::sys::signal::SIGKILL,
        nix::sys::signal::SIGTERM,
        nix::sys::signal::SIGABRT,
    ] {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(target_pid), sig).ok();
    }
}
