/// Implements Cgroup Driver - high-level cgroup manager
mod detect;
mod v1;
mod v2;

use std::{ffi::OsString, fmt, os::unix::io::RawFd, path::PathBuf};

// used by crate::linux::check
pub(in crate::linux) use detect::CgroupVersion;

use super::Error;

/// Information, sufficient for joining a cgroup.
pub(in crate::linux) enum JoinHandle {
    /// Fds of `tasks` file in each hierarchy.
    V1(Vec<RawFd>),
    /// Fd of `cgroup.procs` file in cgroup dir.
    V2(RawFd),
}

fn is_einval(e: &nix::Error) -> bool {
    match e {
        nix::Error::Sys(errno) => *errno == nix::errno::Errno::EINVAL,
        _ => false,
    }
}

impl JoinHandle {
    fn with(&self, f: impl FnOnce(&mut dyn Iterator<Item = RawFd>)) {
        let mut slice_iter;
        let mut once_iter;
        let it: &mut dyn std::iter::Iterator<Item = RawFd> = match &self {
            Self::V1(handles) => {
                slice_iter = handles.iter().copied();
                &mut slice_iter
            }
            Self::V2(handle) => {
                once_iter = std::iter::once(*handle);
                &mut once_iter
            }
        };
        f(it);
    }

    pub(super) fn check_access(&self) -> Result<(), nix::Error> {
        let mut err = None;
        self.with(|it| {
            for fd in it {
                if let Err(e) = nix::unistd::write(fd, &[]) {
                    if !is_einval(&e) {
                        err.replace(e);
                    }
                }
            }
        });
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    pub(super) fn join_self(&self) {
        let my_pid = std::process::id();
        let mut buf = itoa::Buffer::new();
        let my_pid = buf.format(my_pid);
        self.with(|it| {
            for fd in it {
                if let Err(_e) = nix::unistd::write(fd, my_pid.as_bytes()) {
                    nix::unistd::write(libc::STDERR_FILENO, b"failed to join cgroup").ok();
                    unsafe {
                        libc::_exit(libc::EXIT_FAILURE);
                    }
                }
            }
        });
    }
}

impl Drop for JoinHandle {
    fn drop(&mut self) {
        self.with(|it| {
            for fd in it {
                nix::unistd::close(fd).ok();
            }
        })
    }
}

/// Abstracts all cgroups manipulations.
/// Each Backend has exactly one Driver.
#[derive(Debug)]
pub(in crate::linux) struct Driver {
    cgroupfs_path: PathBuf,
    cgroup_prefix: Vec<OsString>,
    version: detect::CgroupVersion,
}

#[derive(Debug, thiserror::Error)]
pub enum CgroupError {
    #[error("failed to write data to {path}")]
    Write {
        path: PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("failed to read data from {path}")]
    Read {
        path: PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("failed to create cgroup directory")]
    CreateCgroupDir {
        path: PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("failed to open file {path}")]
    OpenFile {
        path: PathBuf,
        #[source]
        cause: std::io::Error,
    },
    #[error("failed to duplicate handle")]
    DuplicateFd {
        #[source]
        cause: nix::Error,
    },
    #[error("unable to join cgroup")]
    Join {
        #[source]
        cause: nix::Error,
    },
    /// This error can only happen during initialization
    /// as forking is part of smoke-tests and config detection.
    #[error("failed to fork")]
    Fork {
        #[source]
        cause: nix::Error,
    },
}

#[derive(Debug)]
pub struct CgroupDetectionError {
    pub attempts: Vec<(RawSettings, crate::linux::Error)>,
}

impl fmt::Display for CgroupDetectionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (settings, error) in &self.attempts {
            writeln!(f, "Tried {:?}, got:", settings)?;
            let mut cur = error as &dyn std::error::Error;
            loop {
                writeln!(f, "\t{}", cur)?;
                cur = match cur.source() {
                    Some(next) => next,
                    None => break,
                };
            }
        }
        Ok(())
    }
}

impl std::error::Error for CgroupDetectionError {}

/// Represents resource limits imposed on sandbox
pub(in crate::linux) struct ResourceLimits {
    pub(in crate::linux) pids_max: u32,
    pub(in crate::linux) memory_max: u64,
}

/// Raw Cgroup Driver creation arguments.
/// This struct should be treated as completely opaque,
/// its fields are private. It is public because it is member
/// of [`CgroupDetectionError` struct](CgroupDetectionError).
#[derive(Debug)]
pub struct RawSettings {
    cgroup_prefix: PathBuf,
    cgroupfs: PathBuf,
    cgroup_version: CgroupVersion,
}

impl Driver {
    pub(in crate::linux) fn new_raw(settings: &RawSettings) -> Driver {
        let mut cgroup_prefix = Vec::new();
        for comp in settings.cgroup_prefix.components() {
            if let std::path::Component::Normal(n) = comp {
                cgroup_prefix.push(n.to_os_string());
            }
        }
        Driver {
            cgroupfs_path: settings.cgroupfs.clone(),
            cgroup_prefix,
            version: settings.cgroup_version,
        }
    }

    #[tracing::instrument]
    pub(in crate::linux) fn new(
        settings: &crate::linux::Settings,
    ) -> Result<Driver, crate::linux::Error> {
        let mut configs = Vec::new();
        for &cgroup_version in &[CgroupVersion::V1, CgroupVersion::V2] {
            configs.push(RawSettings {
                cgroup_prefix: settings.cgroup_prefix.clone(),
                cgroupfs: settings.cgroupfs.clone(),
                cgroup_version,
            });
        }

        let mut err = CgroupDetectionError {
            attempts: Vec::new(),
        };
        for config in configs {
            let driver = Self::new_raw(&config);
            match driver.smoke_check() {
                Ok(()) => {
                    tracing::debug!(settings=?config, "Found working configuration");
                    return Ok(driver);
                }
                Err(e) => {
                    tracing::debug!(settings=?config, error=%e, "Configuration does not work");
                    err.attempts.push((config, e.into()));
                }
            }
        }
        Err(Error::CgroupDetection { cause: err })
    }

    pub(in crate::linux) fn create_group(
        &self,
        cgroup_id: &str,
        limits: &ResourceLimits,
    ) -> Result<JoinHandle, CgroupError> {
        let handle = match self.version {
            CgroupVersion::V1 => JoinHandle::V1(self.setup_cgroups_v1(limits, cgroup_id)?),
            CgroupVersion::V2 => JoinHandle::V2(self.setup_cgroups_v2(limits, cgroup_id)?),
        };
        Ok(handle)
    }

    pub(in crate::linux) fn get_cpu_usage(&self, cgroup_id: &str) -> Result<u64, CgroupError> {
        let usage = match self.version {
            CgroupVersion::V1 => self.get_cpu_usage_v1(cgroup_id)?,
            CgroupVersion::V2 => self.get_cpu_usage_v2(cgroup_id)?,
        };
        Ok(usage)
    }

    pub(in crate::linux) fn get_memory_usage(
        &self,
        cgroup_id: &str,
    ) -> Result<Option<u64>, CgroupError> {
        let usage = match self.version {
            // memory cgroup v2 does not provide way to get peak memory usage.
            // `memory.current` contains only current usage.
            CgroupVersion::V2 => None,
            CgroupVersion::V1 => Some(self.get_memory_usage_v1(cgroup_id)?),
        };
        Ok(usage)
    }

    pub(in crate::linux) fn drop_cgroup(&self, cgroup_id: &str, legacy_subsystems: &[&str]) {
        match self.version {
            CgroupVersion::V1 => self.drop_cgroup_v1(cgroup_id, legacy_subsystems),
            CgroupVersion::V2 => self.drop_cgroup_v2(cgroup_id),
        }
    }
}
