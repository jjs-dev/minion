use std::{io, os::unix::io::RawFd};

pub struct LinuxReadPipe {
    fd: RawFd,
}

impl std::io::Read for LinuxReadPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        nix::unistd::read(self.fd, buf)
            .map_err(|e| e.as_errno().unwrap())
            .map_err(Into::into)
    }
}

impl LinuxReadPipe {
    pub(crate) fn new(fd: RawFd) -> LinuxReadPipe {
        LinuxReadPipe { fd }
    }
}

impl Drop for LinuxReadPipe {
    fn drop(&mut self) {
        nix::unistd::close(self.fd).ok();
    }
}

pub struct LinuxWritePipe {
    fd: RawFd,
}

impl Drop for LinuxWritePipe {
    fn drop(&mut self) {
        nix::unistd::close(self.fd).ok();
    }
}

impl LinuxWritePipe {
    pub(crate) fn new(fd: RawFd) -> LinuxWritePipe {
        LinuxWritePipe { fd }
    }
}

impl io::Write for LinuxWritePipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        nix::unistd::write(self.fd, buf)
            .map_err(|e| e.as_errno().unwrap())
            .map_err(Into::into)
    }

    fn flush(&mut self) -> io::Result<()> {
        nix::unistd::fsync(self.fd).map_err(|e| e.as_errno().unwrap())?;
        Ok(())
    }
}

pub(crate) fn setup_pipe(
    read_end: &mut RawFd,
    write_end: &mut RawFd,
) -> Result<(), crate::linux::Error> {
    let ends = nix::unistd::pipe2(nix::fcntl::OFlag::O_CLOEXEC)?;
    *read_end = ends.0;
    *write_end = ends.1;
    Ok(())
}
