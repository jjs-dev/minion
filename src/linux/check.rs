use crate::linux::cgroup::Driver;

/// `crate::check()` on linux
pub fn check(settings: &crate::linux::Settings, res: &mut crate::check::CheckResult) {
    if !pidfd_supported() {
        res.warning("PID file descriptors not supported")
    }
    if let Err(err) = Driver::new(settings) {
        res.error(&format!("Cgroup failure: {:#}", err));
    }
}

/// Checks if the kernel has support for PID file descriptors.
pub fn pidfd_supported() -> bool {
    static ONCE: once_cell::sync::Lazy<bool> = once_cell::sync::Lazy::new(|| {
        fn check() -> Result<(), std::io::Error> {
            let me = nix::unistd::Pid::parent();
            let pidfd = crate::linux::util::pidfd_open(me.as_raw())?;
            let send_res =
                crate::linux::util::pidfd_send_signal(pidfd, 0).or_else(|err| match err.kind() {
                    std::io::ErrorKind::InvalidInput => Ok(()),
                    _ => Err(err),
                });
            nix::unistd::close(pidfd).unwrap();
            send_res
        }
        check().is_ok()
    });
    *ONCE
}

pub(crate) fn run_all_feature_checks() {
    let _ = pidfd_supported();
}
