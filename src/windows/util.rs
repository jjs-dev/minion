use crate::windows::{Cvt, Error};
use std::mem::ManuallyDrop;
use winapi::{
    shared::minwindef::{FALSE, TRUE},
    um::{
        handleapi::{CloseHandle, DuplicateHandle, INVALID_HANDLE_VALUE},
        processthreadsapi::GetCurrentProcess,
        winnt::{DUPLICATE_SAME_ACCESS, HANDLE},
    },
};

#[derive(Debug)]
pub struct OwnedHandle(HANDLE);

unsafe impl Send for OwnedHandle {}
unsafe impl Sync for OwnedHandle {}

impl OwnedHandle {
    pub fn new(h: HANDLE) -> Self {
        assert_ne!(h, INVALID_HANDLE_VALUE);
        OwnedHandle(h)
    }

    pub fn as_raw(&self) -> HANDLE {
        self.0
    }

    pub fn into_inner(self) -> HANDLE {
        let this = ManuallyDrop::new(self);
        this.0
    }

    pub fn read(&self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.len() > i32::max_value() as usize {
            buf = &mut buf[..(i32::max_value() as usize)]
        }
        let mut read_cnt = 0;
        let res = unsafe {
            winapi::um::fileapi::ReadFile(
                self.0,
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

    pub fn write(&self, mut buf: &[u8]) -> std::io::Result<usize> {
        if buf.len() > (i32::max_value() as usize) {
            buf = &buf[..(i32::max_value() as usize)];
        }
        let mut written_cnt = 0;
        let res = unsafe {
            winapi::um::fileapi::WriteFile(
                self.0,
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

    fn duplicate(&self, inherit: bool) -> Result<Self, Error> {
        let mut cloned_handle = std::ptr::null_mut();
        unsafe {
            Cvt::nonzero(DuplicateHandle(
                GetCurrentProcess(),
                self.as_raw(),
                GetCurrentProcess(),
                &mut cloned_handle,
                0,
                if inherit { TRUE } else { FALSE },
                DUPLICATE_SAME_ACCESS,
            ))?;
        }
        Ok(Self::new(cloned_handle))
    }

    pub fn try_clone(&self) -> Result<Self, Error> {
        self.duplicate(false)
    }

    pub fn try_clone_with_inheritance(&self) -> Result<Self, Error> {
        self.duplicate(true)
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let ret = unsafe { CloseHandle(self.0) };
        if ret == 0 {
            panic!("failed to close handle {}", self.0 as usize);
        }
    }
}
