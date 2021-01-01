#![cfg_attr(minion_nightly, feature(unsafe_block_in_unsafe_fn))]
#![cfg_attr(minion_nightly, warn(unsafe_op_in_unsafe_fn))]
// use minion::{self};
use std::{
    alloc::GlobalAlloc,
    ffi::{CStr, OsStr, OsString},
    mem::{self},
    os::raw::{c_char, c_void},
};

static mut CAPTURE_ERRORS: bool = false;

/// Operation status.
/// It consists of status code and
/// optionally details.
#[repr(C)]
pub struct Status {
    pub code: StatusCode,
    // owned anyhow::Error
    // TODO: can this be safer?
    /// If not NULL, then additional details are available.
    /// Use `minion_status_*` functions to inspect then.
    pub details: *mut c_void,
}

impl Status {
    fn from_code(c: StatusCode) -> Self {
        assert_ne!(c, StatusCode::Minion);
        Status {
            code: c,
            details: std::ptr::null_mut(),
        }
    }

    fn from_err(err: anyhow::Error) -> Self {
        Status {
            code: StatusCode::Minion,
            details: unsafe {
                if CAPTURE_ERRORS {
                    std::mem::transmute(err)
                } else {
                    std::ptr::null_mut()
                }
            },
        }
    }

    fn err(&self) -> Option<&anyhow::Error> {
        if self.details.is_null() {
            return None;
        }
        unsafe { Some(&*(&self.details as *const *mut c_void as *const anyhow::Error)) }
    }
}

#[repr(i32)]
#[derive(PartialEq, Eq, Debug)]
pub enum StatusCode {
    /// operation completed successfully
    Ok,
    /// passed arguments didn't pass some basic checks
    /// examples:
    /// - provided buffer was expected to be null-terminated utf8-encoded string, but wasn't
    /// - something was expected to be unique, but wasn't, and so on
    /// these errors usually imply bug exists in caller code
    InvalidInput,
    /// Minion error
    Minion,
}

/// Get string description of given `error_code`, returned by minion-ffi previously.
/// Returns char const* pointer with static lifetime. This pointer must not be freed.
/// Description is guaranteed to be null-terminated ASCII string
#[no_mangle]
pub extern "C" fn minion_describe_status_code(status_code: StatusCode) -> *const u8 {
    match status_code {
        StatusCode::Ok => b"ok\0".as_ptr(),
        StatusCode::InvalidInput => b"invalid input\0".as_ptr(),
        StatusCode::Minion => b"minion error\0".as_ptr(),
    }
}

/// Get string message for a given Status. If `details` is null, nullptr is returned.
/// Otherwise this function allocated and returned null-terminated string using malloc.
/// Use `free` to deallocate returned string
#[no_mangle]
pub extern "C" fn minion_status_get_message(status: &Status) -> *const u8 {
    let err = match status.err() {
        Some(e) => e,
        None => return std::ptr::null(),
    };
    let message = format!("{:#}", err);
    unsafe {
        let buf = std::alloc::System
            .alloc(std::alloc::Layout::from_size_align(message.len() + 1, 1).unwrap());
        std::ptr::copy(message.as_ptr(), buf, message.len());
        buf.add(message.len()).write(0);
        buf
    }
}

#[repr(i32)]
pub enum WaitOutcome {
    Exited,
    Timeout,
}

/// # Safety
/// `buf` must be valid, readable pointer
unsafe fn get_string(buf: *const c_char) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    let buf = unsafe { CStr::from_ptr(buf) };
    let buf = buf.to_bytes();
    let s = OsStr::from_bytes(buf);
    s.to_os_string()
}

pub struct Backend(Box<dyn minion::erased::Backend>);

/// # Safety
/// Must be called once
/// Must be called before any library usage
/// If `capture_errors` is true, returned `Status` objects are allowed
/// to contain details object. Take care to call `minion_status_free` on all
/// `Status`-es to prevent memory leaks.
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_lib_init(capture_errors: bool) -> Status {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[minion-ffi] PANIC: {} ({:?})", &info, info);
        std::process::abort();
    }));
    unsafe {
        CAPTURE_ERRORS = capture_errors;
    }
    Status::from_code(StatusCode::Ok)
}

/// Create backend, default for target platform
#[no_mangle]
#[must_use]
pub extern "C" fn minion_backend_create(out: &mut *mut Backend) -> Status {
    let backend = match minion::erased::setup() {
        Ok(b) => b,
        Err(err) => return Status::from_err(err),
    };
    let backend = Backend(backend);
    let backend = Box::new(backend);
    *out = Box::into_raw(backend);
    Status::from_code(StatusCode::Ok)
}

/// Drop backend
/// # Safety
/// `b` must be pointer to Backend, allocated by `minion_backend_create`
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_backend_free(b: *mut Backend) -> Status {
    let b = unsafe { Box::from_raw(b) };
    mem::drop(b);
    Status::from_code(StatusCode::Ok)
}

#[repr(C)]
pub struct TimeSpec {
    pub seconds: u32,
    pub nanoseconds: u32,
}

#[repr(C)]
pub struct SandboxOptions {
    pub cpu_time_limit: TimeSpec,
    pub real_time_limit: TimeSpec,
    pub process_limit: u32,
    pub memory_limit: u32,
    pub isolation_root: *const c_char,
    pub shared_directories: *const SharedDirectoryAccess,
}

#[derive(Clone)]
pub struct Sandbox(Box<dyn minion::erased::Sandbox>);

/// # Safety
/// `out` must be valid
#[no_mangle]
pub unsafe extern "C" fn minion_sandbox_check_cpu_tle(sandbox: &Sandbox, out: *mut bool) -> Status {
    match sandbox.0.check_cpu_tle() {
        Ok(st) => {
            unsafe {
                out.write(st);
            }
            Status::from_code(StatusCode::Ok)
        }
        Err(err) => Status::from_err(err),
    }
}

/// # Safety
/// `out` must be valid
#[no_mangle]
pub unsafe extern "C" fn minion_sandbox_check_real_tle(
    sandbox: &Sandbox,
    out: *mut bool,
) -> Status {
    match sandbox.0.check_real_tle() {
        Ok(st) => {
            unsafe {
                out.write(st);
            }
            Status::from_code(StatusCode::Ok)
        }
        Err(err) => Status::from_err(err),
    }
}

#[no_mangle]
pub extern "C" fn minion_sandbox_kill(sandbox: &Sandbox) -> Status {
    match sandbox.0.kill() {
        Ok(_) => Status::from_code(StatusCode::Ok),
        Err(err) => Status::from_err(err),
    }
}

/// # Safety
/// Provided arguments must be well-formed
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_sandbox_create(
    backend: &Backend,
    options: SandboxOptions,
    out: &mut *mut Sandbox,
) -> Status {
    let mut exposed_paths = Vec::new();
    unsafe {
        let mut p = options.shared_directories;
        while !(*p).host_path.is_null() {
            let opt = minion::SharedDir {
                src: get_string((*p).host_path).into(),
                dest: get_string((*p).sandbox_path).into(),
                kind: match (*p).kind {
                    SharedDirectoryAccessKind::Full => minion::SharedDirKind::Full,
                    SharedDirectoryAccessKind::Readonly => minion::SharedDirKind::Readonly,
                },
            };
            exposed_paths.push(opt);
            p = p.offset(1);
        }
    }
    let isolation_root = unsafe { get_string(options.isolation_root) }.into();
    let opts = minion::SandboxOptions {
        max_alive_process_count: options.process_limit as _,
        memory_limit: u64::from(options.memory_limit),
        cpu_time_limit: std::time::Duration::new(
            options.cpu_time_limit.seconds.into(),
            options.cpu_time_limit.nanoseconds,
        ),
        real_time_limit: std::time::Duration::new(
            options.real_time_limit.seconds.into(),
            options.real_time_limit.nanoseconds,
        ),
        isolation_root,
        exposed_paths,
    };
    let d = backend.0.new_sandbox(opts);
    let d = d.unwrap();

    let dw = Sandbox(d);
    *out = Box::into_raw(Box::new(dw));
    Status::from_code(StatusCode::Ok)
}

/// # Safety
/// `sandbox` must be pointer, returned by `minion_sandbox_create`.
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_sandbox_free(sandbox: *mut Sandbox) -> Status {
    let b = unsafe { Box::from_raw(sandbox) };
    mem::drop(b);
    Status::from_code(StatusCode::Ok)
}

#[repr(C)]
pub struct EnvItem {
    pub name: *const c_char,
    pub value: *const c_char,
}

// minion-ffi will never modify nave or value, so no races can occur
unsafe impl Sync for EnvItem {}

#[no_mangle]
pub static ENV_ITEM_FIN: EnvItem = EnvItem {
    name: std::ptr::null(),
    value: std::ptr::null(),
};

#[repr(C)]
pub enum StdioMember {
    Stdin,
    Stdout,
    Stderr,
}

#[repr(C)]
pub struct StdioHandleSet {
    pub stdin: u64,
    pub stdout: u64,
    pub stderr: u64,
}

#[repr(C)]
pub struct ChildProcessOptions {
    pub image_path: *const c_char,
    pub argv: *const *const c_char,
    pub envp: *const EnvItem,
    pub stdio: StdioHandleSet,
    pub sandbox: *mut Sandbox,
    pub workdir: *const c_char,
}

#[repr(C)]
pub enum SharedDirectoryAccessKind {
    Full,
    Readonly,
}

#[repr(C)]
pub struct SharedDirectoryAccess {
    pub kind: SharedDirectoryAccessKind,
    pub host_path: *const c_char,
    pub sandbox_path: *const c_char,
}

// minion-ffi will never modify host_path or sandbox_path, so no races can occur
unsafe impl Sync for SharedDirectoryAccess {}

#[no_mangle]
pub static SHARED_DIRECTORY_ACCESS_FIN: SharedDirectoryAccess = SharedDirectoryAccess {
    kind: SharedDirectoryAccessKind::Full, //doesn't matter
    host_path: std::ptr::null(),
    sandbox_path: std::ptr::null(),
};

pub struct ChildProcess(Box<dyn minion::erased::ChildProcess>);

/// # Safety
/// Provided `options` must be well-formed
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_spawn(
    backend: &Backend,
    options: ChildProcessOptions,
    out: &mut *mut ChildProcess,
) -> Status {
    let mut arguments = Vec::new();
    unsafe {
        let mut p = options.argv;
        while !(*p).is_null() {
            arguments.push(get_string(*p));
            p = p.offset(1);
        }
    }
    let mut environment = Vec::new();
    unsafe {
        let mut p = options.envp;
        while !(*p).name.is_null() {
            let name = get_string((*p).name);
            let value = get_string((*p).value);
            // TODO check for duplicated names
            let mut t = name;
            t.push("=");
            t.push(value);
            environment.push(t);
            p = p.offset(1);
        }
    }
    let stdio = unsafe {
        minion::StdioSpecification {
            stdin: minion::InputSpecification::handle(options.stdio.stdin),
            stdout: minion::OutputSpecification::handle(options.stdio.stdout),
            stderr: minion::OutputSpecification::handle(options.stdio.stderr),
        }
    };
    let options = unsafe {
        minion::ChildProcessOptions {
            path: get_string(options.image_path).into(),
            arguments,
            environment,
            sandbox: (*options.sandbox).0.clone(),
            stdio,
            pwd: get_string(options.workdir).into(),
        }
    };
    let cp = backend.0.spawn(options).unwrap();
    let cp = ChildProcess(cp);
    let cp = Box::new(cp);
    *out = Box::into_raw(cp);
    Status::from_code(StatusCode::Ok)
}

/// Wait for process exit, with timeout.
/// # Safety
/// Provided pointers must be valid
#[no_mangle]
#[must_use]
// TODO: async counterpart
pub unsafe extern "C" fn minion_cp_wait(
    cp: &mut ChildProcess,
    timeout: Option<&TimeSpec>,
    out: *mut WaitOutcome,
) -> Status {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start an async runtime");
    let timeout = timeout
        .map(|timeout| std::time::Duration::new(timeout.seconds.into(), timeout.nanoseconds));
    let fut = match cp.0.wait_for_exit() {
        Ok(fut) => fut,
        Err(err) => return Status::from_err(err),
    };
    let outcome = if let Some(timeout) = timeout {
        match rt.block_on(tokio::time::timeout(timeout, fut)) {
            Ok(Ok(_)) => WaitOutcome::Exited,
            Ok(Err(err)) => {
                return Status::from_err(err);
            }
            Err(_elapsed) => WaitOutcome::Timeout,
        }
    } else {
        match rt.block_on(fut) {
            Ok(_) => WaitOutcome::Exited,
            Err(err) => return Status::from_err(err),
        }
    };
    unsafe {
        out.write(outcome);
    }
    Status::from_code(StatusCode::Ok)
}

/// # Safety
/// `cp` must be valid pointer to ChildProcess object, allocated by `minion_cp_spawn`
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_free(cp: *mut ChildProcess) -> Status {
    mem::drop(unsafe { Box::from_raw(cp) });
    Status::from_code(StatusCode::Ok)
}
