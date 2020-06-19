use crate::{
    linux::{
        jail_common::{self, JailOptions},
        util::{err_exit, Handle, IpcSocketExt, Pid, StraceLogger},
        zygote::{
            SANDBOX_INTERNAL_UID, WM_CLASS_PID_MAP_CREATED, WM_CLASS_PID_MAP_READY_FOR_SETUP,
            WM_CLASS_SETUP_FINISHED,
        },
    },
    SharedDir, SharedDirKind,
};
use std::{ffi::CString, fs, io, io::Write, os::unix::ffi::OsStrExt, path::Path, ptr, time};
use tiny_nix_ipc::Socket;

pub(in crate::linux) struct SetupData {
    pub(in crate::linux) cgroups: super::cgroup::Group,
}

fn configure_dir(dir_path: &Path) -> crate::Result<()> {
    use nix::sys::stat::Mode;
    let mode = Mode::S_IRUSR
        | Mode::S_IWUSR
        | Mode::S_IXUSR
        | Mode::S_IRGRP
        | Mode::S_IWGRP
        | Mode::S_IXGRP
        | Mode::S_IROTH
        | Mode::S_IWOTH
        | Mode::S_IXOTH;
    let path = CString::new(dir_path.as_os_str().as_bytes()).unwrap();
    nix::sys::stat::fchmodat(
        None,
        path.as_c_str(),
        mode,
        nix::sys::stat::FchmodatFlags::FollowSymlink,
    )?;
    let uid = nix::unistd::Uid::from_raw(SANDBOX_INTERNAL_UID);
    let gid = nix::unistd::Gid::from_raw(SANDBOX_INTERNAL_UID);
    nix::unistd::chown(path.as_c_str(), Some(uid), Some(gid))?;
    Ok(())
}

fn expose_dir(jail_root: &Path, system_path: &Path, alias_path: &Path, kind: SharedDirKind) {
    let bind_target = jail_root.join(alias_path);
    fs::create_dir_all(&bind_target).unwrap();
    let stat = fs::metadata(&system_path)
        .unwrap_or_else(|err| panic!("failed to stat {}: {}", system_path.display(), err));
    if stat.is_file() {
        fs::remove_dir(&bind_target).unwrap();
        fs::write(&bind_target, &"").unwrap();
    }
    let bind_target = CString::new(bind_target.as_os_str().as_bytes()).unwrap();
    let bind_src = CString::new(system_path.as_os_str().as_bytes()).unwrap();
    unsafe {
        let mnt_res = libc::mount(
            bind_src.as_ptr(),
            bind_target.as_ptr(),
            ptr::null(),
            libc::MS_BIND,
            ptr::null(),
        );
        if mnt_res == -1 {
            err_exit("mount");
        }

        if let SharedDirKind::Readonly = kind {
            let rem_ret = libc::mount(
                ptr::null(),
                bind_target.as_ptr(),
                ptr::null(),
                libc::MS_BIND | libc::MS_REMOUNT | libc::MS_RDONLY,
                ptr::null(),
            );
            if rem_ret == -1 {
                err_exit("mount");
            }
        }
    }
}

pub(crate) fn expose_dirs(expose: &[SharedDir], jail_root: &Path) {
    // mount --bind
    for x in expose {
        expose_dir(jail_root, &x.src, &x.dest, x.kind.clone())
    }
}

extern "C" fn exit_sighandler(_code: i32) {
    unsafe {
        libc::exit(1);
    }
}

fn setup_sighandler() {
    use nix::sys::signal;
    for &death in &[
        signal::Signal::SIGABRT,
        signal::Signal::SIGINT,
        signal::Signal::SIGSEGV,
    ] {
        let handler = signal::SigHandler::SigDfl;
        let action =
            signal::SigAction::new(handler, signal::SaFlags::empty(), signal::SigSet::empty());
        // Safety: default action is correct
        unsafe {
            signal::sigaction(death, &action).expect("Couldn't setup sighandler");
        }
    }
    {
        let sigterm_handler = signal::SigHandler::Handler(exit_sighandler);
        let action = signal::SigAction::new(
            sigterm_handler,
            signal::SaFlags::empty(),
            signal::SigSet::empty(),
        );
        // Safety: `sigterm_handler` only calls allowed functions
        unsafe {
            signal::sigaction(signal::Signal::SIGTERM, &action)
                .expect("Failed to setup SIGTERM handler");
        }
    }
}

fn setup_chroot(jail_options: &JailOptions) -> crate::Result<()> {
    let path = &jail_options.isolation_root;
    nix::unistd::chroot(path)?;
    nix::unistd::chdir("/")?;
    Ok(())
}

fn setup_procfs(jail_options: &JailOptions) -> crate::Result<()> {
    let procfs_path = jail_options.isolation_root.join(Path::new("proc"));
    match fs::create_dir(&procfs_path) {
        Ok(_) => (),
        Err(e) => match e.kind() {
            io::ErrorKind::AlreadyExists => (),
            _ => Err(e).unwrap(),
        },
    }
    nix::mount::mount(
        Some("proc"),
        procfs_path.as_path(),
        Some("proc"),
        nix::mount::MsFlags::empty(),
        None::<&str>,
    )?;
    Ok(())
}

fn setup_uid_mapping(sock: &mut Socket) -> crate::Result<()> {
    sock.wake(WM_CLASS_PID_MAP_READY_FOR_SETUP)?;
    sock.lock(WM_CLASS_PID_MAP_CREATED)?;
    Ok(())
}

fn setup_time_watch(jail_options: &JailOptions) -> crate::Result<()> {
    let cpu_tl = jail_options.cpu_time_limit.as_nanos() as u64;
    let real_tl = jail_options.real_time_limit.as_nanos() as u64;
    observe_time(
        &jail_options.jail_id,
        cpu_tl,
        real_tl,
        jail_options.watchdog_chan,
    )
}

fn setup_expositions(options: &JailOptions) {
    expose_dirs(&options.exposed_paths, &options.isolation_root);
}

fn setup_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let mut logger = StraceLogger::new();
        write!(logger, "PANIC: {}", info).ok();
        let bt = backtrace::Backtrace::new();
        write!(logger, "{:?}", &bt).ok();
        // Now write same to stdout
        unsafe {
            logger.set_fd(2);
        }
        write!(logger, "PANIC: {}", info).ok();
        write!(logger, "{:?}", &bt).ok();
        write!(logger, "Exiting").ok();
        unsafe {
            libc::exit(libc::EXIT_FAILURE);
        }
    }));
}

pub(in crate::linux) fn setup(
    jail_params: &JailOptions,
    sock: &mut Socket,
) -> crate::Result<SetupData> {
    setup_panic_hook();
    setup_sighandler();
    // must be done before `configure_dir`.
    setup_uid_mapping(sock)?;
    configure_dir(&jail_params.isolation_root)?;
    setup_expositions(&jail_params);
    setup_procfs(&jail_params)?;
    let handles = super::cgroup::setup_cgroups(&jail_params);
    setup_time_watch(&jail_params)?;
    setup_chroot(&jail_params)?;
    sock.wake(WM_CLASS_SETUP_FINISHED)?;
    let mut logger = crate::linux::util::StraceLogger::new();
    writeln!(logger, "sandbox {}: setup done", &jail_params.jail_id).unwrap();
    let res = SetupData { cgroups: handles };
    Ok(res)
}

/// Internal function, kills processes which used all their CPU time limit.
/// Limits are given in nanoseconds
fn cpu_time_observer(
    jail_id: &str,
    cpu_time_limit: u64,
    real_time_limit: u64,
    chan: std::os::unix::io::RawFd,
) -> ! {
    let mut logger = crate::linux::util::StraceLogger::new();
    writeln!(logger, "sandbox {}: cpu time watcher", jail_id).unwrap();
    let start = time::Instant::now();
    loop {
        nix::unistd::sleep(1);

        let elapsed = time::Instant::now().duration_since(start);
        let elapsed = elapsed.as_nanos();
        let current_usage = super::cgroup::get_cpu_usage(jail_id);
        let was_cpu_tle = current_usage > cpu_time_limit;
        let was_real_tle = elapsed as u64 > real_time_limit;
        let ok = !was_cpu_tle && !was_real_tle;
        if ok {
            continue;
        }
        if was_cpu_tle {
            writeln!(logger, "minion-watchdog: CPU time limit exceeded").unwrap();
            nix::unistd::write(chan, b"c").ok();
        } else if was_real_tle {
            writeln!(
                logger,
                "minion-watchdog: Real time limit exceeded: limit {}ns, used {}ns",
                real_time_limit, elapsed
            )
            .unwrap();
            nix::unistd::write(chan, b"r").ok();
        }
        // since we are inside pid ns, we can refer to zygote as pid1.
        let err = jail_common::sandbox_kill_all(1 as Pid, None);
        if let Err(err) = err {
            eprintln!("minion-watchdog: failed to kill sandbox {}", err);
        }
        // we will be killed by kernel too
    }
}

fn observe_time(
    jail_id: &str,
    cpu_time_limit: u64,
    real_time_limit: u64,
    chan: Handle,
) -> crate::Result<()> {
    let fret = nix::unistd::fork()?;

    match fret {
        nix::unistd::ForkResult::Child => {
            cpu_time_observer(jail_id, cpu_time_limit, real_time_limit, chan)
        }
        nix::unistd::ForkResult::Parent { .. } => Ok(()),
    }
}
