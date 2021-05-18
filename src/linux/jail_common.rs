use crate::{
    linux::{ipc::Socket, seccomp::Seccomp, util::Pid},
    SharedItemKind,
};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, path::PathBuf, time::Duration};

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(in crate::linux) struct JailOptions {
    pub(crate) max_alive_process_count: u32,
    pub(crate) memory_limit: u64,
    /// Specifies total CPU time for whole sandbox.
    pub(crate) cpu_time_limit: Duration,
    /// Specifies wall-closk time limit for whole sandbox.
    /// Possible value: time_limit * 3.
    pub(crate) real_time_limit: Duration,
    pub(crate) isolation_root: PathBuf,
    pub(crate) shared_items: Vec<LinuxSharedItem>,
    pub(crate) jail_id: String,
    pub(crate) allow_mount_ns_failure: bool,
    pub(crate) sandbox_uid: Option<u32>,
    pub(crate) enable_watchdog: bool,
    pub(crate) seccomp: Seccomp,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct LinuxSharedItem {
    pub(crate) src: PathBuf,
    pub(crate) dest: PathBuf,
    pub(crate) kind: SharedItemKind,
    pub(crate) flags: SharedItemFlags,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct SharedItemFlags {
    pub(crate) recursive: bool,
}

const ID_CHARS: &[u8] = b"qwertyuiopasdfghjklzxcvbnm1234567890";
const ID_SIZE: usize = 8;

pub(crate) fn gen_jail_id() -> String {
    let mut gen = rand::thread_rng();
    let mut out = Vec::new();
    for _i in 0..ID_SIZE {
        let ch = *(ID_CHARS.choose(&mut gen).unwrap());
        out.push(ch);
    }
    String::from_utf8_lossy(&out[..]).to_string()
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct JobQuery {
    pub(crate) image_path: PathBuf,
    pub(crate) argv: Vec<OsString>,
    pub(crate) environment: Vec<OsString>,
    pub(crate) pwd: PathBuf,
    /// These are just "names" of fds, actual ownership
    /// is passed in subsequent query (right after stdio)
    pub(crate) extra_fds: Vec<i32>,
}

/// Asks zygote for exit code of **completed** task.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct GetExitCodeQuery {
    pub(crate) pid: Pid,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ResourceUsageInformation {
    pub(crate) memory: u64,
    pub(crate) cpu: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct JobStartupInfo {
    pub(crate) pid: Pid,
}

pub(crate) struct ZygoteStartupInfo {
    pub(crate) socket: Socket,
    pub(crate) zygote_pid: Pid,
}

#[derive(Serialize, Deserialize, Debug)]
#[repr(C)]
pub(crate) enum Query {
    Spawn(JobQuery),
    GetExitCode(GetExitCodeQuery),
    GetResourceUsage,
}

#[derive(Debug)]
pub(in crate::linux) struct ZygoteInfo {
    pub(in crate::linux) sock: Socket,
    pub(in crate::linux) pid: Pid,
}

impl Drop for ZygoteInfo {
    fn drop(&mut self) {
        // We will kill zygote, and
        // kernel will kill all other processes by itself.
        send_term_signals(self.pid);
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
