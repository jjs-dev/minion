use crate::{
    windows::{
        pipe::{self, ReadPipe, WritePipe},
        Cvt, Error, WindowsSandbox,
    },
    InputSpecificationData, OutputSpecificationData,
};
use std::{
    ffi::{OsStr, OsString},
    mem::size_of,
    os::windows::{
        ffi::OsStrExt,
        io::{FromRawHandle, IntoRawHandle},
    },
};
use winapi::{
    shared::{minwindef::TRUE, winerror::ERROR_INSUFFICIENT_BUFFER},
    um::{
        errhandlingapi::GetLastError,
        handleapi::{CloseHandle, INVALID_HANDLE_VALUE},
        minwinbase::SECURITY_ATTRIBUTES,
        processthreadsapi::{
            CreateProcessW, DeleteProcThreadAttributeList, GetCurrentProcess,
            InitializeProcThreadAttributeList, UpdateProcThreadAttribute, PROCESS_INFORMATION,
            PROC_THREAD_ATTRIBUTE_LIST,
        },
        userenv::{CreateAppContainerProfile, DeleteAppContainerProfile},
        winbase::{
            CreateFileMappingA, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
            STARTF_USESTDHANDLES, STARTUPINFOEXW,
        },
        winnt::{HANDLE, PAGE_READWRITE, SECURITY_CAPABILITIES},
    },
};

pub(in crate::windows) struct Stdio {
    pub stdin: HANDLE,
    pub stdout: HANDLE,
    pub stderr: HANDLE,
}

impl Stdio {
    fn make_input(
        spec: crate::InputSpecificationData,
    ) -> Result<(HANDLE, Option<WritePipe>), Error> {
        match spec {
            InputSpecificationData::Handle(h) => Ok((h as HANDLE, None)),
            InputSpecificationData::Pipe => {
                let (reader, writer) = pipe::make(pipe::InheritKind::Allow)?;
                Ok((reader.into_raw_handle(), Some(writer)))
            }
            InputSpecificationData::Empty => Ok((open_empty_readable_file(), None)),
            InputSpecificationData::Null => Ok((-1_i32 as usize as HANDLE, None)),
        }
    }

    fn make_output(
        spec: crate::OutputSpecificationData,
    ) -> Result<(HANDLE, Option<ReadPipe>), Error> {
        match spec {
            OutputSpecificationData::Handle(h) => Ok((h as HANDLE, None)),
            OutputSpecificationData::Pipe => {
                let (reader, writer) = pipe::make(pipe::InheritKind::Allow)?;
                Ok((writer.into_raw_handle(), Some(reader)))
            }
            OutputSpecificationData::Null => Ok((-1_i32 as usize as HANDLE, None)),
            OutputSpecificationData::Ignore => {
                let file = std::fs::File::create("C:\\NUL").map_err(|io_err| Error::Syscall {
                    errno: io_err.raw_os_error().unwrap_or(-1) as u32,
                })?;
                let file = file.into_raw_handle();
                let cloned_file = crate::windows::util::duplicate_with_inheritance(file)?;
                unsafe {
                    CloseHandle(file);
                }
                Ok((cloned_file, None))
            }
            OutputSpecificationData::Buffer(sz) => unsafe {
                let sz = sz.unwrap_or(usize::max_value()) as u64;
                let mmap = CreateFileMappingA(
                    INVALID_HANDLE_VALUE,
                    std::ptr::null_mut(),
                    PAGE_READWRITE,
                    (sz >> 32) as u32,
                    sz as u32,
                    std::ptr::null(),
                );
                if mmap.is_null() {
                    Cvt::nonzero(0)?;
                }
                let child_side = crate::windows::util::duplicate_with_inheritance(mmap)?;
                Ok((child_side, Some(ReadPipe::from_raw_handle(mmap))))
            },
        }
    }

    pub(in crate::windows) fn make(
        params: crate::StdioSpecification,
    ) -> Result<
        (
            Self,
            (Option<WritePipe>, Option<ReadPipe>, Option<ReadPipe>),
        ),
        Error,
    > {
        let (h_stdin, p_stdin) = Self::make_input(params.stdin.0)?;
        let (h_stdout, p_stdout) = Self::make_output(params.stdout.0)?;
        let (h_stderr, p_stderr) = Self::make_output(params.stderr.0)?;
        Ok((
            Stdio {
                stdin: h_stdin,
                stdout: h_stdout,
                stderr: h_stderr,
            },
            (p_stdin, p_stdout, p_stderr),
        ))
    }
}

fn open_empty_readable_file() -> HANDLE {
    static EMPTY_FILE_HANDLE: once_cell::sync::Lazy<usize> = once_cell::sync::Lazy::new(|| {
        let (reader, writer) =
            pipe::make(pipe::InheritKind::Allow).expect("failed to create a pipe");
        drop(writer);
        reader.into_raw_handle() as usize
    });
    *EMPTY_FILE_HANDLE as HANDLE
}

pub(in crate::windows) struct ChildParams {
    pub exe: OsString,
    /// Does not contain argv[0] - will be prepended automatically.
    pub argv: Vec<OsString>,
    pub env: Vec<OsString>,
    pub cwd: OsString,
}

// TODO: upstream to winapi: https://github.com/retep998/winapi-rs/pull/933/
const MAGIC_PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES: usize = 131081;

struct AlignedMemBlock(*mut u8, usize);

impl AlignedMemBlock {
    fn layout(cnt: usize) -> std::alloc::Layout {
        assert!(cnt > 0);
        std::alloc::Layout::from_size_align(cnt, 8).unwrap()
    }

    fn new(cnt: usize) -> AlignedMemBlock {
        let ptr = unsafe { std::alloc::alloc_zeroed(Self::layout(cnt)) };
        AlignedMemBlock(ptr, cnt)
    }

    fn ptr(&self) -> *mut u8 {
        self.0
    }
}

impl Drop for AlignedMemBlock {
    fn drop(&mut self) {
        unsafe {
            std::alloc::dealloc(self.0, Self::layout(self.1));
        }
    }
}

pub(in crate::windows) fn spawn(
    sandbox: &WindowsSandbox,
    stdio: Stdio,
    params: ChildParams,
) -> Result<PROCESS_INFORMATION, Error> {
    let proc_thread_attr_list_storage;
    let mut security_capabilities;
    let mut startup_info = unsafe {
        let mut startup_info: STARTUPINFOEXW = std::mem::zeroed();
        let mut proc_thread_attr_list_len = 0;
        {
            InitializeProcThreadAttributeList(
                std::ptr::null_mut(),
                // we need only one attribute: security capabilities.
                1,
                0,
                &mut proc_thread_attr_list_len,
            );
            if GetLastError() != ERROR_INSUFFICIENT_BUFFER {
                return Err(Error::last());
            }
        }
        proc_thread_attr_list_storage = AlignedMemBlock::new(proc_thread_attr_list_len);
        let proc_thread_attr_list = proc_thread_attr_list_storage.ptr();
        startup_info.lpAttributeList = proc_thread_attr_list.cast();
        Cvt::nonzero(InitializeProcThreadAttributeList(
            startup_info.lpAttributeList,
            1,
            0,
            &mut proc_thread_attr_list_len,
        ))?;
        security_capabilities = sandbox.profile.get_security_capabilities();
        Cvt::nonzero(UpdateProcThreadAttribute(
            startup_info.lpAttributeList,
            // reserved
            0,
            MAGIC_PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            (&mut security_capabilities as *mut SECURITY_CAPABILITIES).cast(),
            std::mem::size_of::<SECURITY_ATTRIBUTES>(),
            // reserved
            std::ptr::null_mut(),
            // reserved
            std::ptr::null_mut(),
        ))?;

        startup_info.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup_info.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup_info.StartupInfo.hStdInput = stdio.stdin;
        startup_info.StartupInfo.hStdOutput = stdio.stdout;
        startup_info.StartupInfo.hStdError = stdio.stderr;
        startup_info
    };
    let creation_flags = CREATE_UNICODE_ENVIRONMENT | EXTENDED_STARTUPINFO_PRESENT;
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    unsafe {
        let application_name: Vec<u16> = params.exe.encode_wide().collect();
        let mut cmd_line = application_name.clone();
        for arg in params.argv {
            quote_arg(&mut cmd_line, &arg);
        }
        let (mut env, env_status) = encode_env(&params.env);
        if let EncodeEnvResult::Partial = env_status {
            tracing::warn!("skipped zero chars in provided environment");
        }
        let cwd: Vec<u16> = params.cwd.encode_wide().collect();
        Cvt::nonzero(CreateProcessW(
            application_name.as_ptr(),
            cmd_line.as_mut_ptr(),
            // pass null as process attributes to disallow inheritance
            std::ptr::null_mut(),
            // same for thread
            std::ptr::null_mut(),
            // inherit handles
            TRUE,
            creation_flags,
            env.as_mut_ptr().cast(),
            cwd.as_ptr(),
            (&mut startup_info as *mut STARTUPINFOEXW).cast(),
            &mut info,
        ))?;
        DeleteProcThreadAttributeList(startup_info.lpAttributeList);
    }
    Ok(info)
}

fn ascii_to_u16(ch: u8) -> u16 {
    let ch = ch as char;
    let mut out: u16 = 0;
    ch.encode_utf16(std::slice::from_mut(&mut out));
    out
}

fn quote_arg(out: &mut Vec<u16>, data: &OsStr) {
    // FIXME incorrectly handles quotes.
    out.push(ascii_to_u16(b' '));
    out.push(ascii_to_u16(b'"'));
    for ch in data.encode_wide() {
        assert_ne!(ch, ascii_to_u16(b'"'));
        out.push(ch);
    }

    out.push(ascii_to_u16(b'"'));
}

#[derive(Eq, PartialEq)]
enum EncodeEnvResult {
    /// Success
    Ok,
    /// Partial success: zero chars were skipped
    Partial,
}

/// Returns None if data contains zero char.
fn encode_env(data: &[OsString]) -> (Vec<u16>, EncodeEnvResult) {
    let mut res = EncodeEnvResult::Ok;
    let mut capacity = 1;
    for item in data {
        capacity += item.encode_wide().count() + 1;
    }
    let mut out = Vec::with_capacity(capacity);
    for item in data {
        for char in item.encode_wide() {
            if char == 0 {
                res = EncodeEnvResult::Partial;
                continue;
            }
            out.push(char);
        }
        out.push(0);
    }
    out.push(0);

    // let's verify capacity was correct
    debug_assert_eq!(out.capacity(), capacity);
    if res == EncodeEnvResult::Ok {
        debug_assert_eq!(out.len(), capacity);
    }
    (out, res)
}
