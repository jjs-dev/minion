use crate::{
    linux::{
        fd::Fd,
        jail_common::{JailOptions, LinuxSharedItem, SharedItemFlags},
        util::{err_exit, StraceLogger},
        zygote::SANDBOX_INTERNAL_UID,
        Error,
    },
    SharedItemKind,
};
use nix::sys::signal;
use std::{ffi::CString, fs, io, io::Write, os::unix::ffi::OsStrExt, path::Path, ptr};

pub(in crate::linux) struct SetupData {
    pub(in crate::linux) cgroup_join_handle: crate::linux::cgroup::JoinHandle,
}

fn configure_dir(dir_path: &Path) -> Result<(), Error> {
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

fn expose_item(
    jail_root: &Path,
    system_path: &Path,
    alias_path: &Path,
    kind: SharedItemKind,
    flags: &SharedItemFlags,
) {
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
        let mut mount_flags = libc::MS_BIND;
        if flags.recursive {
            mount_flags |= libc::MS_REC;
        }
        let mnt_res = libc::mount(
            bind_src.as_ptr(),
            bind_target.as_ptr(),
            ptr::null(),
            mount_flags,
            ptr::null(),
        );
        if mnt_res == -1 {
            err_exit("mount");
        }
        mount_flags |= libc::MS_REMOUNT | libc::MS_RDONLY;

        if let SharedItemKind::Readonly = kind {
            let rem_ret = libc::mount(
                ptr::null(),
                bind_target.as_ptr(),
                ptr::null(),
                mount_flags,
                ptr::null(),
            );
            if rem_ret == -1 {
                err_exit("mount");
            }
        }
    }
}

pub(crate) fn expose_items(expose: &[LinuxSharedItem], jail_root: &Path) {
    // mount --bind
    for x in expose {
        expose_item(jail_root, &x.src, &x.dest, x.kind.clone(), &x.flags)
    }
}

extern "C" fn exit_sighandler(_code: i32) {
    unsafe {
        libc::exit(1);
    }
}

fn setup_sighandler() {
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
    // block SIGCHLD
    // zygote will listen to it by itself
    let mut sigset = signal::SigSet::empty();
    sigset.add(signal::Signal::SIGCHLD);
    signal::sigprocmask(signal::SigmaskHow::SIG_BLOCK, Some(&sigset), None)
        .expect("failed to block SIGCHLD");
}

fn setup_chroot(jail_options: &JailOptions) -> Result<(), Error> {
    let path = &jail_options.isolation_root;
    nix::unistd::chroot(path)?;
    nix::unistd::chdir("/")?;
    Ok(())
}

fn setup_procfs(jail_options: &JailOptions) -> Result<(), Error> {
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

fn setup_expositions(options: &JailOptions) {
    expose_items(&options.shared_items, &options.isolation_root);
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
    uid_mapping_done: &mut Fd,
    cgroup_driver: &crate::linux::cgroup::Driver,
) -> Result<SetupData, Error> {
    setup_panic_hook();
    setup_sighandler();
    // must be done before `configure_dir`.
    {
        // lock until uids are mapped
        uid_mapping_done.read(&mut [0])?;
    }
    configure_dir(&jail_params.isolation_root)?;
    setup_expositions(&jail_params);
    setup_procfs(&jail_params)?;
    let cgroup_join_handle = cgroup_driver.create_group(
        &jail_params.jail_id,
        &crate::linux::cgroup::ResourceLimits {
            pids_max: jail_params.max_alive_process_count,
            memory_max: jail_params.memory_limit,
        },
    )?;
    setup_chroot(&jail_params)?;
    let mut logger = crate::linux::util::StraceLogger::new();
    writeln!(logger, "sandbox {}: setup done", &jail_params.jail_id).unwrap();
    let res = SetupData { cgroup_join_handle };
    Ok(res)
}
