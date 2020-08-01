use crate::linux::Error;
use libc::{self, c_char, c_void};
use std::{
    ffi::{CString, OsStr},
    io,
    os::unix::{ffi::OsStrExt, io::RawFd},
};
use tiny_nix_ipc::{self, Socket};

pub type Fd = RawFd;

pub type Pid = libc::pid_t;
pub type ExitCode = i64;
pub type Uid = libc::uid_t;

pub fn get_last_error() -> i32 {
    errno::errno().0
}

pub fn err_exit(syscall_name: &str) -> ! {
    unsafe {
        let e = errno::errno();
        eprintln!("{}() failed with error {}: {}", syscall_name, e.0, e);
        if libc::getpid() != 1 {
            panic!("syscall error (msg upper)")
        } else {
            libc::exit(libc::EXIT_FAILURE);
        }
    }
}

fn sock_lock(sock: &mut Socket, expected_class: &'static [u8]) -> Result<(), Error> {
    use std::io::Write;
    let mut logger = strace_logger();
    let mut recv_buf = vec![0; expected_class.len()];
    match sock.recv_into_slice::<[RawFd; 0]>(&mut recv_buf) {
        Ok(x) => x,
        Err(e) => {
            writeln!(logger, "receive error: {:?}", e).unwrap();
            return Err(Error::Sandbox);
        }
    };
    if recv_buf != expected_class {
        writeln!(
            logger,
            "validation error: invalid class (expected {}, got {})",
            String::from_utf8_lossy(expected_class),
            String::from_utf8_lossy(&recv_buf)
        )
        .unwrap();
        return Err(Error::Sandbox);
    };
    Ok(())
}

fn sock_wake(sock: &mut Socket, wake_class: &'static [u8]) -> Result<(), Error> {
    match sock.send_slice(&wake_class, None) {
        Ok(_) => Ok(()),
        Err(_) => Err(Error::Sandbox),
    }
}

pub(crate) trait IpcSocketExt {
    fn lock(&mut self, expected_class: &'static [u8]) -> Result<(), Error>;
    fn wake(&mut self, wake_class: &'static [u8]) -> Result<(), Error>;

    fn send<T: serde::ser::Serialize>(&mut self, data: &T) -> Result<(), Error>;
    fn recv<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, Error>;
}

const MAX_MSG_SIZE: usize = 16384;

impl IpcSocketExt for Socket {
    fn lock(&mut self, expected_class: &'static [u8]) -> Result<(), Error> {
        sock_lock(self, expected_class)
    }

    fn wake(&mut self, wake_class: &'static [u8]) -> Result<(), Error> {
        sock_wake(self, wake_class)
    }

    fn send<T: serde::ser::Serialize>(&mut self, data: &T) -> Result<(), Error> {
        let data = serde_json::to_vec(data).unwrap();
        assert!(data.len() <= MAX_MSG_SIZE);
        self.send_slice(&data, None)
            .map(|_num_written| ())
            .map_err(|_e| Error::Sandbox)
    }

    fn recv<T: serde::de::DeserializeOwned>(&mut self) -> Result<T, Error> {
        use std::io::Write;
        let mut logger = StraceLogger::new();
        let mut buf = vec![0; MAX_MSG_SIZE];

        let num_read = match self.recv_into_slice::<[RawFd; 0]>(&mut buf) {
            Ok(cnt) => cnt.0,
            Err(_e) => return Err(Error::Sandbox),
        };
        writeln!(logger, "util::recv() got message of {} bytes", num_read).ok();
        match serde_json::from_slice(&buf[..num_read]) {
            Ok(x) => Ok(x),
            Err(e) => {
                writeln!(logger, "ERROR: deserialization failed: {}", e).ok();
                Err(Error::Sandbox)
            }
        }
    }
}

pub fn duplicate_string(arg: &OsStr) -> *mut c_char {
    unsafe {
        let cstr = CString::new(arg.as_bytes()).unwrap();
        let strptr = cstr.as_ptr();
        libc::strdup(strptr)
    }
}

const STRACE_LOGGER_FD: Fd = -779;

#[derive(Copy, Clone, Default)]
pub struct StraceLogger(Fd);

#[allow(dead_code)]
pub fn strace_logger() -> StraceLogger {
    StraceLogger(STRACE_LOGGER_FD)
}

impl StraceLogger {
    pub fn new() -> StraceLogger {
        strace_logger()
    }

    pub unsafe fn set_fd(&mut self, f: i32) {
        self.0 = f;
    }
}

impl io::Write for StraceLogger {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        unsafe {
            libc::write(self.0, buf.as_ptr() as *const c_void, buf.len());
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        // empty
        Ok(())
    }
}
