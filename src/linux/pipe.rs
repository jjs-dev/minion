use crate::linux::util::err_exit;
use libc::c_void;
use std::{io, os::unix::io::RawFd};

pub struct LinuxReadPipe {
    fd: RawFd,
}

impl std::io::Read for LinuxReadPipe {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        unsafe {
            let ret = libc::read(self.fd, buf.as_mut_ptr() as *mut c_void, buf.len());
            if ret == -1 {
                err_exit("read");
            }
            Ok(ret as usize)
        }
    }
}

impl LinuxReadPipe {
    pub(crate) fn new(fd: RawFd) -> LinuxReadPipe {
        LinuxReadPipe { fd }
    }
}

impl Drop for LinuxReadPipe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

pub struct LinuxWritePipe {
    fd: RawFd,
}

impl Drop for LinuxWritePipe {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.fd);
        }
    }
}

impl LinuxWritePipe {
    pub(crate) fn new(fd: RawFd) -> LinuxWritePipe {
        LinuxWritePipe { fd }
    }
}

impl io::Write for LinuxWritePipe {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        unsafe {
            let ret = libc::write(self.fd, buf.as_ptr() as *const c_void, buf.len());
            if ret == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(ret as usize)
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        unsafe {
            let ret = libc::fsync(self.fd);
            if ret == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }
    }
}

pub(crate) fn setup_pipe(
    read_end: &mut RawFd,
    write_end: &mut RawFd,
) -> Result<(), crate::linux::Error> {
    unsafe {
        let mut ends = [0; 2];
        let ret = libc::pipe2(ends.as_mut_ptr(), libc::O_CLOEXEC);
        if ret == -1 {
            err_exit("pipe");
        }
        *read_end = ends[0];
        *write_end = ends[1];
        Ok(())
    }
}
