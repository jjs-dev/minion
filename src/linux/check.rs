use std::cmp::{max, min};

use crate::linux::limits::Driver;

/// `crate::check()` on linux
pub fn check(settings: &crate::linux::Settings, res: &mut crate::check::CheckResult) {
    if !pidfd_supported() {
        res.warning("PID file descriptors not supported")
    }
    if let Err(err) = Driver::new(settings) {
        res.error(&format!(
            "Resource limits failure: {:#}",
            anyhow::Error::new(err)
        ));
    }
    if !settings.rootless {
        check_uid(settings, res);
    }
}

// https://github.com/torvalds/linux/blob/6ad4bf6ea1609fb539a62f10fca87ddbd53a0315/include/uapi/linux/capability.h
const CAP_SETGID: u64 = 1 << 6;
const CAP_SETUID: u64 = 1 << 7;

fn check_uid(settings: &crate::linux::Settings, res: &mut crate::check::CheckResult) {
    let me = match procfs::process::Process::myself() {
        Ok(m) => m,
        Err(err) => {
            res.warning(&format!(
                "procfs not accessible, unable to perform some checks: {}",
                err
            ));
            return;
        }
    };
    let status = match me.status() {
        Ok(s) => s,
        Err(err) => {
            res.warning(&format!(
                "failed to parse /proc/self/status, unable to perform some checks: {}",
                err
            ));
            return;
        }
    };
    let caps = status.capeff;
    if caps & CAP_SETUID == 0 {
        res.error("CAP_SETUID missing");
    }
    if caps & CAP_SETGID == 0 {
        res.error("CAP_GETUID missing");
    }
    for &mapping_path in &["/proc/self/uid_map", "/proc/self/gid_map"] {
        let mapping = match std::fs::read_to_string(mapping_path) {
            Ok(m) => m,
            Err(err) => {
                res.warning(&format!(
                    "failed to read {}, unable to perform come checks: {}",
                    mapping_path, err
                ));
                continue;
            }
        };
        let mut covered = 0;
        for line in mapping.lines() {
            let mut spl = line.split_ascii_whitespace();
            let our_start = spl.next();
            let parent_start = spl.next();
            let len = spl.next();
            let (our_start, _parent_start, len) = match (our_start, parent_start, len, spl.next()) {
                (Some(a), Some(b), Some(c), None) => (a, b, c),
                _ => {
                    res.warning(&format!("failed to parse a line in {}, this can lead to false positives: line did not contain three fields", mapping_path));
                    continue;
                }
            };
            let our_start: u32 = our_start.parse().unwrap();
            let len: u32 = len.parse().unwrap();
            let our_end = our_start + len;
            let intersection_begin = max(our_start, settings.uid.low);
            let intersection_end = min(our_end, settings.uid.high);
            if intersection_begin <= intersection_end {
                covered += intersection_end - intersection_begin;
            }
        }
        if covered != settings.uid.high - settings.uid.low {
            res.error(&format!(
                "Settings specify a range [{}; {}), but {} only maps {} identifiers",
                settings.uid.low, settings.uid.high, mapping_path, covered
            ));
        }
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
