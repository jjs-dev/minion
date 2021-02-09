//! Implements security restrictions
use crate::windows::{Cvt, Error};
use std::{
    ffi::OsString,
    os::windows::ffi::{OsStrExt, OsStringExt},
};
use tracing::instrument;
use winapi::{
    shared::sddl::ConvertSidToStringSidW,
    um::{
        securitybaseapi::FreeSid,
        userenv::{CreateAppContainerProfile, DeleteAppContainerProfile},
        winbase::LocalFree,
        winnt::{PSID, SECURITY_CAPABILITIES},
    },
};

/// Represents one AppContainer
#[derive(Debug)]
pub(crate) struct Profile {
    /// Pointer to the Security Identifier (SID) of this container
    sid: PSID,
    /// Profile name (used in destructor)
    profile_name: Vec<u16>,
}

unsafe impl Send for Profile {}
unsafe impl Sync for Profile {}

impl Profile {
    /// Creates new profile. Takes new, unique sandbox_id.
    #[instrument(skip(sandbox_id))]
    pub(crate) fn new(sandbox_id: &str) -> Result<Profile, Error> {
        tracing::info!(sandbox_id, "creating profile");
        let profile_name = OsString::from(format!("minion-sandbox-appcontainer-{}", sandbox_id));
        let mut profile_name: Vec<u16> = profile_name.encode_wide().collect();
        profile_name.push(0);
        let mut sid = std::ptr::null_mut();
        unsafe {
            let mut sid_string_repr = std::ptr::null_mut();
            let res = ConvertSidToStringSidW(sid, &mut sid_string_repr);
            if res != 0 {
                let mut cnt = 0;
                while *(sid_string_repr.add(cnt)) == 0 {
                    cnt += 1;
                }
                let repr = OsString::from_wide(std::slice::from_raw_parts(sid_string_repr, cnt));
                let repr = repr.to_string_lossy();
                tracing::info!(sid=%repr, "obtained SID");
                LocalFree(sid_string_repr.cast());
            }
        }
        unsafe {
            Cvt::hresult(CreateAppContainerProfile(
                profile_name.as_ptr(),
                profile_name.as_ptr(),
                profile_name.as_ptr(),
                std::ptr::null_mut(),
                0,
                &mut sid,
            ))?;
        }
        Ok(Profile { sid, profile_name })
    }
    /// Returns `SECURITY_CAPABILITIES` representing this container.
    #[instrument(skip(self))]
    pub(crate) fn get_security_capabilities(&self) -> SECURITY_CAPABILITIES {
        let mut caps: SECURITY_CAPABILITIES = unsafe { std::mem::zeroed() };
        caps.CapabilityCount = 0;
        caps.AppContainerSid = self.sid;
        caps
    }
}

impl Drop for Profile {
    fn drop(&mut self) {
        unsafe {
            FreeSid(self.sid);
            if let Err(e) = Cvt::hresult(DeleteAppContainerProfile(self.profile_name.as_ptr())) {
                tracing::warn!(error = %e, "Ignoring resource cleanup error");
            }
        }
    }
}
