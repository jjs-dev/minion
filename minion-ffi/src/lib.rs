#![warn(unsafe_op_in_unsafe_fn)]
use minion::{self};
use std::{
    ffi::{CStr, OsStr, OsString},
    mem::{self},
    os::raw::c_char,
    sync::Arc,
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
    /// Minion error
    Minion,
}

/// Get string description of given `error_code`, returned by minion-ffi previously.
/// Returns char const* pointer with static lifetime. This pointer must not be freed.
/// Description is guaranteed to be null-terminated ASCII string
#[no_mangle]
pub extern "C" fn minion_describe_status(error_code: ErrorCode) -> *const u8 {
    match error_code {
        ErrorCode::Ok => b"ok\0".as_ptr(),
        ErrorCode::InvalidInput => b"invalid input\0".as_ptr(),
        ErrorCode::Minion => b"minion error\0".as_ptr(),
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

unsafe fn get_string_list(mut buf: *const *const c_char) -> Vec<String> {
    if buf.is_null() {
        return Vec::new();
    }
    let mut res = Vec::new();
    while unsafe { !(*buf).is_null() } {
        let s = unsafe { CStr::from_ptr(*buf) };
        res.push(s.to_str().expect("non-utf8 string received").to_string());

        buf = unsafe { buf.add(1) };
    }

    res
}

pub struct Backend(Box<dyn minion::erased::Backend>);

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
    let backend = match minion::erased::setup() {
        Ok(b) => b,
        Err(_) => return ErrorCode::Minion,
    };
    let backend = Backend(backend);
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
    let b = unsafe { Box::from_raw(b) };
    mem::drop(b);
    ErrorCode::Ok
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
    pub shared_items: *const SharedItem,
}

#[derive(Clone)]
pub struct Sandbox(Arc<dyn minion::erased::Sandbox>);

/// # Safety
/// `out` must be valid
#[no_mangle]
pub unsafe extern "C" fn minion_sandbox_check_cpu_tle(
    sandbox: &Sandbox,
    out: *mut bool,
) -> ErrorCode {
    match sandbox.0.check_cpu_tle() {
        Ok(st) => {
            unsafe {
                out.write(st);
            }
            ErrorCode::Ok
        }
        Err(_) => ErrorCode::Minion,
    }
}

/// # Safety
/// `out` must be valid
#[no_mangle]
pub unsafe extern "C" fn minion_sandbox_check_real_tle(
    sandbox: &Sandbox,
    out: *mut bool,
) -> ErrorCode {
    match sandbox.0.check_real_tle() {
        Ok(st) => {
            unsafe {
                out.write(st);
            }
            ErrorCode::Ok
        }
        Err(_) => ErrorCode::Minion,
    }
}

#[no_mangle]
pub extern "C" fn minion_sandbox_kill(sandbox: &Sandbox) -> ErrorCode {
    match sandbox.0.kill() {
        Ok(_) => ErrorCode::Ok,
        Err(_) => ErrorCode::Minion,
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
) -> ErrorCode {
    let mut shared_items = Vec::new();
    unsafe {
        let mut p = options.shared_items;
        while !(*p).host_path.is_null() {
            let opt = minion::SharedItem {
                id: None,
                src: get_string((*p).host_path).into(),
                dest: get_string((*p).sandbox_path).into(),
                flags: get_string_list((*p).flags),
                kind: match (*p).kind {
                    SharedItemAccessKind::Full => minion::SharedItemKind::Full,
                    SharedItemAccessKind::Readonly => minion::SharedItemKind::Readonly,
                },
            };
            shared_items.push(opt);
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
        shared_items,
    };
    let d = backend.0.new_sandbox(opts);
    let d = d.unwrap();

    let dw = Sandbox(d);
    *out = Box::into_raw(Box::new(dw));
    ErrorCode::Ok
}

/// # Safety
/// `sandbox` must be pointer, returned by `minion_sandbox_create`.
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_sandbox_free(sandbox: *mut Sandbox) -> ErrorCode {
    let b = unsafe { Box::from_raw(sandbox) };
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
    pub sandbox: *mut Sandbox,
    pub workdir: *const c_char,
}

#[repr(C)]
pub enum SharedItemAccessKind {
    Full,
    Readonly,
}

#[repr(C)]
pub struct SharedItem {
    pub kind: SharedItemAccessKind,
    pub host_path: *const c_char,
    pub sandbox_path: *const c_char,
    /// Mount flags.
    /// This nullable pointer should point to null-terminated
    /// array of null-terminated utf-8 strings
    pub flags: *const *const c_char,
}

// minion-ffi will never modify host_path or sandbox_path, so no races can occur
unsafe impl Sync for SharedItem {}

#[no_mangle]
pub static SHARED_DIRECTORY_ACCESS_FIN: SharedItem = SharedItem {
    kind: SharedItemAccessKind::Full, //doesn't matter
    host_path: std::ptr::null(),
    sandbox_path: std::ptr::null(),
    flags: std::ptr::null(),
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
) -> ErrorCode {
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
    let stdio = minion::StdioSpecification {
        stdin: minion::InputSpecification::handle(minion::Handle::new(options.stdio.stdin)),
        stdout: minion::OutputSpecification::handle(minion::Handle::new(options.stdio.stdout)),
        stderr: minion::OutputSpecification::handle(minion::Handle::new(options.stdio.stderr)),
    };
    let sandbox = unsafe { (*options.sandbox).0.clone() };
    let options = unsafe {
        minion::ChildProcessOptions {
            path: get_string(options.image_path).into(),
            arguments,
            environment,
            stdio,
            pwd: get_string(options.workdir).into(),
        }
    };
    let cp = backend.0.spawn(options, sandbox).unwrap();
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
// TODO: async counterpart
pub unsafe extern "C" fn minion_cp_wait(
    cp: &mut ChildProcess,
    timeout: Option<&TimeSpec>,
    out: *mut WaitOutcome,
) -> ErrorCode {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to start an async runtime");
    let timeout = timeout
        .map(|timeout| std::time::Duration::new(timeout.seconds.into(), timeout.nanoseconds));
    let fut = match cp.0.wait_for_exit() {
        Ok(fut) => fut,
        Err(_) => return ErrorCode::Minion,
    };
    let outcome = if let Some(timeout) = timeout {
        match rt.block_on(tokio::time::timeout(timeout, fut)) {
            Ok(res) => {
                if res.is_err() {
                    return ErrorCode::Minion;
                }
                WaitOutcome::Exited
            }
            Err(_elapsed) => WaitOutcome::Timeout,
        }
    } else {
        match rt.block_on(fut) {
            Ok(_) => WaitOutcome::Exited,
            Err(_) => return ErrorCode::Minion,
        }
    };
    unsafe {
        out.write(outcome);
    }
    ErrorCode::Ok
}

/// # Safety
/// `cp` must be valid pointer to ChildProcess object, allocated by `minion_cp_spawn`
#[no_mangle]
#[must_use]
pub unsafe extern "C" fn minion_cp_free(cp: *mut ChildProcess) -> ErrorCode {
    mem::drop(unsafe { Box::from_raw(cp) });
    ErrorCode::Ok
}
