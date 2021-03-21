//! Implements resource limits
mod cgroup_common;
mod cgroup_v1;
mod cgroup_v2;
mod prlimit;

use self::{cgroup_common::CgroupEnter, prlimit::PrlimitEnter};
use crate::linux::{jail_common::ZygoteInfo, Error, ResourceDriverKind, Settings};
use parking_lot::Mutex;
use rand::Rng;
use std::{collections::HashMap, fmt, path::PathBuf, sync::Arc};

/// See ResourceUsageData for docs.
#[derive(Debug, Copy, Clone, Default)]
pub(in crate::linux) struct InternalResourceUsageData {
    /// Non-optional (this is the only difference from ResourceUsageData)
    pub time: u64,
    pub memory: Option<u64>,
}

/// Represents resource limits imposed on sandbox
#[derive(Clone)]
pub(in crate::linux) struct ResourceLimits {
    pub(in crate::linux) pids_max: u32,
    pub(in crate::linux) memory_max: u64,
    pub(in crate::linux) cpu_usage: u64,
}

trait ResourceLimitImpl {
    /// Can be called by specific process to setup limits.
    /// Must survive `fork`.
    type Enter: EnterHandle;

    type Error: std::error::Error + Send + Sync + 'static + Into<DriverError>;

    fn create_group(
        &self,
        group_id: &str,
        limits: &ResourceLimits,
    ) -> Result<Self::Enter, Self::Error>;

    fn register_group_details(&self, _group_id: &str, _zygote: Arc<Mutex<Option<ZygoteInfo>>>) {}

    fn delete_group(&self, group_id: &str) -> Result<(), Self::Error>;

    fn resource_usage(&self, group_id: &str) -> Result<InternalResourceUsageData, Self::Error>;
}

trait EnterHandle: Clone {
    /// Sets up restrictions for the current process.
    /// Must return Err if enforcement failed.
    fn join(self) -> anyhow::Result<()>;

    fn check_access(&self) -> Result<(), Error>;
}

fn smoke_check<R: ResourceLimitImpl>(imp: &R) -> Result<(), Error> {
    let mut group_id = "minion-cgroup-access-check-".to_string();
    let mut rng = rand::thread_rng();
    for _ in 0..5 {
        group_id.push(rng.sample(rand::distributions::Alphanumeric) as char);
    }
    let cgroup = imp
        .create_group(
            &group_id,
            &ResourceLimits {
                memory_max: 1 << 30,
                pids_max: 1024,
                cpu_usage: 1_000_000_000,
            },
        )
        .map_err(Into::into)?;

    cgroup.check_access()?;
    Ok(())
}

#[derive(thiserror::Error, Debug)]
pub enum DriverError {
    #[error("cgroup manipulation failed")]
    Cgroup(#[from] cgroup_common::CgroupError),
    #[error("prlimit-based impl failed")]
    Prlimit(#[from] prlimit::PrlimitError),
}

#[derive(Clone)]
enum OpaqueEnterHandleInner {
    Cgroup(CgroupEnter),
    Prlimit(PrlimitEnter),
}

#[derive(Clone)]
pub(in crate::linux) struct OpaqueEnterHandle(OpaqueEnterHandleInner);

impl From<CgroupEnter> for OpaqueEnterHandle {
    fn from(inner: CgroupEnter) -> Self {
        OpaqueEnterHandle(OpaqueEnterHandleInner::Cgroup(inner))
    }
}

impl From<PrlimitEnter> for OpaqueEnterHandle {
    fn from(inner: PrlimitEnter) -> Self {
        OpaqueEnterHandle(OpaqueEnterHandleInner::Prlimit(inner))
    }
}

impl OpaqueEnterHandle {
    pub(in crate::linux) fn join(self) {
        let res = match self.0 {
            OpaqueEnterHandleInner::Cgroup(inner) => inner.join(),
            OpaqueEnterHandleInner::Prlimit(inner) => inner.join(),
        };

        if let Err(e) = res {
            eprintln!("FATAL: failed to setup resource limits: {:#}", e);
            std::process::exit(1);
        }
    }
}

/// Type holding raw settings.
#[derive(Debug)]
pub struct RawSettingsWrapper(RawSettings);

#[derive(Debug)]
pub struct DriverInitializationError {
    pub attempts: Vec<(RawSettingsWrapper, Error)>,
}

impl fmt::Display for DriverInitializationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (settings, error) in &self.attempts {
            writeln!(f, "Tried {:?}, got: {:#}", settings, error)?;
        }
        Ok(())
    }
}

impl std::error::Error for DriverInitializationError {}

#[derive(Eq, PartialEq, Debug, Clone, Copy)]
pub(in crate::linux) enum CgroupVersion {
    /// Legacy
    V1,
    /// Unified
    V2,
}

/// Raw Cgroup Driver creation arguments.
#[derive(Debug)]
enum RawSettings {
    Cgroup {
        prefix: PathBuf,
        mount: PathBuf,
        version: CgroupVersion,
    },
    Prlimit {
        allow_many_pids: bool,
    },
}

#[derive(Debug)]
enum Inner {
    CgroupV1(cgroup_v1::CgroupV1),
    CgroupV2(cgroup_v2::CgroupV2),
    Prlimit(prlimit::Prlimit),
}

#[derive(Debug)]
pub(in crate::linux) struct Driver {
    inner: Inner,
}

fn process_driver_kind(settings: &Settings, out: &mut Vec<RawSettings>, kind: &ResourceDriverKind) {
    match kind {
        ResourceDriverKind::CgroupV1 => {
            out.push(RawSettings::Cgroup {
                prefix: settings.cgroup.name_prefix.clone(),
                mount: settings.cgroup.mount.clone(),
                version: CgroupVersion::V1,
            });
        }
        ResourceDriverKind::CgroupV2 => out.push(RawSettings::Cgroup {
            prefix: settings.cgroup.name_prefix.clone(),
            mount: settings.cgroup.mount.clone(),
            version: CgroupVersion::V2,
        }),
        ResourceDriverKind::CgroupAuto => {
            process_driver_kind(settings, out, &ResourceDriverKind::CgroupV1);
            process_driver_kind(settings, out, &ResourceDriverKind::CgroupV2);
        }
        ResourceDriverKind::Prlimit => out.push(RawSettings::Prlimit {
            allow_many_pids: !settings.rootless,
        }),
        ResourceDriverKind::Auto { allow_dangerous } => {
            process_driver_kind(settings, out, &ResourceDriverKind::CgroupAuto);
            if *allow_dangerous {
                process_driver_kind(settings, out, &ResourceDriverKind::Prlimit);
            }
        }
    }
}

impl Driver {
    fn new_raw(raw_settings: &RawSettings) -> Self {
        let inner = match raw_settings {
            RawSettings::Cgroup {
                prefix,
                mount,
                version,
            } => {
                let mut cgroup_prefix = Vec::new();
                for comp in prefix.components() {
                    if let std::path::Component::Normal(n) = comp {
                        cgroup_prefix.push(n.to_os_string());
                    }
                }
                let cgroupfs_path = mount.clone();
                match version {
                    CgroupVersion::V1 => Inner::CgroupV1(cgroup_v1::CgroupV1 {
                        cgroupfs_path,
                        cgroup_prefix,
                    }),
                    CgroupVersion::V2 => Inner::CgroupV2(cgroup_v2::CgroupV2 {
                        cgroupfs_path,
                        cgroup_prefix,
                    }),
                }
            }
            RawSettings::Prlimit { allow_many_pids } => Inner::Prlimit(prlimit::Prlimit {
                groups: Mutex::new(HashMap::new()),
                allow_multiple_processes: *allow_many_pids,
            }),
        };
        Driver { inner }
    }

    fn smoke_check(&self) -> Result<(), Error> {
        match &self.inner {
            Inner::CgroupV1(inner) => smoke_check(inner),
            Inner::CgroupV2(inner) => smoke_check(inner),
            Inner::Prlimit(inner) => smoke_check(inner),
        }
    }

    pub fn new(settings: &Settings) -> Result<Self, Error> {
        let mut configs = Vec::new();
        for driver_kind in &settings.resource_drivers {
            process_driver_kind(settings, &mut configs, driver_kind)
        }

        let mut err = DriverInitializationError {
            attempts: Vec::new(),
        };
        for config in configs {
            let driver = Self::new_raw(&config);
            match driver.smoke_check() {
                Ok(()) => {
                    tracing::debug!(settings=?config, "Found working configuration");
                    return Ok(driver);
                }
                Err(e) => {
                    tracing::debug!(settings=?config, error=%e, "Configuration does not work");
                    err.attempts.push((RawSettingsWrapper(config), e));
                }
            }
        }
        Err(Error::SelectDriverImpl { cause: err })
    }

    pub fn create_group(
        &self,
        group_id: &str,
        limits: &ResourceLimits,
    ) -> Result<OpaqueEnterHandle, DriverError> {
        let handle = match &self.inner {
            Inner::CgroupV1(inner) => inner.create_group(group_id, limits)?.into(),
            Inner::CgroupV2(inner) => inner.create_group(group_id, limits)?.into(),
            Inner::Prlimit(inner) => inner.create_group(group_id, limits)?.into(),
        };
        Ok(handle)
    }

    pub fn resource_usage(&self, group_id: &str) -> Result<InternalResourceUsageData, DriverError> {
        let res = match &self.inner {
            Inner::CgroupV1(inner) => inner.resource_usage(group_id)?,
            Inner::CgroupV2(inner) => inner.resource_usage(group_id)?,
            Inner::Prlimit(inner) => inner.resource_usage(group_id)?,
        };
        Ok(res)
    }

    pub fn delete_group(&self, group_id: &str) -> Result<(), DriverError> {
        match &self.inner {
            Inner::CgroupV1(inner) => inner.delete_group(group_id)?,
            Inner::CgroupV2(inner) => inner.delete_group(group_id)?,
            Inner::Prlimit(inner) => inner.delete_group(group_id)?,
        };
        Ok(())
    }

    pub fn register_group_details(&self, group_id: &str, zygote: Arc<Mutex<Option<ZygoteInfo>>>) {
        match &self.inner {
            Inner::CgroupV1(inner) => inner.register_group_details(group_id, zygote),
            Inner::CgroupV2(inner) => inner.register_group_details(group_id, zygote),
            Inner::Prlimit(inner) => inner.register_group_details(group_id, zygote),
        }
    }

    pub fn get_watchdog(&self) -> bool {
        match &self.inner {
            Inner::CgroupV1(_) => true,
            Inner::CgroupV2(_) => true,
            Inner::Prlimit(_) => false,
        }
    }
}
