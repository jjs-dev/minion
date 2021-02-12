use crate::linux::util::cvt_error;
use nix::fcntl::{fcntl, FcntlArg, FdFlag};
use std::{
    mem::ManuallyDrop,
    os::unix::prelude::{AsRawFd, RawFd},
};

/// Represents owned file descriptor
pub struct Fd(RawFd);

impl AsRawFd for Fd {
    fn as_raw_fd(&self) -> RawFd {
        self.as_raw()
    }
}

impl Fd {
    pub fn new(inner: RawFd) -> Self {
        Fd(inner)
    }

    pub fn as_raw(&self) -> RawFd {
        self.0
    }

    pub fn into_raw(self) -> RawFd {
        let this = ManuallyDrop::new(self);
        this.0
    }

    pub fn read(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        nix::unistd::read(self.0, buf).map_err(cvt_error)
    }

    pub fn write(&self, buf: &[u8]) -> std::io::Result<usize> {
        nix::unistd::write(self.0, buf).map_err(cvt_error)
    }

    pub fn duplicate_with_inheritance(&self) -> nix::Result<Self> {
        let f = nix::unistd::dup(self.0)?;
        Ok(Fd::new(f))
    }

    pub fn fcntl(&self, arg: FcntlArg) -> nix::Result<()> {
        fcntl(self.0, arg).map(drop)
    }

    pub fn allow_inherit(&self) -> nix::Result<()> {
        self.fcntl(FcntlArg::F_SETFD(FdFlag::empty()))
    }
}

impl Drop for Fd {
    fn drop(&mut self) {
        nix::unistd::close(self.0).unwrap();
    }
}
