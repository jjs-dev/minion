/*!
 * This crate provides ability to spawn highly isolated processes
 *
 * # Platform support
 * _warning_: not all features are supported by all backends. See documentation for particular backend
 * to know more
 */
#![warn(unsafe_op_in_unsafe_fn)]
mod command;

#[cfg(target_os = "linux")]
pub mod linux;

pub mod erased;

mod check;
pub use check::{check, CheckResult};

use serde::{Deserialize, Serialize};

#[cfg(target_os = "linux")]
pub use crate::linux::{LinuxBackend, LinuxChildProcess, LinuxSandbox};

use std::{
    error::Error as StdError,
    fmt::Debug,
    io::{Read, Write},
    sync::Arc,
    time::Duration,
};

/// Represents way of isolation
pub trait Backend: Debug + Send + Sync + 'static {
    type Error: StdError + Send + Sync + 'static;
    type Sandbox: Sandbox<Error = Self::Error>;
    type ChildProcess: ChildProcess<Error = Self::Error>;
    fn new_sandbox(&self, options: SandboxOptions) -> Result<Self::Sandbox, Self::Error>;
    fn spawn(
        &self,
        options: ChildProcessOptions,
        sandbox: Arc<Self::Sandbox>,
    ) -> Result<Self::ChildProcess, Self::Error>;
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
pub enum SharedItemKind {
    Readonly,
    Full,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct SharedItem {
    /// Optional identifier.
    /// It can be used to provide additional backend-specific settings.
    pub id: Option<String>,
    /// Path on system
    pub src: PathBuf,
    /// Path for child
    pub dest: PathBuf,
    pub kind: SharedItemKind,
    /// Additional mount flags.
    /// Meaning is completely backend specific.
    /// By default, empty list should be used.
    pub flags: Vec<String>,
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
    pub shared_items: Vec<SharedItem>,
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
        let mut paths = std::mem::take(&mut self.shared_items);
        for x in &mut paths {
            x.dest = self.make_relative(&x.dest).to_path_buf();
        }
        std::mem::swap(&mut paths, &mut self.shared_items);
    }
}

/// Represents highly-isolated sandbox
pub trait Sandbox: Debug + Send + Sync + 'static {
    type Error: StdError + Send + Sync + 'static;
    fn id(&self) -> String;

    /// Returns true if sandbox exceeded CPU time limit
    fn check_cpu_tle(&self) -> Result<bool, Self::Error>;

    /// Returns true if sandbox exceeded wall-clock time limit
    fn check_real_tle(&self) -> Result<bool, Self::Error>;

    /// Kills all processes in sandbox.
    /// Probably, subsequent `spawn` requests will fail.
    fn kill(&self) -> Result<(), Self::Error>;

    /// Returns debugging information, such as pathes or process
    /// identifiers.
    fn debug_info(&self) -> Result<serde_json::Value, Self::Error>;

    /// Returns information about resource usage by total sandbox
    fn resource_usage(&self) -> Result<ResourceUsageData, Self::Error>;
}

/// Kernel object descriptor.
#[derive(Debug)]
pub struct Handle(pub(crate) u64);

impl Handle {
    /// # Correctness
    /// - Handle must not be used since passing to this function unless created
    ///   handle instance is left unused
    /// - Handle must be valid
    pub fn new(h: u64) -> Self {
        Handle(h)
    }

    /// # Correctness
    /// See requirements of `handle`
    pub fn of<T: std::os::unix::io::IntoRawFd>(obj: T) -> Self {
        Handle(obj.into_raw_fd() as u64)
    }
}

/// Configures stdin for child
#[derive(Debug)]
enum InputSpecificationData {
    Null,
    Empty,
    Pipe,
    Handle(Handle),
}

#[derive(Debug)]
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

    pub fn handle(h: Handle) -> Self {
        Self(InputSpecificationData::Handle(h))
    }
}

/// Configures stdout and stderr for child
#[derive(Debug)]
enum OutputSpecificationData {
    Null,
    Ignore,
    Pipe,
    Buffer(Option<usize>),
    Handle(Handle),
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

    pub fn handle(h: Handle) -> Self {
        Self(OutputSpecificationData::Handle(h))
    }
}

#[derive(Debug)]
pub struct OutputSpecification(OutputSpecificationData);

/// Specifies how to provide child stdio
#[derive(Debug)]
pub struct StdioSpecification {
    pub stdin: InputSpecification,
    pub stdout: OutputSpecification,
    pub stderr: OutputSpecification,
}

/// This type should only be used by Backend implementations
/// Use `Command` instead
#[derive(Debug)]
pub struct ChildProcessOptions {
    pub path: PathBuf,
    pub arguments: Vec<OsString>,
    pub environment: Vec<OsString>,
    pub stdio: StdioSpecification,
    pub extra_inherit: Vec<Handle>,
    /// Child's working dir. Relative to `sandbox` isolation_root
    pub pwd: PathBuf,
}

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

/// Child process exit code.
#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub struct ExitCode(pub i64);

impl ExitCode {
    /// By convention program returns this code on success
    pub const OK: ExitCode = ExitCode(0);
    /// May be returned when process was killed
    pub const KILLED: ExitCode = ExitCode(0x7eaddeadbeeff00d);
    /// Base code for reporting signals
    pub const SIGNALLED: ExitCode = ExitCode(1000);
}

impl ExitCode {
    pub fn is_success(self) -> bool {
        self.0 == 0
    }

    pub fn linux_signal(sig: i64) -> ExitCode {
        ExitCode(sig + Self::SIGNALLED.0)
    }
}

/// Represents child process.
pub trait ChildProcess: Debug + Send + Sync + 'static {
    type Error: StdError + Send + Sync + 'static;
    /// Represents pipe from current process to isolated
    type PipeIn: Write + Send + Sync + 'static;
    /// Represents pipe from isolated process to current
    type PipeOut: Read + Send + Sync + 'static;
    /// Future for `wait_for_exit` method.
    /// If this function resolves to Err, than wait failed.
    /// Otherwise child has finished and `get_exit_code` function will return
    /// exit code.
    type WaitFuture: std::future::Future<Output = Result<ExitCode, Self::Error>>
        + Send
        + Sync
        + 'static;

    /// Returns a future that resolves when process exited.
    /// This function should be called once.
    fn wait_for_exit(&mut self) -> Result<Self::WaitFuture, Self::Error>;

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
}
