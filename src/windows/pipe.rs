use crate::windows::{util::OwnedHandle, Cvt, Error};
use std::os::windows::io::{FromRawHandle, IntoRawHandle, RawHandle};
use winapi::{
    shared::minwindef::TRUE,
    um::{minwinbase::SECURITY_ATTRIBUTES, namedpipeapi::CreatePipe},
};

#[derive(Debug)]
pub struct ReadPipe {
    handle: OwnedHandle,
}

impl IntoRawHandle for ReadPipe {
    fn into_raw_handle(self) -> RawHandle {
        self.handle.into_inner()
    }
}

impl FromRawHandle for ReadPipe {
    unsafe fn from_raw_handle(handle: RawHandle) -> Self {
        ReadPipe {
            handle: OwnedHandle::new(handle),
        }
    }
}

impl std::io::Read for ReadPipe {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.handle.read(buf)
    }
}

#[derive(Debug)]
pub struct WritePipe {
    handle: OwnedHandle,
}

impl IntoRawHandle for WritePipe {
    fn into_raw_handle(self) -> RawHandle {
        self.handle.into_inner()
    }
}

impl std::io::Write for WritePipe {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.handle.write(buf)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        // no need to flush
        Ok(())
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
    Ok((
        ReadPipe {
            handle: OwnedHandle::new(read),
        },
        WritePipe {
            handle: OwnedHandle::new(write),
        },
    ))
}
