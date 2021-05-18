//! this module implements a Zygote.
//! Jygote is a long-running process in sandbox.
//! In particular, zygote is namespace root.
//! Zygote accepts queries for spawning child process

mod main_loop;
mod setup;

use crate::linux::{
    fd::Fd,
    ipc::Socket,
    jail_common::{self, JailOptions, ZygoteStartupInfo},
    seccomp::Seccomp,
    util::{duplicate_string, err_exit, Uid},
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

const SANDBOX_INTERNAL_UID: Uid = 179;

struct Stdio {
    stdin: Fd,
    stdout: Fd,
    stderr: Fd,
}

impl Stdio {
    fn from_fd_array(fds: [Fd; 3]) -> Stdio {
        let mut fds = std::array::IntoIter::new(fds);
        let stdin = fds.next().unwrap();
        let stdout = fds.next().unwrap();
        let stderr = fds.next().unwrap();
        Stdio {
            stdin,
            stdout,
            stderr,
        }
    }
}

struct JobOptions {
    exe: PathBuf,
    argv: Vec<OsString>,
    env: Vec<OsString>,
    stdio: Stdio,
    extra: Vec<(i32, Fd)>,
    pwd: OsString,
}

pub(crate) struct ZygoteOptions<'a> {
    jail_options: JailOptions,
    sock: Socket,
    uid_mapping_done: Fd,
    resource_group_enter_handle: &'a crate::linux::limits::OpaqueEnterHandle,
}

struct DoExecArg<'a> {
    path: &'a OsStr,
    arguments: &'a [OsString],
    environment: &'a [OsString],
    stdio: Stdio,
    extra_fds: &'a [(i32, Fd)],
    pwd: &'a OsStr,
    enter_handle: crate::linux::limits::OpaqueEnterHandle,
    jail_id: &'a str,
    setuid: bool,
    seccomp: &'a Seccomp,
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

fn do_exec(arg: DoExecArg) -> ! {
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
        arg.enter_handle.join();

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

        if arg.setuid {
            if libc::setgid(SANDBOX_INTERNAL_UID as u32) != 0 {
                err_exit("setgid");
            }

            if libc::setuid(SANDBOX_INTERNAL_UID as u32) != 0 {
                err_exit("setuid");
            }
        }

        // Call dup2 as late as possible for all panics to write to normal stdio instead of pipes.
        libc::dup2(arg.stdio.stdin.as_raw(), libc::STDIN_FILENO);
        libc::dup2(arg.stdio.stdout.as_raw(), libc::STDOUT_FILENO);
        libc::dup2(arg.stdio.stderr.as_raw(), libc::STDERR_FILENO);

        for (new_fd, cur_fd) in arg.extra_fds {
            libc::dup2(cur_fd.as_raw(), *new_fd);
        }

        let mut logger = crate::linux::util::StraceLogger::new();
        writeln!(
            logger,
            "sandbox {}: ready to enable seccomp and execve",
            arg.jail_id
        )
        .unwrap();

        arg.seccomp.enable();

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
    jail_id: String,
    setuid: bool,
    resource_group_enter_handle: crate::linux::limits::OpaqueEnterHandle,
    seccomp: &Seccomp,
) -> Result<jail_common::JobStartupInfo, Error> {
    // `dea` will be passed to child process
    let dea = DoExecArg {
        path: options.exe.as_os_str(),
        arguments: &options.argv,
        environment: &options.env,
        stdio: options.stdio,
        extra_fds: &options.extra,
        pwd: &options.pwd,
        enter_handle: resource_group_enter_handle,
        jail_id: &jail_id,
        setuid,
        seccomp: &seccomp,
    };
    let res = unsafe { nix::unistd::fork() }?;
    let child_pid = match res {
        nix::unistd::ForkResult::Child => do_exec(dea),
        nix::unistd::ForkResult::Parent { child } => child,
    };

    Ok(jail_common::JobStartupInfo {
        pid: child_pid.as_raw(),
    })
}

pub(in crate::linux) fn start_zygote(
    jail_options: JailOptions,
    resource_group_enter_handle: &crate::linux::limits::OpaqueEnterHandle,
) -> Result<jail_common::ZygoteStartupInfo, Error> {
    let (client_socket, zyg_sock) = Socket::pair()?;

    let (zygote_pid_r, zygote_pid_w) =
        nix::unistd::pipe().map(|(x, y)| (Fd::new(x), Fd::new(y)))?;
    let (uid_mapping_done_r, uid_mapping_done_w) =
        nix::unistd::pipe().map(|(x, y)| (Fd::new(x), Fd::new(y)))?;

    match unsafe { nix::unistd::fork() }? {
        nix::unistd::ForkResult::Child => {
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
                nix::unistd::ForkResult::Child => start_zygote_main_process(
                    jail_options,
                    uid_mapping_done_r,
                    zyg_sock,
                    resource_group_enter_handle,
                ),
                nix::unistd::ForkResult::Parent { child } => {
                    let len = zygote_pid_w
                        .write(&child.as_raw().to_ne_bytes())
                        .expect("failed to send zygote pid");
                    assert!(len == 4);
                    std::process::exit(0);
                }
            }
        }
        nix::unistd::ForkResult::Parent { .. } => Ok(start_zygote_caller(
            jail_options,
            client_socket,
            zygote_pid_r,
            uid_mapping_done_w,
        )),
    }
}

/// Thread A it is thread that entered start_zygote() normally, returns from function
fn start_zygote_caller(
    jail_options: JailOptions,
    socket: Socket,
    zygote_pid_r: Fd,
    uid_mapping_done: Fd,
) -> ZygoteStartupInfo {
    let mut logger = crate::linux::util::strace_logger();
    write!(logger, "sandbox {}: thread A (main)", &jail_options.jail_id).unwrap();

    let mut zygote_pid = [0; 4];
    {
        let len = zygote_pid_r
            .read(&mut zygote_pid)
            .expect("failed to receive zygote pid");
        assert_eq!(len, 4);
    }
    let zygote_pid = i32::from_ne_bytes(zygote_pid);

    let mapping = match jail_options.sandbox_uid {
        Some(separate_user) => format!(
            "0 {} 1\n{} {} 1",
            nix::unistd::Uid::effective().as_raw(),
            SANDBOX_INTERNAL_UID,
            separate_user
        ),
        None => {
            format!("0 {} 1", nix::unistd::Uid::effective().as_raw())
        }
    };
    let uid_map_path = format!("/proc/{}/uid_map", zygote_pid);
    let gid_map_path = format!("/proc/{}/gid_map", zygote_pid);
    let setgroups_path = format!("/proc/{}/setgroups", zygote_pid);

    fs::write(setgroups_path, "deny").unwrap();

    fs::write(&uid_map_path, mapping.as_str()).unwrap();
    fs::write(&gid_map_path, mapping.as_str()).unwrap();

    uid_mapping_done
        .write(b"D")
        .expect("failed to notify zygote that UIDs are remapped");

    ZygoteStartupInfo { socket, zygote_pid }
}

/// Thread C is zygote main process
fn start_zygote_main_process(
    jail_options: JailOptions,
    uid_mapping_done: Fd,
    zyg_sock: Socket,
    resource_group_enter_handle: &crate::linux::limits::OpaqueEnterHandle,
) -> ! {
    let mut logger = crate::linux::util::strace_logger();
    write!(
        logger,
        "sandbox {}: thread C (zygote main)",
        &jail_options.jail_id
    )
    .unwrap();
    let zyg_opts = ZygoteOptions {
        jail_options,
        sock: zyg_sock,
        uid_mapping_done,
        resource_group_enter_handle,
    };
    let zygote_ret_code = main_loop::entry(zyg_opts);

    unsafe {
        libc::exit(zygote_ret_code.map(main_loop::ReturnCode::get).unwrap_or(1));
    }
}
