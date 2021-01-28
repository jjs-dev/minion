use crate::{linux::util::Pid, SharedDir};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use std::{ffi::OsString, os::unix::io::RawFd, path::PathBuf, time::Duration};
use tiny_nix_ipc::Socket;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub(crate) struct JailOptions {
    pub(crate) max_alive_process_count: u32,
    pub(crate) memory_limit: u64,
    /// Specifies total CPU time for whole sandbox.
    pub(crate) cpu_time_limit: Duration,
    /// Specifies wall-closk time limit for whole sandbox.
    /// Possible value: time_limit * 3.
    pub(crate) real_time_limit: Duration,
    pub(crate) isolation_root: PathBuf,
    pub(crate) exposed_paths: Vec<SharedDir>,
    pub(crate) use_mount_for_binds: bool,
    pub(crate) jail_id: String,
    pub(crate) watchdog_chan: RawFd,
    pub(crate) allow_mount_ns_failure: bool,
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
}

/// Asks zygote for exit code of **completed** task.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub(crate) struct GetExitCodeQuery {
    pub(crate) pid: Pid,
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
    // TODO: is this used?
    Exit,
    Spawn(JobQuery),
    GetExitCode(GetExitCodeQuery),
}

fn send_term_signals(target_pid: Pid) {
    for &sig in &[
        nix::sys::signal::SIGKILL,
        nix::sys::signal::SIGTERM,
        nix::sys::signal::SIGABRT,
    ] {
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(target_pid), sig).ok();
    }
}

/// Kills sandbox where current process is executed
pub(crate) fn kill_this_sandbox() -> ! {
    send_term_signals(1);
    // now let's wait until kernel kills us
    loop {
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// Kills sandbox by zygote pid and cgroup_id
pub(in crate::linux) fn kill_sandbox(
    zygote_pid: Pid,
    cgroup_id: &str,
    cgroup_driver: &crate::linux::cgroup::Driver,
) -> std::io::Result<()> {
    // We will kill zygote, and
    // kernel will kill all other processes by itself.
    send_term_signals(zygote_pid);
    // now let's wait until kill is done
    let pids_tasks_file_path = cgroup_driver.get_cgroup_tasks_file_path(cgroup_id);
    loop {
        let buf = std::fs::read(&pids_tasks_file_path)?;
        let has_some = buf.iter().take(8).any(|c| c.is_ascii_digit());
        if !has_some {
            break;
        }
    }
    Ok(())
}
