mod cgroup;
pub mod check;
pub mod error;
pub mod ext;
mod jail_common;
mod pipe;
mod sandbox;
mod util;
mod zygote;

use crate::{
    linux::{
        pipe::{LinuxReadPipe, LinuxWritePipe},
        util::{get_last_error, Fd, Pid},
    },
    Backend, ChildProcess, ChildProcessOptions, InputSpecification, InputSpecificationData,
    OutputSpecification, OutputSpecificationData, SandboxOptions, WaitOutcome,
};
pub use error::Error;
use nix::sys::memfd;
pub use sandbox::LinuxSandbox;
use std::{
    ffi::CString,
    fs,
    os::unix::io::IntoRawFd,
    path::PathBuf,
    sync::atomic::{AtomicI64, Ordering},
    sync::Arc,
    time::Duration,
};

pub type LinuxHandle = libc::c_int;
pub struct LinuxChildProcess {
    exit_code: AtomicI64,

    stdin: Option<LinuxWritePipe>,
    stdout: Option<LinuxReadPipe>,
    stderr: Option<LinuxReadPipe>,
    sandbox_ref: LinuxSandbox,

    pid: Pid,
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
    fn get_exit_code(&self) -> Result<Option<i64>, Error> {
        self.poll()?;
        let ec = self.exit_code.load(Ordering::SeqCst);
        let ec = match ec {
            EXIT_CODE_STILL_RUNNING => None,
            w => Some(w),
        };
        Ok(ec)
    }

    fn stdin(&mut self) -> Option<LinuxWritePipe> {
        self.stdin.take()
    }

    fn stdout(&mut self) -> Option<LinuxReadPipe> {
        self.stdout.take()
    }

    fn stderr(&mut self) -> Option<LinuxReadPipe> {
        self.stderr.take()
    }

    fn wait_for_exit(&self, timeout: Option<std::time::Duration>) -> Result<WaitOutcome, Error> {
        if self.exit_code.load(Ordering::SeqCst) != EXIT_CODE_STILL_RUNNING {
            return Ok(WaitOutcome::AlreadyFinished);
        }
        let wait_result = unsafe { self.sandbox_ref.poll_job(self.pid, timeout) };
        match wait_result {
            None => Ok(WaitOutcome::Timeout),
            Some(w) => {
                self.exit_code.store(w, Ordering::SeqCst);
                Ok(WaitOutcome::Exited)
            }
        }
    }

    fn poll(&self) -> Result<(), Error> {
        self.wait_for_exit(Some(Duration::from_nanos(1)))
            .map(|_w| ())
    }

    fn is_finished(&self) -> Result<bool, Error> {
        self.poll()?;
        Ok(self.exit_code.load(Ordering::SeqCst) != EXIT_CODE_STILL_RUNNING)
    }
}

fn handle_input_io(spec: InputSpecification) -> Result<(Option<Fd>, Fd), Error> {
    match spec.0 {
        InputSpecificationData::Pipe => {
            let mut h_read = 0;
            let mut h_write = 0;
            pipe::setup_pipe(&mut h_read, &mut h_write)?;
            let f = unsafe { libc::dup(h_read) };
            unsafe { libc::close(h_read) };
            Ok((Some(h_write), f))
        }
        InputSpecificationData::Handle(rh) => {
            let h = rh as Fd;
            Ok((None, h))
        }
        InputSpecificationData::Empty => {
            let file = fs::File::create("/dev/null")?;
            let file = file.into_raw_fd();
            Ok((None, file))
        }
        InputSpecificationData::Null => Ok((None, -1)),
    }
}

fn handle_output_io(spec: OutputSpecification) -> Result<(Option<Fd>, Fd), Error> {
    match spec.0 {
        OutputSpecificationData::Null => Ok((None, -1)),
        OutputSpecificationData::Handle(rh) => Ok((None, rh as Fd)),
        OutputSpecificationData::Pipe => {
            let mut h_read = 0;
            let mut h_write = 0;
            pipe::setup_pipe(&mut h_read, &mut h_write)?;
            let f = unsafe { libc::dup(h_write) };
            unsafe { libc::close(h_write) };
            Ok((Some(h_read), f))
        }
        OutputSpecificationData::Ignore => {
            let file = fs::File::open("/dev/null")?;
            let file = file.into_raw_fd();
            let fd = unsafe { libc::dup(file) };
            Ok((None, fd))
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
            let child_fd = unsafe { libc::dup(mfd) };
            Ok((Some(mfd), child_fd))
        }
    }
}

fn spawn(mut options: ChildProcessOptions<LinuxSandbox>) -> Result<LinuxChildProcess, Error> {
    unsafe {
        let q = jail_common::JobQuery {
            image_path: options.path.clone(),
            argv: options.arguments.clone(),
            environment: std::mem::take(&mut options.environment)
                .into_iter()
                .collect(),
            pwd: options.pwd.clone(),
        };

        let (in_w, in_r) = handle_input_io(options.stdio.stdin)?;
        let (out_r, out_w) = handle_output_io(options.stdio.stdout)?;
        let (err_r, err_w) = handle_output_io(options.stdio.stderr)?;

        let q = sandbox::ExtendedJobQuery {
            job_query: q,

            stdin: in_r,
            stdout: out_w,
            stderr: err_w,
        };

        let spawn_result = options.sandbox.spawn_job(q);

        // cleanup child stdio now
        libc::close(in_r);
        libc::close(out_w);
        libc::close(err_w);

        let ret = match spawn_result {
            Some(x) => x,
            None => return Err(error::Error::Sandbox),
        };

        let mut stdin = None;
        if let Some(h) = in_w {
            stdin.replace(LinuxWritePipe::new(h));
        }

        let process = |maybe_handle, out: &mut Option<LinuxReadPipe>| {
            if let Some(h) = maybe_handle {
                out.replace(LinuxReadPipe::new(h));
            }
        };

        let mut stdout = None;
        let mut stderr = None;

        process(out_r, &mut stdout);
        process(err_r, &mut stderr);

        Ok(LinuxChildProcess {
            exit_code: AtomicI64::new(EXIT_CODE_STILL_RUNNING),
            stdin,
            stdout,
            stderr,
            sandbox_ref: options.sandbox,
            pid: ret.pid,
        })
    }
}

/// Allows some customization
#[non_exhaustive]
#[derive(Debug)]
pub struct Settings {
    /// All created cgroups will be children of specified group
    /// Default value is "/minion"
    pub cgroup_prefix: PathBuf,

    /// If enabled, minion will ignore clone(MOUNT_NEWNS) error.
    /// This flag has to be enabled for gVisor support.
    pub allow_unsupported_mount_namespace: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            cgroup_prefix: "/minion".into(),
            allow_unsupported_mount_namespace: false,
        }
    }
}

impl Settings {
    pub fn new() -> Settings {
        Default::default()
    }
}
#[derive(Debug)]
pub struct LinuxBackend {
    settings: Settings,
    cgroup_driver: Arc<cgroup::Driver>,
}

impl Backend for LinuxBackend {
    type Error = Error;
    type Sandbox = LinuxSandbox;
    type ChildProcess = LinuxChildProcess;
    fn new_sandbox(&self, mut options: SandboxOptions) -> Result<LinuxSandbox, Error> {
        options.postprocess();
        let sb =
            unsafe { LinuxSandbox::create(options, &self.settings, self.cgroup_driver.clone())? };
        Ok(sb)
    }

    fn spawn(
        &self,
        options: ChildProcessOptions<LinuxSandbox>,
    ) -> Result<Self::ChildProcess, Error> {
        spawn(options)
    }
}

impl LinuxBackend {
    pub fn new(settings: Settings) -> Result<LinuxBackend, Error> {
        let cgroup_driver = Arc::new(cgroup::Driver::new(&settings)?);
        Ok(LinuxBackend {
            settings,
            cgroup_driver,
        })
    }
}
