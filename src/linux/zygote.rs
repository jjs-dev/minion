//! this module implements a Zygote.
//! Jygote is a long-running process in sandbox.
//! In particular, zygote is namespace root.
//! Zygote accepts queries for spawning child process

mod main_loop;
mod setup;

use crate::linux::{
    jail_common::{self, JailOptions},
    util::{duplicate_string, err_exit, IpcSocketExt, Pid, Uid},
    Error,
};
use libc::c_char;
use std::{
    ffi::{CString, OsStr, OsString},
    fs,
    io::Write,
    mem,
    os::unix::{ffi::OsStrExt, io::RawFd},
    path::PathBuf,
    ptr,
};
use tiny_nix_ipc::Socket;

use jail_common::ZygoteStartupInfo;
use setup::SetupData;

const SANDBOX_INTERNAL_UID: Uid = 179;

struct Stdio {
    stdin: RawFd,
    stdout: RawFd,
    stderr: RawFd,
}

impl Stdio {
    fn from_fd_array(fds: [RawFd; 3]) -> Stdio {
        Stdio {
            stdin: fds[0],
            stdout: fds[1],
            stderr: fds[2],
        }
    }

    fn close_fds(self) {
        nix::unistd::close(self.stdin).ok();
        nix::unistd::close(self.stdout).ok();
        nix::unistd::close(self.stderr).ok();
    }
}

struct JobOptions {
    exe: PathBuf,
    argv: Vec<OsString>,
    env: Vec<OsString>,
    stdio: Stdio,
    pwd: OsString,
}

pub(crate) struct ZygoteOptions<'a> {
    jail_options: JailOptions,
    sock: Socket,
    cgroup_driver: &'a crate::linux::cgroup::Driver,
}

struct DoExecArg<'a> {
    path: &'a OsStr,
    arguments: &'a [OsString],
    environment: &'a [OsString],
    stdio: Stdio,
    sock: Socket,
    pwd: &'a OsStr,
    join_handle: &'a crate::linux::cgroup::JoinHandle,
    jail_id: &'a str,
}

fn duplicate_string_list(v: &[OsString]) -> *mut *mut c_char {
    let n = v.len();
    let mut res = Vec::with_capacity(n + 1);
    for str in v {
        let str = duplicate_string(&str);
        res.push(str);
    }
    res.push(ptr::null_mut());
    let ret = res.as_mut_ptr();
    mem::forget(res);
    ret
}

const WAIT_MESSAGE_CLASS_EXECVE_PERMITTED: &[u8] = b"EXECVE";

// This function is called, when execve() returned ENOENT, to provide additional information on best effort basis.
fn print_diagnostics(path: &OsStr, out: &mut dyn Write) {
    let mut path = std::path::PathBuf::from(path);
    let existing_prefix;
    loop {
        let metadata = fs::metadata(&path);
        if let Ok(m) = metadata {
            if m.is_dir() {
                existing_prefix = path;
                break;
            }
        }
        path.pop();
    }
    writeln!(
        out,
        "following path exists: {:?}, with the following items:",
        &existing_prefix
    )
    .ok();
    let items = fs::read_dir(existing_prefix);
    let items = match items {
        Ok(it) => it,
        Err(e) => {
            writeln!(out, "couldn't list path: {:?}", e).ok();
            return;
        }
    };
    for item in items {
        write!(out, "\t- ").ok();
        match item {
            Ok(item) => {
                writeln!(out, "{}", item.file_name().to_string_lossy()).ok();
            }
            Err(err) => {
                writeln!(out, "<error: {:?}>", err).ok();
            }
        }
    }
}

fn do_exec(mut arg: DoExecArg) -> ! {
    use std::os::unix::io::FromRawFd;
    unsafe {
        let stderr_fd = libc::dup(2);
        let mut stderr = std::fs::File::from_raw_fd(stderr_fd);
        let path = duplicate_string(&arg.path);

        let mut argv_with_path = vec![arg.path.to_os_string()];
        argv_with_path.extend(arg.arguments.iter().cloned());

        // Duplicate argv.
        let argv = duplicate_string_list(&argv_with_path);

        // Duplicate envp.
        let environ = arg.environment;
        let envp = duplicate_string_list(&environ);

        // Join cgroups.
        // This doesn't require any additional capablities, because we just write some stuff
        // to preopened handle.
        arg.join_handle.join_self();

        // Now we need mark all FDs as CLOEXEC for not to expose them to sandboxed process
        let fd_list;
        {
            let fd_list_path = "/proc/self/fd".to_string();
            fd_list = fs::read_dir(fd_list_path).expect("failed to enumerate /proc/self/fd");
        }
        for fd in fd_list {
            let fd = fd.expect("failed to get fd entry metadata");
            let fd = fd.file_name().to_string_lossy().to_string();
            let fd: RawFd = fd
                .parse()
                .expect("/proc/self/fd member file name is not fd");
            if -1 == libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) {
                let fd_info_path = format!("/proc/self/fd/{}", fd);
                let fd_info_path = CString::new(fd_info_path.as_str()).unwrap();
                let mut fd_info = [0; 4096];
                libc::readlink(fd_info_path.as_ptr(), fd_info.as_mut_ptr(), 4096);
                let fd_info = CString::from_raw(fd_info.as_mut_ptr());
                let fd_info = fd_info.to_str().unwrap();
                panic!("couldn't cloexec fd: {}({})", fd, fd_info);
            }
        }
        // Now let's change our working dir to desired.
        let pwd = CString::new(arg.pwd.as_bytes()).unwrap();
        if libc::chdir(pwd.as_ptr()) == -1 {
            let code = nix::errno::errno();
            writeln!(
                stderr,
                "WARNING: couldn't change dir (error {} - {})",
                code,
                nix::errno::from_i32(code).desc()
            )
            .ok();
            // It is not error from security PoV if chdir failed: chroot isolation works even if current dir is outside of chroot.
        }

        if libc::setgid(SANDBOX_INTERNAL_UID as u32) != 0 {
            err_exit("setgid");
        }

        if libc::setuid(SANDBOX_INTERNAL_UID as u32) != 0 {
            err_exit("setuid");
        }
        // Now we pause ourselves until parent process places us into appropriate groups.
        arg.sock.lock(WAIT_MESSAGE_CLASS_EXECVE_PERMITTED).unwrap();

        // Call dup2 as late as possible for all panics to write to normal stdio instead of pipes.
        libc::dup2(arg.stdio.stdin, libc::STDIN_FILENO);
        libc::dup2(arg.stdio.stdout, libc::STDOUT_FILENO);
        libc::dup2(arg.stdio.stderr, libc::STDERR_FILENO);

        let mut logger = crate::linux::util::StraceLogger::new();
        writeln!(logger, "sandbox {}: ready to execve", arg.jail_id).unwrap();

        libc::execvpe(
            path,
            argv as *const *const c_char,
            envp as *const *const c_char,
        );
        // Execve only returns on error.

        let err_code = errno::errno().0;
        if err_code == libc::ENOENT {
            writeln!(
                stderr,
                "FATAL ERROR: executable ({}) was not found",
                &arg.path.to_string_lossy()
            )
            .ok();

            print_diagnostics(&arg.path, &mut stderr);
            libc::exit(108)
        } else {
            writeln!(stderr, "couldn't execute: error code {}", err_code).ok();
            err_exit("execve");
        }
    }
}

fn spawn_job(
    options: JobOptions,
    setup_data: &SetupData,
    jail_id: String,
) -> Result<jail_common::JobStartupInfo, Error> {
    let (mut sock, mut child_sock) = Socket::new_socketpair().unwrap();
    child_sock
        .no_cloexec()
        .expect("Couldn't make child socket inheritable");
    // `dea` will be passed to child process
    let dea = DoExecArg {
        path: options.exe.as_os_str(),
        arguments: &options.argv,
        environment: &options.env,
        stdio: options.stdio,
        sock: child_sock,
        pwd: &options.pwd,
        join_handle: &setup_data.cgroup_join_handle,
        jail_id: &jail_id,
    };
    let res = unsafe { nix::unistd::fork() }?;
    let child_pid = match res {
        nix::unistd::ForkResult::Child => do_exec(dea),
        nix::unistd::ForkResult::Parent { child } => child,
    };
    // Parent
    dea.stdio.close_fds();

    // Now we can allow child to execve()
    sock.wake(WAIT_MESSAGE_CLASS_EXECVE_PERMITTED)?;

    Ok(jail_common::JobStartupInfo {
        pid: child_pid.as_raw(),
    })
}

const WM_CLASS_SETUP_FINISHED: &[u8] = b"WM_SETUP";
const WM_CLASS_PID_MAP_READY_FOR_SETUP: &[u8] = b"WM_SETUP_READY";
const WM_CLASS_PID_MAP_CREATED: &[u8] = b"WM_PIDMAP_DONE";

pub(in crate::linux) fn start_zygote(
    jail_options: JailOptions,
    cgroup_driver: &crate::linux::cgroup::Driver,
) -> Result<jail_common::ZygoteStartupInfo, Error> {
    let (socket, zyg_sock) = Socket::new_socketpair().unwrap();

    let (return_allowed_r, return_allowed_w) = nix::unistd::pipe().expect("couldn't create pipe");

    match unsafe { nix::unistd::fork() }? {
        nix::unistd::ForkResult::Child => {
            let sandbox_uid = nix::unistd::Uid::effective();
            // why we use unshare(PID) here, and not in setup_namespace()? See pid_namespaces(7) and unshare(2)
            let unshare_ns = nix::sched::CloneFlags::CLONE_NEWUSER
                | nix::sched::CloneFlags::CLONE_NEWPID
                | nix::sched::CloneFlags::CLONE_NEWNET;
            nix::sched::unshare(unshare_ns)?;
            nix::sched::unshare(nix::sched::CloneFlags::CLONE_NEWNS).or_else(|err| {
                if jail_options.allow_mount_ns_failure {
                    Ok(())
                } else {
                    Err(err)
                }
            })?;
            match unsafe { nix::unistd::fork() }? {
                nix::unistd::ForkResult::Child => {
                    start_zygote_main_process(jail_options, socket, zyg_sock, cgroup_driver)
                }
                nix::unistd::ForkResult::Parent { child } => start_zygote_initialization_helper(
                    zyg_sock,
                    child.as_raw(),
                    jail_options,
                    socket,
                    return_allowed_w,
                    sandbox_uid.as_raw(),
                )
                .map(|never| match never {}),
            }
        }
        nix::unistd::ForkResult::Parent { .. } => Ok(start_zygote_caller(
            return_allowed_r,
            return_allowed_w,
            jail_options,
            socket,
        )),
    }
}

/// Thread A it is thread that entered start_zygote() normally, returns from function
fn start_zygote_caller(
    return_allowed_r: RawFd,
    return_allowed_w: RawFd,
    jail_options: JailOptions,
    socket: Socket,
) -> ZygoteStartupInfo {
    let mut logger = crate::linux::util::strace_logger();
    write!(logger, "sandbox {}: thread A (main)", &jail_options.jail_id).unwrap();

    let mut zygote_pid_bytes = [0; 4];

    // Wait until zygote is ready.
    // Zygote is ready when zygote launcher returns it's pid
    nix::unistd::read(return_allowed_r, &mut zygote_pid_bytes).expect("protocol violation");
    nix::unistd::close(return_allowed_r).unwrap();
    nix::unistd::close(return_allowed_w).unwrap();
    nix::unistd::close(jail_options.watchdog_chan).unwrap();
    jail_common::ZygoteStartupInfo {
        socket,
        zygote_pid: i32::from_ne_bytes(zygote_pid_bytes),
    }
}

/// Thread B is zygote initialization helper, external to sandbox.
fn start_zygote_initialization_helper(
    zyg_sock: Socket,
    child_pid: Pid,
    jail_options: JailOptions,
    mut socket: Socket,
    return_allowed_w: RawFd,
    sandbox_uid: u32,
) -> Result<std::convert::Infallible, Error> {
    let mut logger = crate::linux::util::strace_logger();
    write!(
        logger,
        "sandbox {}: thread B (zygote launcher)",
        &jail_options.jail_id
    )
    .unwrap();
    mem::drop(zyg_sock);

    // currently our only task is to setup uid/gid mapping.

    // map sandbox uid: internal to external.
    let mapping = format!("{} {} 1", SANDBOX_INTERNAL_UID, sandbox_uid);
    let uid_map_path = format!("/proc/{}/uid_map", child_pid);
    let gid_map_path = format!("/proc/{}/gid_map", child_pid);
    let setgroups_path = format!("/proc/{}/setgroups", child_pid);
    socket.lock(WM_CLASS_PID_MAP_READY_FOR_SETUP)?;
    fs::write(setgroups_path, "deny").unwrap();
    fs::write(&uid_map_path, mapping.as_str()).unwrap();
    fs::write(&gid_map_path, mapping.as_str()).unwrap();
    socket.wake(WM_CLASS_PID_MAP_CREATED)?;
    socket.lock(WM_CLASS_SETUP_FINISHED)?;
    // And now thread A can return.
    nix::unistd::write(return_allowed_w, &child_pid.to_ne_bytes()).expect("protocol violation");
    unsafe {
        libc::exit(0);
    }
}

/// Thread C is zygote main process
fn start_zygote_main_process(
    jail_options: JailOptions,
    socket: Socket,
    zyg_sock: Socket,
    cgroup_driver: &crate::linux::cgroup::Driver,
) -> ! {
    let mut logger = crate::linux::util::strace_logger();
    write!(
        logger,
        "sandbox {}: thread C (zygote main)",
        &jail_options.jail_id
    )
    .unwrap();
    mem::drop(socket);
    let zyg_opts = ZygoteOptions {
        jail_options,
        sock: zyg_sock,
        cgroup_driver,
    };
    let zygote_ret_code = main_loop::entry(zyg_opts);

    unsafe {
        libc::exit(zygote_ret_code.map(main_loop::ReturnCode::get).unwrap_or(1));
    }
}
