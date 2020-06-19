/*!
 * This crate provides ability to spawn highly isolated processes
 *
 * # Platform support
 * _warning_: not all features are supported by all backends. See documentation for particular backend
 * to know more
 */
#![cfg_attr(minion_nightly, feature(unsafe_block_in_unsafe_fn))]
#![cfg_attr(minion_nightly, warn(unsafe_op_in_unsafe_fn))]
mod command;

#[cfg(target_os = "linux")]
pub mod linux;

pub mod erased;

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub use crate::linux::{LinuxBackend, LinuxChildProcess, LinuxSandbox};

use std::{
    fmt::Debug,
    io::{Read, Write},
    time::Duration,
};

/// This functions checks for system configurations issues.
/// If it returns None, minion will probably work.
/// If it returns Some(s), s is human-readable string
/// describing these problems. It should be shown to administrtor,
/// so that they can fix this problem.
pub fn check() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        if let Err(err) = linux::check::check() {
            return Some(err);
        }
    }
    None
}

/// Represents way of isolation
pub trait Backend: Debug + Send + Sync {
    type Sandbox: Sandbox;
    type ChildProcess: ChildProcess;
    fn new_sandbox(&self, options: SandboxOptions) -> Result<Self::Sandbox>;
    fn spawn(&self, options: ChildProcessOptions<Self::Sandbox>) -> Result<Self::ChildProcess>;
}

pub use command::Command;

/// Mount options.
/// * Readonly: jailed process can read & execute, but not write to
/// * Full: jailed process can read & write & execute
///
/// Anyway, SUID-bit will be disabled.
///
/// Warning: this type is __unstable__ (i.e. not covered by SemVer) and __non-portable__
#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum SharedDirKind {
    Readonly,
    Full,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SharedDir {
    /// Path on system
    pub src: PathBuf,
    /// Path for child
    pub dest: PathBuf,
    pub kind: SharedDirKind,
}

/// This struct is returned by `Sandbox::resource_usage`
/// It represents various resource usage
/// Some items can be absent or rounded
#[derive(Debug, Copy, Clone, Default)]
pub struct ResourceUsageData {
    /// Total CPU time usage in nanoseconds
    pub time: Option<u64>,
    /// Max memory usage in bytes
    pub memory: Option<u64>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SandboxOptions {
    pub max_alive_process_count: u32,
    /// Memory limit for all processes in cgroup, in bytes
    pub memory_limit: u64,
    /// Specifies total CPU time for whole sandbox
    pub cpu_time_limit: Duration,
    /// Specifies total wall-clock timer limit for whole sandbox
    pub real_time_limit: Duration,
    pub isolation_root: PathBuf,
    pub exposed_paths: Vec<SharedDir>,
}

impl SandboxOptions {
    fn make_relative<'a>(&self, p: &'a Path) -> &'a Path {
        if p.starts_with("/") {
            p.strip_prefix("/").unwrap()
        } else {
            p
        }
    }

    fn postprocess(&mut self) {
        let mut paths = std::mem::replace(&mut self.exposed_paths, Vec::new());
        for x in &mut paths {
            x.dest = self.make_relative(&x.dest).to_path_buf();
        }
        std::mem::swap(&mut paths, &mut self.exposed_paths);
    }
}

/// Represents highly-isolated sandbox
pub trait Sandbox: Clone + Debug + 'static {
    fn id(&self) -> String;

    /// Returns true if sandbox exceeded CPU time limit
    fn check_cpu_tle(&self) -> Result<bool>;

    /// Returns true if sandbox exceeded wall-clock time limit
    fn check_real_tle(&self) -> Result<bool>;

    /// Kills all processes in sandbox.
    /// Probably, subsequent `spawn` requests will fail.
    fn kill(&self) -> Result<()>;

    /// Returns information about resource usage by total sandbox
    fn resource_usage(&self) -> Result<ResourceUsageData>;
}

/// Configures stdin for child
#[derive(Debug, Clone)]
enum InputSpecificationData {
    Null,
    Empty,
    Pipe,
    Handle(u64),
}

#[derive(Debug, Clone)]
pub struct InputSpecification(InputSpecificationData);

impl InputSpecification {
    pub fn null() -> Self {
        Self(InputSpecificationData::Null)
    }

    pub fn empty() -> Self {
        Self(InputSpecificationData::Empty)
    }

    pub fn pipe() -> Self {
        Self(InputSpecificationData::Pipe)
    }

    /// # Safety
    /// - Handle must not be used since passing to this function
    /// - Handle must be valid
    pub unsafe fn handle(h: u64) -> Self {
        Self(InputSpecificationData::Handle(h))
    }

    /// # Safety
    /// See requirements of `handle`
    pub unsafe fn handle_of<T: std::os::unix::io::IntoRawFd>(obj: T) -> Self {
        unsafe { Self::handle(obj.into_raw_fd() as u64) }
    }
}

/// Configures stdout and stderr for child
#[derive(Debug, Clone)]
enum OutputSpecificationData {
    Null,
    Ignore,
    Pipe,
    Buffer(Option<usize>),
    Handle(u64),
}

impl OutputSpecification {
    pub fn null() -> Self {
        Self(OutputSpecificationData::Null)
    }

    pub fn ignore() -> Self {
        Self(OutputSpecificationData::Ignore)
    }

    pub fn pipe() -> Self {
        Self(OutputSpecificationData::Pipe)
    }

    pub fn buffer(size: usize) -> Self {
        Self(OutputSpecificationData::Buffer(Some(size)))
    }

    pub fn unbounded_buffer() -> Self {
        Self(OutputSpecificationData::Buffer(None))
    }

    /// # Safety
    /// - Handle must not be used since passing to this function
    /// - Handle must be valid
    pub unsafe fn handle(h: u64) -> Self {
        Self(OutputSpecificationData::Handle(h))
    }

    /// # Safety
    /// See requirements of `handle`
    pub unsafe fn handle_of<T: std::os::unix::io::IntoRawFd>(obj: T) -> Self {
        unsafe { Self::handle(obj.into_raw_fd() as u64) }
    }
}

#[derive(Debug, Clone)]
pub struct OutputSpecification(OutputSpecificationData);

/// Specifies how to provide child stdio
#[derive(Debug, Clone)]
pub struct StdioSpecification {
    pub stdin: InputSpecification,
    pub stdout: OutputSpecification,
    pub stderr: OutputSpecification,
}

/// This type should only be used by Backend implementations
/// Use `Command` instead
#[derive(Debug, Clone)]
pub struct ChildProcessOptions<Sandbox> {
    pub path: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<OsString>,
    pub sandbox: Sandbox,
    pub stdio: StdioSpecification,
    /// Child's working dir. Relative to `sandbox` isolation_root
    pub pwd: PathBuf,
}

mod errors {
    #[derive(Eq, PartialEq)]
    pub enum ErrorKind {
        /// This error typically means that isolated process tried to break its sandbox
        Sandbox,
        /// Bug in code, using minion, or in minion itself
        System,
    }

    #[derive(Debug, thiserror::Error)]
    #[non_exhaustive]
    pub enum Error {
        #[error("requested operation is not supported by backend")]
        NotSupported,
        #[error("system call failed in undesired fashion (error code {})", code)]
        Syscall { code: i32 },
        #[error("io error")]
        Io {
            #[from]
            source: std::io::Error,
        },
        #[error("sandbox interaction failed")]
        Sandbox,
        #[error("unknown error")]
        Unknown,
    }

    impl Error {
        pub fn kind(&self) -> ErrorKind {
            match self {
                Error::NotSupported => ErrorKind::System,
                Error::Syscall { .. } => ErrorKind::System,
                Error::Io { .. } => ErrorKind::System,
                Error::Sandbox => ErrorKind::Sandbox,
                Error::Unknown => ErrorKind::System,
            }
        }

        pub fn is_system(&self) -> bool {
            self.kind() == ErrorKind::System
        }

        pub fn is_sandbox(&self) -> bool {
            self.kind() == ErrorKind::Sandbox
        }
    }

    impl From<nix::Error> for Error {
        fn from(err: nix::Error) -> Self {
            if let Some(errno) = err.as_errno() {
                Error::Syscall { code: errno as i32 }
            } else {
                Error::Unknown
            }
        }
    }
}

pub use errors::Error;
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

pub type Result<T> = std::result::Result<T, Error>;

/// May be returned when process was killed
pub const EXIT_CODE_KILLED: i64 = 0x7eaddeadbeeff00d;

/// Returned by [ChildProcess::wait_for_exit]
///
/// [ChildProcess::wait_fot_exit]: trait.ChildProcess.html#tymethod.wait_for_exit
#[derive(Eq, PartialEq, Debug)]
pub enum WaitOutcome {
    /// Child process has exited during `wait_for_exit`
    Exited,
    /// Child process has exited before `wait_for_exit` and it is somehow already reported
    AlreadyFinished,
    /// Child process hasn't exited during `timeout` period
    Timeout,
}

/// Represents child process.
pub trait ChildProcess: Debug + 'static {
    /// Represents pipe from current process to isolated
    type PipeIn: Write + Send + Sync + 'static;
    /// Represents pipe from isolated process to current
    type PipeOut: Read + Send + Sync + 'static;
    /// Returns exit code, if process had exited by the moment of call, or None otherwise.
    fn get_exit_code(&self) -> Result<Option<i64>>;

    /// Returns writeable stream, connected to child stdin
    ///
    /// Stream will only be returned, if corresponding `Stdio` item was `new_pipe`.
    /// Otherwise, None will be returned
    ///
    /// On all subsequent calls, None will be returned

    fn stdin(&mut self) -> Option<Self::PipeIn>;

    /// Returns readable stream, connected to child stdoutn
    ///
    /// Stream will only be returned, if corresponding `Stdio` item was `new_pipe`.
    /// Otherwise, None will be returned
    ///
    /// On all subsequent calls, None will be returned
    fn stdout(&mut self) -> Option<Self::PipeOut>;

    /// Returns readable stream, connected to child stderr
    ///
    /// Stream will only be returned, if corresponding `Stdio` item was `new_pipe`.
    /// Otherwise, None will be returned
    ///
    /// On all subsequent calls, None will be returned
    fn stderr(&mut self) -> Option<Self::PipeOut>;

    /// Waits for child process exit with timeout.
    /// If timeout is None, `wait_for_exit` will block until child has exited
    fn wait_for_exit(&self, timeout: Option<Duration>) -> Result<WaitOutcome>;

    /// Refreshes information about process
    fn poll(&self) -> Result<()>;

    /// Returns whether child process has exited by the moment of call
    /// This function doesn't blocks on waiting (see `wait_for_exit`).
    fn is_finished(&self) -> Result<bool>;
}
