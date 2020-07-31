use crate::windows::{Cvt, Error};
use std::os::windows::io::{FromRawHandle, IntoRawHandle, RawHandle};
use winapi::{
    shared::minwindef::TRUE,
    um::{
        handleapi::CloseHandle, minwinbase::SECURITY_ATTRIBUTES, namedpipeapi::CreatePipe,
        winnt::HANDLE,
    },
};

#[derive(Debug)]
pub struct ReadPipe {
    handle: HANDLE,
}

unsafe impl Send for ReadPipe {}
unsafe impl Sync for ReadPipe {}

impl IntoRawHandle for ReadPipe {
    fn into_raw_handle(self) -> RawHandle {
        let h = self.handle;
        std::mem::forget(self);
        h
    }
}

impl FromRawHandle for ReadPipe {
    unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        ReadPipe { handle }
    }
}

impl std::io::Read for ReadPipe {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.len() > i32::max_value() as usize {
            buf = &mut buf[..(i32::max_value() as usize)]
        }
        let mut read_cnt = 0;
        let res = unsafe {
            winapi::um::fileapi::ReadFile(
                self.handle,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read_cnt,
                std::ptr::null_mut(),
            )
        };

        if res == 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(read_cnt as usize)
    }
}

impl Drop for ReadPipe {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

#[derive(Debug)]
pub struct WritePipe {
    handle: HANDLE,
}

unsafe impl Send for WritePipe {}
unsafe impl Sync for WritePipe {}

impl IntoRawHandle for WritePipe {
    fn into_raw_handle(self) -> RawHandle {
        let h = self.handle;
        std::mem::forget(self);
        h
    }
}

impl std::io::Write for WritePipe {
    fn write(&mut self, mut buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > (i32::max_value() as usize) {
            buf = &buf[..(i32::max_value() as usize)];
        }
        let mut written_cnt = 0;
        let res = unsafe {
            winapi::um::fileapi::WriteFile(
                self.handle,
                buf.as_ptr().cast(),
                buf.len() as u32,
                &mut written_cnt,
                std::ptr::null_mut(),
            )
        };
        if res != 0 {
            Ok(written_cnt as usize)
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // no need to flush
        Ok(())
    }
}

impl Drop for WritePipe {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.handle);
        }
    }
}

pub(in crate::windows) enum InheritKind {
    Allow,
}

pub(in crate::windows) fn make(inherit: InheritKind) -> Result<(ReadPipe, WritePipe), Error> {
    let mut read = std::ptr::null_mut();
    let mut write = std::ptr::null_mut();
    unsafe {
        let mut security_attributes: SECURITY_ATTRIBUTES = std::mem::zeroed();
        security_attributes.nLength = std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32;
        if matches!(inherit, InheritKind::Allow) {
            security_attributes.bInheritHandle = TRUE;
        }
        Cvt::nonzero(CreatePipe(
            &mut read,
            &mut write,
            &mut security_attributes,
            0,
        ))?;
    }
    Ok((ReadPipe { handle: read }, WritePipe { handle: write }))
}
