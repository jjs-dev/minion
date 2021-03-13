use crate::linux::{fd::Fd, util::cvt_error};
use std::io;

pub struct LinuxReadPipe {
    fd: Fd,
}

impl std::io::Read for LinuxReadPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.fd.read(buf)
    }
}

impl LinuxReadPipe {
    pub fn new(fd: Fd) -> LinuxReadPipe {
        LinuxReadPipe { fd }
    }

    pub fn inner(&self) -> &Fd {
        &self.fd
    }
}

pub struct LinuxWritePipe {
    fd: Fd,
}

impl LinuxWritePipe {
    fn new(fd: Fd) -> LinuxWritePipe {
        LinuxWritePipe { fd }
    }

    pub fn inner(&self) -> &Fd {
        &self.fd
    }

    pub fn into_inner(self) -> Fd {
        self.fd
    }
}

impl io::Write for LinuxWritePipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.fd.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        nix::unistd::fsync(self.fd.as_raw()).map_err(cvt_error)
    }
}

pub(crate) fn setup_pipe() -> Result<(LinuxWritePipe, LinuxReadPipe), crate::linux::Error> {
    let ends = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC)?;
    Ok((
        LinuxWritePipe::new(Fd::new(ends.1)),
        LinuxReadPipe::new(Fd::new(ends.0)),
    ))
}
