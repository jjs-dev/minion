use crate::linux::{limits::EnterHandle, Error};
use std::{os::unix::prelude::RawFd, path::PathBuf};

/// Information, sufficient for joining a cgroup.
#[derive(Clone)]
pub(super) enum CgroupEnter {
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

impl CgroupEnter {
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
}

impl EnterHandle for CgroupEnter {
    fn join(self) -> anyhow::Result<()> {
        let my_pid = std::process::id();
        let mut buf = itoa::Buffer::new();
        let my_pid = buf.format(my_pid);
        let mut err = None;
        self.with(|it| {
            for fd in it {
                if let Err(e) = nix::unistd::write(fd, my_pid.as_bytes()) {
                    err.replace(e);
                    break;
                }
            }
        });
        match err {
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }

    fn check_access(&self) -> Result<(), Error> {
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
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }
}

impl Drop for CgroupEnter {
    fn drop(&mut self) {
        self.with(|it| {
            for fd in it {
                nix::unistd::close(fd).ok();
            }
        })
    }
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
