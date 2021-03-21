use crate::linux::{
    ipc::IpcError,
    jail_common::{Query, ResourceUsageInformation, ZygoteInfo},
    limits::{EnterHandle, InternalResourceUsageData, ResourceLimitImpl, ResourceLimits},
    Error,
};
use parking_lot::Mutex;
use std::{collections::HashMap, sync::Arc};

#[derive(Debug)]
pub(super) struct Group {
    zygote: Option<Arc<Mutex<Option<ZygoteInfo>>>>,
}

#[derive(Debug)]
pub(super) struct Prlimit {
    pub(super) groups: Mutex<HashMap<String, Group>>,
    pub(super) allow_multiple_processes: bool,
}

#[derive(Clone)]
pub(in crate::linux) struct PrlimitEnter {
    limits: ResourceLimits,
}

#[derive(thiserror::Error, Debug)]
pub enum PrlimitError {
    #[error("ipc error")]
    Ipc(#[from] IpcError),
    #[error("unable to correctly enforce pids>1 limit without root")]
    PidsLimitEnforcementImpossible,
    #[error("sandbox is killed")]
    SandboxGone,
}

impl EnterHandle for PrlimitEnter {
    fn check_access(&self) -> Result<(), Error> {
        // TODO
        Ok(())
    }

    fn join(self) -> anyhow::Result<()> {
        unsafe {
            let lim = libc::rlimit {
                rlim_cur: self.limits.memory_max,
                rlim_max: self.limits.memory_max,
            };
            if libc::setrlimit(libc::RLIMIT_DATA, &lim) == -1 {
                return Err(std::io::Error::last_os_error().into());
            }

            let lim = libc::rlimit {
                rlim_cur: self.limits.pids_max.into(),
                rlim_max: self.limits.pids_max.into(),
            };
            // NOTE: this is not process limit, but threads limit
            // this must be paired with a seccomp policy that bans process
            // creation.
            if libc::setrlimit(libc::RLIMIT_NPROC, &lim) == -1 {
                return Err(std::io::Error::last_os_error().into());
            }

            let cpu_usage_limit = (self.limits.cpu_usage - 1) / 1_000_000_000 + 1;

            let lim = libc::rlimit {
                rlim_cur: cpu_usage_limit,
                rlim_max: cpu_usage_limit,
            };

            // NOTE: this limit makes sure that every single process
            // does not use too much CPU and eventually terminates.
            // Later enforcement is implemented by the watchdog.
            if libc::setrlimit(libc::RLIMIT_CPU, &lim) == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        Ok(())
    }
}

impl ResourceLimitImpl for Prlimit {
    type Error = PrlimitError;

    type Enter = PrlimitEnter;

    fn create_group(
        &self,
        group_id: &str,
        limits: &ResourceLimits,
    ) -> Result<PrlimitEnter, Self::Error> {
        if limits.pids_max > 1 && !self.allow_multiple_processes {
            return Err(PrlimitError::PidsLimitEnforcementImpossible);
        }
        let prev = self
            .groups
            .lock()
            .insert(group_id.to_string(), Group { zygote: None });
        assert!(prev.is_none());
        Ok(PrlimitEnter {
            limits: limits.clone(),
        })
    }

    fn delete_group(&self, group_id: &str) -> Result<(), Self::Error> {
        let deleted = self.groups.lock().remove(group_id);
        assert!(deleted.is_some());
        Ok(())
    }

    fn register_group_details(&self, group_id: &str, z: Arc<Mutex<Option<ZygoteInfo>>>) {
        let mut groups = self.groups.lock();
        let group = groups
            .get_mut(group_id)
            .expect("register_group_details is called on unknown group_id");
        group.zygote = Some(z);
    }

    fn resource_usage(&self, group_id: &str) -> Result<InternalResourceUsageData, Self::Error> {
        let groups = self.groups.lock();
        let group = groups
            .get(group_id)
            .expect("resource_usage is called on unknown group_id");
        let zygote = group.zygote.clone().expect("zygotek not initialized yet");
        let mut zygote = zygote.lock();
        let zygote = zygote.as_mut().ok_or(PrlimitError::SandboxGone)?;
        drop(groups);
        zygote.sock.send(&Query::GetResourceUsage)?;
        let info: ResourceUsageInformation = zygote.sock.recv()?;
        Ok(InternalResourceUsageData {
            memory: Some(info.memory),
            time: info.cpu,
        })
    }
}
