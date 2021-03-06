pub mod check;
pub mod error;
pub mod ext;
mod fd;
mod ipc;
mod jail_common;
mod limits;
mod pipe;
mod sandbox;
mod seccomp;
mod uid_alloc;
mod util;
mod wait;
mod zygote;

use crate::{
    linux::{
        fd::Fd,
        pipe::{LinuxReadPipe, LinuxWritePipe},
        util::{get_last_error, Pid},
    },
    Backend, ChildProcess, ChildProcessOptions, InputSpecification, InputSpecificationData,
    OutputSpecification, OutputSpecificationData, SandboxOptions,
};
pub use error::Error;
use nix::sys::memfd;
pub use sandbox::LinuxSandbox;
use std::{
    ffi::CString,
    fs,
    os::unix::io::{IntoRawFd, RawFd},
    path::PathBuf,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};

use self::uid_alloc::UidAllocator;

pub type LinuxHandle = libc::c_int;
pub struct LinuxChildProcess {
    exit_code: AtomicI64,

    stdin: Option<LinuxWritePipe>,
    stdout: Option<LinuxReadPipe>,
    stderr: Option<LinuxReadPipe>,
    sandbox_ref: Arc<LinuxSandbox>,

    pid: Pid,
    /// FD of object which will be readable when child finishes.
    /// Wrapped in Option to catch user errors.
    fd: Option<Fd>,
}

impl std::fmt::Debug for LinuxChildProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.debug_struct("LinuxChildProcess")
            .field("exit_code", &self.exit_code.load(Ordering::Relaxed))
            .field("pid", &self.pid)
            .finish()
    }
}
// It doesn't intersect with normal exit codes
// because they fit in i32
const EXIT_CODE_STILL_RUNNING: i64 = i64::min_value();

impl ChildProcess for LinuxChildProcess {
    type Error = Error;
    type PipeIn = LinuxWritePipe;
    type PipeOut = LinuxReadPipe;

    type WaitFuture = wait::WaitFuture;

    fn stdin(&mut self) -> Option<LinuxWritePipe> {
        self.stdin.take()
    }

    fn stdout(&mut self) -> Option<LinuxReadPipe> {
        self.stdout.take()
    }

    fn stderr(&mut self) -> Option<LinuxReadPipe> {
        self.stderr.take()
    }

    fn wait_for_exit(&mut self) -> Result<Self::WaitFuture, Error> {
        wait::WaitFuture::new(
            self.fd.take().expect("wait_for_exit called twice"),
            self.pid,
            self.sandbox_ref.clone(),
        )
    }
}

fn handle_input_io(
    spec: InputSpecification,
) -> Result<(Option<LinuxWritePipe>, Option<Fd>), Error> {
    match spec.0 {
        InputSpecificationData::Pipe => {
            let (tx, rx) = pipe::setup_pipe()?;
            let f = rx.inner().duplicate_with_inheritance()?;
            Ok((Some(tx), Some(f)))
        }
        InputSpecificationData::Handle(rh) => {
            let h = Fd::new(rh.0 as RawFd);
            Ok((None, Some(h)))
        }
        InputSpecificationData::Empty => {
            let file = fs::File::create("/dev/null")?;
            let file = file.into_raw_fd();
            let file = Fd::new(file);
            let file = file.duplicate_with_inheritance()?;
            Ok((None, Some(file)))
        }
        InputSpecificationData::Null => Ok((None, None)),
    }
}

fn handle_output_io(
    spec: OutputSpecification,
) -> Result<(Option<LinuxReadPipe>, Option<Fd>), Error> {
    match spec.0 {
        OutputSpecificationData::Null => Ok((None, None)),
        OutputSpecificationData::Handle(rh) => Ok((None, Some(Fd::new(rh.0 as RawFd)))),
        OutputSpecificationData::Pipe => {
            let (tx, rx) = pipe::setup_pipe()?;
            let f = tx.inner().duplicate_with_inheritance()?;
            Ok((Some(rx), Some(f)))
        }
        OutputSpecificationData::Ignore => {
            let file = fs::File::open("/dev/null")?;
            let file = file.into_raw_fd();
            let file = Fd::new(file);
            let file = file.duplicate_with_inheritance()?;
            Ok((None, Some(file)))
        }
        OutputSpecificationData::Buffer(sz) => {
            let memfd_name = "libminion_output_memfd";
            let memfd_name = CString::new(memfd_name).unwrap();
            let mut flags = memfd::MemFdCreateFlag::MFD_CLOEXEC;
            if sz.is_some() {
                flags |= memfd::MemFdCreateFlag::MFD_ALLOW_SEALING;
            }
            let mfd = memfd::memfd_create(&memfd_name, flags).unwrap();
            if let Some(sz) = sz {
                if unsafe { libc::ftruncate(mfd, sz as i64) } == -1 {
                    return Err(Error::Syscall {
                        code: get_last_error(),
                    });
                }
            }
            let mfd = Fd::new(mfd);
            let child_mfd = mfd.duplicate_with_inheritance()?;
            Ok((Some(LinuxReadPipe::new(mfd)), Some(child_mfd)))
        }
    }
}

fn spawn(
    mut options: ChildProcessOptions,
    sandbox: Arc<LinuxSandbox>,
) -> Result<LinuxChildProcess, Error> {
    unsafe {
        let q = jail_common::JobQuery {
            image_path: options.path.clone(),
            argv: options.arguments.clone(),
            environment: std::mem::take(&mut options.environment)
                .into_iter()
                .collect(),
            pwd: options.pwd.clone(),
            extra_fds: options.extra_inherit.iter().map(|h| h.0 as i32).collect(),
        };

        let (in_w, in_r) = handle_input_io(options.stdio.stdin)?;
        let (out_r, out_w) = handle_output_io(options.stdio.stdout)?;
        let (err_r, err_w) = handle_output_io(options.stdio.stderr)?;

        let q = sandbox::ExtendedJobQuery {
            job_query: q,

            stdin: in_r,
            stdout: out_w,
            stderr: err_w,
            extra: options
                .extra_inherit
                .iter()
                .map(|h| Fd::new(h.0 as i32))
                .collect(),
        };

        let (job_startup_info, exit_fd) = sandbox.spawn_job(q)?;

        Ok(LinuxChildProcess {
            exit_code: AtomicI64::new(EXIT_CODE_STILL_RUNNING),
            stdin: in_w,
            stdout: out_r,
            stderr: err_r,
            sandbox_ref: sandbox,
            pid: job_startup_info.pid,
            fd: Some(exit_fd),
        })
    }
}

#[derive(Debug, Clone)]
pub struct UserIdBounds {
    pub low: u32,
    pub high: u32,
}

impl Default for UserIdBounds {
    fn default() -> Self {
        UserIdBounds {
            low: 100_000,
            high: 200_000,
        }
    }
}

/// Allows some customization
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct CgroupSettings {
    /// All created cgroups will be children of specified group
    /// Default value is "/minion"
    pub name_prefix: PathBuf,

    /// Overrides path to cgroupfs mount.
    /// This can be both cgroupfs v1 and cgroupfs v2.
    /// Additionally fallback (`/sys/fs/cgroup`) can be overrided
    /// at runtime using `MINION_CGROUPFS` environment variable.
    pub mount: PathBuf,
}

impl Default for CgroupSettings {
    fn default() -> Self {
        CgroupSettings {
            name_prefix: "/minion".into(),
            mount: std::env::var_os("MINION_CGROUPFS")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/sys/fs/cgroup")),
        }
    }
}

/// Resource limiting implementation
#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum ResourceDriverKind {
    /// Legacy cgroups
    CgroupV1,
    /// Unified cgroups
    CgroupV2,
    /// Auto-detected cgroups
    CgroupAuto,
    /// Use per-process limits (prlimit)
    /// This is the only option that does not require write access
    /// to cgroupfs.
    /// # Caveats
    /// [TODO]
    Prlimit,
    /// Auto-detect
    Auto { allow_dangerous: bool },
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum SeccompPolicy {
    /// Policy that allows all syscalls is be used.
    Unrestricted,
    /// Policy that disallows several possibly dangerous syscalls
    /// is used. You should use this (or more restrictive policy)
    /// when `rootless` is false.
    ///
    /// This is the default.
    DenyDangerous,
    /// Policy that disallows almost all syscalls
    Pure,
    /// Use provided policy
    Manual { policy: Vec<u8> },
}

impl Default for SeccompPolicy {
    fn default() -> Self {
        SeccompPolicy::DenyDangerous
    }
}

/// Allows some customization
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Settings {
    /// If enabled, minion will ignore clone(MOUNT_NEWNS) error.
    /// This flag has to be enabled for gVisor support.
    pub allow_unsupported_mount_namespace: bool,
    /// Cgroup settings
    pub cgroup: CgroupSettings,
    /// Resource limits drivers to use
    pub resource_drivers: Vec<ResourceDriverKind>,
    /// Do not perform actions that require root access.
    /// Note that some other options may require root as well.
    ///
    /// When set to true, `seccomp` should not be set to
    /// Unrestricted.
    pub rootless: bool,
    /// User identifiers to use for the sandboxes.
    /// Ignored in rootless mode (because calling process uid will be used
    /// instead). Also applies to GIDs
    pub uid: UserIdBounds,
    /// Seccomp settings
    pub seccomp: SeccompPolicy,
}

impl Default for Settings {
    fn default() -> Self {
        let have_root = nix::unistd::Uid::effective().is_root();
        Settings {
            allow_unsupported_mount_namespace: false,
            cgroup: CgroupSettings::default(),
            resource_drivers: vec![ResourceDriverKind::Auto {
                allow_dangerous: false,
            }],
            rootless: !have_root,
            uid: Default::default(),
            seccomp: SeccompPolicy::default(),
        }
    }
}

impl Settings {
    pub fn new() -> Settings {
        Default::default()
    }
}

/// Linux backend
/// # Sandbox extensions
/// Supported shared items flags:
///  - `recursive`: use recursive bind mounts (i.e. MS_REC)
#[derive(Debug)]
pub struct LinuxBackend {
    settings: Settings,
    driver: Arc<limits::Driver>,
    // used for allocating sandbox UIDs when we are root
    uid_alloc: Arc<UidAllocator>,
}

impl Backend for LinuxBackend {
    type Error = Error;
    type Sandbox = LinuxSandbox;
    type ChildProcess = LinuxChildProcess;
    fn new_sandbox(&self, mut options: SandboxOptions) -> Result<LinuxSandbox, Error> {
        options.postprocess();
        let sb = LinuxSandbox::create(
            options,
            &self.settings,
            self.driver.clone(),
            self.uid_alloc.clone(),
        )?;
        Ok(sb)
    }

    fn spawn(
        &self,
        options: ChildProcessOptions,
        sandbox: Arc<LinuxSandbox>,
    ) -> Result<Self::ChildProcess, Error> {
        spawn(options, sandbox)
    }
}

impl LinuxBackend {
    pub fn new(settings: Settings) -> Result<LinuxBackend, Error> {
        self::check::run_all_feature_checks();
        let driver = Arc::new(limits::Driver::new(&settings)?);
        let uid_alloc = Arc::new(UidAllocator::new(settings.uid.low, settings.uid.high));
        Ok(LinuxBackend {
            settings,
            driver,
            uid_alloc,
        })
    }
}
