use winapi::{
    shared::minwindef::TRUE,
    um::{
        handleapi::DuplicateHandle,
        processthreadsapi::GetCurrentProcess,
        winnt::{DUPLICATE_SAME_ACCESS, HANDLE},
    },
};

use crate::windows::{Cvt, Error};

pub(in crate::windows) fn duplicate_with_inheritance(handle: HANDLE) -> Result<HANDLE, Error> {
    let mut cloned_handle = std::ptr::null_mut();
    unsafe {
        Cvt::nonzero(DuplicateHandle(
            GetCurrentProcess(),
            handle,
            GetCurrentProcess(),
            &mut cloned_handle,
            0,
            TRUE,
            DUPLICATE_SAME_ACCESS,
        ))?;
    }
    Ok(cloned_handle)
}
