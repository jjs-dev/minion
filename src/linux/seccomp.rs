#[rustfmt::skip]
mod gen;

use crate::linux::{util::get_last_error, SeccompPolicy};
use serde::{Deserialize, Serialize};
use std::{convert::TryInto, marker::PhantomData};

#[derive(Clone, Serialize, Deserialize)]
pub(in crate::linux) struct Seccomp(Vec<u8>);

impl std::fmt::Debug for Seccomp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Seccomp")
            .field("filter", &format_args!("{} bytes", self.0.len()))
            .finish()
    }
}

impl Seccomp {
    pub(in crate::linux) fn new(policy: &SeccompPolicy) -> Self {
        match policy {
            SeccompPolicy::Manual { policy } => (Seccomp(policy.clone())),
            SeccompPolicy::DenyDangerous => Self(gen::DENY_DANGEROUS.to_vec()),
            SeccompPolicy::Unrestricted => Self(gen::UNRESTRICTED.to_vec()),
            SeccompPolicy::Pure => Self(gen::PURE.to_vec()),
        }
    }

    pub(in crate::linux) fn enable(&self) {
        let ret = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if ret != 0 {
            panic!(
                "Failed to disable privilege escalation: {}",
                get_last_error()
            )
        }
        let prog = LibcSockFprog {
            len: (self.0.len() / 8).try_into().expect("too long program"),
            prog: self.0.as_ptr().cast(),
            phantom: PhantomData,
        };
        let ret = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                // operation
                SECCOMP_SET_MODE_FILTER,
                // flags
                0,
                // prog
                &prog as *const _,
            )
        };
        if ret != 0 {
            panic!("Failed to enable seccomp: {}", get_last_error())
        }
    }
}

const SECCOMP_SET_MODE_FILTER: i32 = 1;

#[repr(C)]
struct LibcSockFprog<'a> {
    len: libc::c_ushort,
    prog: *const libc::c_void,
    phantom: PhantomData<&'a ()>,
}
