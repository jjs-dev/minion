//! Implements resource limits

use std::{ffi::OsString, os::windows::ffi::OsStrExt};

use crate::{
    windows::{util::OwnedHandle, Cvt, Error},
    ResourceUsageData,
};

use winapi::um::{
    jobapi2::{
        AssignProcessToJobObject, CreateJobObjectW, QueryInformationJobObject,
        SetInformationJobObject, TerminateJobObject,
    },
    winnt::{
        JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
        JobObjectLimitViolationInformation, HANDLE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOBOBJECT_LIMIT_VIOLATION_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY, JOB_OBJECT_LIMIT_JOB_TIME,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    },
};

/// Responsible for resource isolation & adding & killing
#[derive(Debug)]
pub(crate) struct Job {
    handle: OwnedHandle,
}

impl Job {
    pub(crate) fn new(jail_id: &str) -> Result<Self, Error> {
        let name: OsString = format!("minion-sandbox-job-{}", jail_id).into();
        let name: Vec<u16> = name.encode_wide().collect();
        let handle = unsafe {
            Cvt::nonzero(CreateJobObjectW(std::ptr::null_mut(), name.as_ptr()) as i32)? as HANDLE
        };
        let handle = OwnedHandle::new(handle);
        Ok(Self { handle })
    }
    pub(crate) fn enable_resource_limits(
        &mut self,
        options: &crate::SandboxOptions,
    ) -> Result<(), Error> {
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.JobMemoryLimit = options.memory_limit as usize;
        info.BasicLimitInformation.ActiveProcessLimit = options.max_alive_process_count;
        unsafe {
            *info
                .BasicLimitInformation
                .PerJobUserTimeLimit
                .QuadPart_mut() = (options.cpu_time_limit.as_nanos() / 100) as i64;
        };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_JOB_MEMORY
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
            | JOB_OBJECT_LIMIT_JOB_TIME
            // let's make sure sandbox will die if we panic / abort
            | JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        unsafe {
            Cvt::nonzero(SetInformationJobObject(
                self.handle.as_raw(),
                JobObjectExtendedLimitInformation,
                (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                sizeof::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
            ))?;
        }
        Ok(())
    }
    pub(crate) fn kill(&self) -> Result<(), Error> {
        unsafe { Cvt::nonzero(TerminateJobObject(self.handle.as_raw(), 0xDEADBEEF)).map(|_| ()) }
    }
    pub(crate) fn add_process(&self, process_handle: HANDLE) -> Result<(), Error> {
        unsafe {
            Cvt::nonzero(AssignProcessToJobObject(
                self.handle.as_raw(),
                process_handle,
            ))
            .map(|_| ())
        }
    }
    pub(crate) fn resource_usage(&self) -> Result<crate::ResourceUsageData, Error> {
        let cpu = unsafe {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = std::mem::zeroed();
            Cvt::nonzero(QueryInformationJobObject(
                self.handle.as_raw(),
                JobObjectBasicAccountingInformation,
                (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                sizeof::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>(),
                std::ptr::null_mut(),
            ))?;

            let user_ticks = *info.TotalUserTime.QuadPart() as u64;
            let kernel_ticks = *info.TotalKernelTime.QuadPart() as u64;
            (user_ticks + kernel_ticks) * 100
        };
        let memory = unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            Cvt::nonzero(QueryInformationJobObject(
                self.handle.as_raw(),
                JobObjectExtendedLimitInformation,
                (&mut info as *mut JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                sizeof::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>(),
                std::ptr::null_mut(),
            ))?;
            info.PeakJobMemoryUsed as u64
        };

        Ok(ResourceUsageData {
            time: Some(cpu),
            memory: Some(memory),
        })
    }
    pub(crate) fn check_cpu_tle(&self) -> Result<bool, Error> {
        unsafe {
            let mut info: JOBOBJECT_LIMIT_VIOLATION_INFORMATION = std::mem::zeroed();
            Cvt::nonzero(QueryInformationJobObject(
                self.handle.as_raw(),
                JobObjectLimitViolationInformation,
                (&mut info as *mut JOBOBJECT_LIMIT_VIOLATION_INFORMATION).cast(),
                sizeof::<JOBOBJECT_LIMIT_VIOLATION_INFORMATION>(),
                std::ptr::null_mut(),
            ))?;
            let viol = info.ViolationLimitFlags & JOB_OBJECT_LIMIT_JOB_TIME;
            Ok(viol != 0)
        }
    }

    pub(crate) fn check_real_tle(&self) -> Result<bool, Error> {
        // TODO
        Ok(false)
    }
}

fn sizeof<T>() -> u32 {
    let sz = std::mem::size_of::<T>();
    assert!(sz <= (u32::max_value() as usize));
    sz as u32
}
