#![feature(try_trait)]

use minion::{self, Dominion as _};
use std::{
    ffi::{CStr, OsStr, OsString},
    mem::{self},
    os::raw::c_char,
};

#[repr(i32)]
pub enum ErrorCode {
    /// operation completed successfully
    Ok,
    /// passed arguments didn't pass some basic checks
    /// examples:
    /// - provided buffer was expected to be null-terminated utf8-encoded string, but wasn't
    /// - something was expected to be unique, but wasn't, and so on
    /// these errors usually imply bug exists in caller code
    InvalidInput,
    /// unknown error
    Unknown,
}

/// Get string description of given `error_code`, returned by minion-ffi previously.
/// Returns char const* pointer with static lifetime. This pointer must not be freed.
/// Description is guaranteed to be null-terminated ASCII string
#[no_mangle]
pub extern "C" fn minion_describe_status(error_code: ErrorCode) -> *const u8 {
    match error_code {
        ErrorCode::Ok => b"ok\0".as_ptr(),
        ErrorCode::InvalidInput => b"invalid input\0".as_ptr(),
        ErrorCode::Unknown => b"unknown error\0".as_ptr(),
    }
}

#[repr(i32)]
pub enum WaitOutcome {
    Exited,
    AlreadyFinished,
    Timeout,
}

unsafe fn get_string(buf: *const c_char) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    let buf = CStr::from_ptr(buf);
    let buf = buf.to_bytes();
    let s = OsStr::from_bytes(buf);
    s.to_os_string()
}

impl std::ops::Try for ErrorCode {
    type Error = ErrorCode;
    type Ok = ErrorCode;

    fn into_result(self) -> Result<ErrorCode, ErrorCode> {
        match self {
            ErrorCode::Ok => Ok(ErrorCode::Ok),
            oth => Err(oth),
        }
    }

    fn from_error(x: ErrorCode) -> Self {
        x
    }

    fn from_ok(x: ErrorCode) -> Self {
        x
    }
}

pub struct Backend(Box<dyn minion::Backend>);

/// # Safety
/// Must be called once
/// Must be called before any library usage
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_lib_init() -> ErrorCode {
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[minion-ffi] PANIC: {} ({:?})", &info, info);
        std::process::abort();
    }));
    ErrorCode::Ok
}

/// Create backend, default for target platform
#[no_mangle]
#[must_use]
pub extern "C" fn minion_backend_create(out: &mut *mut Backend) -> ErrorCode {
    let backend = Backend(minion::setup());
    let backend = Box::new(backend);
    *out = Box::into_raw(backend);
    ErrorCode::Ok
}

/// Drop backend
/// # Safety
/// `b` must be pointer to Backend, allocated by `minion_backend_create`
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_backend_free(b: *mut Backend) -> ErrorCode {
    let b = Box::from_raw(b);
    mem::drop(b);
    ErrorCode::Ok
}

#[repr(C)]
pub struct TimeSpec {
    pub seconds: u32,
    pub nanoseconds: u32,
}

#[repr(C)]
pub struct DominionOptions {
    pub cpu_time_limit: TimeSpec,
    pub real_time_limit: TimeSpec,
    pub process_limit: u32,
    pub memory_limit: u32,
    pub isolation_root: *const c_char,
    pub shared_directories: *const SharedDirectoryAccess,
}

#[derive(Clone)]
pub struct Dominion(minion::DominionRef);

/// # Safety
/// `out` must be valid
#[no_mangle]
pub unsafe extern "C" fn minion_dominion_check_cpu_tle(
    dominion: &Dominion,
    out: *mut bool,
) -> ErrorCode {
    match dominion.0.check_cpu_tle() {
        Ok(st) => {
            out.write(st);
            ErrorCode::Ok
        }
        Err(_) => ErrorCode::Unknown,
    }
}

/// # Safety
/// `out` must be valid
#[no_mangle]
pub unsafe extern "C" fn minion_dominion_check_real_tle(
    dominion: &Dominion,
    out: *mut bool,
) -> ErrorCode {
    match dominion.0.check_real_tle() {
        Ok(st) => {
            out.write(st);
            ErrorCode::Ok
        }
        Err(_) => ErrorCode::Unknown,
    }
}

#[no_mangle]
pub extern "C" fn minion_dominion_kill(dominion: &Dominion) -> ErrorCode {
    match dominion.0.kill() {
        Ok(_) => ErrorCode::Ok,
        Err(_) => ErrorCode::Unknown,
    }
}

/// # Safety
/// Provided arguments must be well-formed
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_dominion_create(
    backend: &Backend,
    options: DominionOptions,
    out: &mut *mut Dominion,
) -> ErrorCode {
    let mut exposed_paths = Vec::new();
    {
        let mut p = options.shared_directories;
        while !(*p).host_path.is_null() {
            let opt = minion::PathExpositionOptions {
                src: get_string((*p).host_path).into(),
                dest: get_string((*p).sandbox_path).into(),
                access: match (*p).kind {
                    SharedDirectoryAccessKind::Full => minion::DesiredAccess::Full,
                    SharedDirectoryAccessKind::Readonly => minion::DesiredAccess::Readonly,
                },
            };
            exposed_paths.push(opt);
            p = p.offset(1);
        }
    }
    let opts = minion::DominionOptions {
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
        isolation_root: get_string(options.isolation_root).into(),
        exposed_paths,
    };
    let d = backend.0.new_dominion(opts);
    let d = d.unwrap();

    let dw = Dominion(d);
    *out = Box::into_raw(Box::new(dw));
    ErrorCode::Ok
}

/// # Safety
/// `dominion` must be pointer, returned by `minion_dominion_create`.
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_dominion_free(dominion: *mut Dominion) -> ErrorCode {
    let b = Box::from_raw(dominion);
    mem::drop(b);
    ErrorCode::Ok
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
    pub dominion: *mut Dominion,
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

pub struct ChildProcess(Box<dyn minion::ChildProcess>);

/// # Safety
/// Provided `options` must be well-formed
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_spawn(
    backend: &Backend,
    options: ChildProcessOptions,
    out: &mut *mut ChildProcess,
) -> ErrorCode {
    let mut arguments = Vec::new();
    {
        let mut p = options.argv;
        while !(*p).is_null() {
            arguments.push(get_string(*p));
            p = p.offset(1);
        }
    }
    let mut environment = Vec::new();
    {
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
    let stdio = minion::StdioSpecification {
        stdin: minion::InputSpecification::handle(options.stdio.stdin),
        stdout: minion::OutputSpecification::handle(options.stdio.stdout),
        stderr: minion::OutputSpecification::handle(options.stdio.stderr),
    };
    let options = minion::ChildProcessOptions {
        path: get_string(options.image_path).into(),
        arguments,
        environment,
        dominion: (*options.dominion).0.clone(),
        stdio,
        pwd: get_string(options.workdir).into(),
    };
    let cp = backend.0.spawn(options).unwrap();
    let cp = ChildProcess(cp);
    let cp = Box::new(cp);
    *out = Box::into_raw(cp);
    ErrorCode::Ok
}

/// Wait for process exit, with timeout.
/// # Safety
/// Provided pointers must be valid
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_wait(
    cp: &mut ChildProcess,
    timeout: Option<&TimeSpec>,
    out: *mut WaitOutcome,
) -> ErrorCode {
    let ans = cp.0.wait_for_exit(
        timeout
            .map(|timeout| std::time::Duration::new(timeout.seconds.into(), timeout.nanoseconds)),
    );
    match ans {
        Result::Ok(ans) => {
            let outcome = match ans {
                minion::WaitOutcome::Exited => WaitOutcome::Exited,
                minion::WaitOutcome::AlreadyFinished => WaitOutcome::AlreadyFinished,
                minion::WaitOutcome::Timeout => WaitOutcome::Timeout,
            };
            out.write(outcome);
            ErrorCode::Ok
        }
        Result::Err(_) => ErrorCode::Unknown,
    }
}

#[no_mangle]
pub static EXIT_CODE_STILL_RUNNING: i64 = 1234_4321;

/// # Safety
/// Provided pointers must be valid
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_exitcode(
    cp: &mut ChildProcess,
    out: *mut i64,
    finish_flag: *mut bool,
) -> ErrorCode {
    match cp.0.get_exit_code() {
        Result::Ok(exit_code) => {
            if let Some(code) = exit_code {
                out.write(code);
            } else {
                out.write(EXIT_CODE_STILL_RUNNING)
            }
            if !finish_flag.is_null() {
                finish_flag.write(exit_code.is_some());
            }
            ErrorCode::Ok
        }
        Result::Err(_) => ErrorCode::Unknown,
    }
}

/// # Safety
/// `cp` must be valid pointer to ChildProcess object, allocated by `minion_cp_spawn`
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_free(cp: *mut ChildProcess) -> ErrorCode {
    mem::drop(Box::from_raw(cp));
    ErrorCode::Ok
}
