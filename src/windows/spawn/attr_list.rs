//! RAII wrapper for ProcThreadAttrList
use std::marker::PhantomData;

use crate::windows::{Cvt, Error};
use winapi::{
    shared::winerror::ERROR_INSUFFICIENT_BUFFER,
    um::{
        errhandlingapi::GetLastError,
        processthreadsapi::{
            DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
            UpdateProcThreadAttribute, PROC_THREAD_ATTRIBUTE_LIST,
        },
    },
};

pub struct AttrList<'a> {
    storage: AlignedMemBlock,
    cap: usize,
    len: usize,
    phantom: PhantomData<&'a mut &'a mut ()>,
}

impl<'a> AttrList<'a> {
    pub fn new(capacity: usize) -> Result<Self, Error> {
        let mut proc_thread_attr_list_len = 0;
        unsafe {
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
        let storage = AlignedMemBlock::new(proc_thread_attr_list_len);
        unsafe {
            Cvt::nonzero(InitializeProcThreadAttributeList(
                storage.ptr().cast(),
                1,
                0,
                &mut proc_thread_attr_list_len,
            ))?;
        }

        Ok(AttrList {
            storage,
            cap: capacity,
            len: 0,
            phantom: PhantomData,
        })
    }

    pub fn add_attr<T>(&mut self, attr_name: usize, attr_val: &'a mut T) -> Result<(), Error> {
        assert!(self.len < self.cap);
        unsafe {
            Cvt::nonzero(UpdateProcThreadAttribute(
                self.storage.ptr().cast(),
                // reserved
                0,
                attr_name,
                (attr_val as *mut T).cast(),
                std::mem::size_of::<T>(),
                // reserved
                std::ptr::null_mut(),
                // reserved
                std::ptr::null_mut(),
            ))?;
        }
        self.len += 1;
        Ok(())
    }

    pub fn borrow_ptr(&self) -> *mut PROC_THREAD_ATTRIBUTE_LIST {
        assert_eq!(self.len, self.cap);
        self.storage.ptr().cast()
    }
}

impl<'a> Drop for AttrList<'a> {
    fn drop(&mut self) {
        unsafe { DeleteProcThreadAttributeList(self.storage.ptr().cast()) };
    }
}

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
