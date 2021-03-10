//! Implements resource limits
mod cgroup_common;
mod cgroup_v1;
mod cgroup_v2;

use self::cgroup_common::CgroupEnter;
use crate::linux::{Error, Settings};
use rand::Rng;
use std::{fmt, path::PathBuf};

/// See ResourceUsageData for docs.
#[derive(Debug, Copy, Clone, Default)]
pub(in crate::linux) struct InternalResourceUsageData {
    /// Non-optional (this is the only difference from ResourceUsageData)
    pub time: u64,
    pub memory: Option<u64>,
}

/// Represents resource limits imposed on sandbox
pub(in crate::linux) struct ResourceLimits {
    pub(in crate::linux) pids_max: u32,
    pub(in crate::linux) memory_max: u64,
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
}

#[derive(Clone)]
enum OpaqueEnterHandleInner {
    Cgroup(CgroupEnter),
}

#[derive(Clone)]
pub(in crate::linux) struct OpaqueEnterHandle(OpaqueEnterHandleInner);

impl From<CgroupEnter> for OpaqueEnterHandle {
    fn from(inner: CgroupEnter) -> Self {
        OpaqueEnterHandle(OpaqueEnterHandleInner::Cgroup(inner))
    }
}

impl OpaqueEnterHandle {
    pub(in crate::linux) fn join(self) {
        let res = match self.0 {
            OpaqueEnterHandleInner::Cgroup(inner) => inner.join(),
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
}

#[derive(Debug)]
enum Inner {
    CgroupV1(cgroup_v1::CgroupV1),
    CgroupV2(cgroup_v2::CgroupV2),
}

#[derive(Debug)]
pub(in crate::linux) struct Driver {
    inner: Inner,
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
        };
        Driver { inner }
    }

    fn smoke_check(&self) -> Result<(), Error> {
        match &self.inner {
            Inner::CgroupV1(inner) => smoke_check(inner),
            Inner::CgroupV2(inner) => smoke_check(inner),
        }
    }

    pub fn new(settings: &Settings) -> Result<Self, Error> {
        let mut configs = Vec::new();
        for &cgroup_version in &[CgroupVersion::V1, CgroupVersion::V2] {
            configs.push(RawSettings::Cgroup {
                prefix: settings.cgroup_prefix.clone(),
                mount: settings.cgroupfs.clone(),
                version: cgroup_version,
            });
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
        };
        Ok(handle)
    }

    pub fn resource_usage(&self, group_id: &str) -> Result<InternalResourceUsageData, DriverError> {
        let res = match &self.inner {
            Inner::CgroupV1(inner) => inner.resource_usage(group_id)?,
            Inner::CgroupV2(inner) => inner.resource_usage(group_id)?,
        };
        Ok(res)
    }

    pub fn delete_group(&self, group_id: &str) -> Result<(), DriverError> {
        match &self.inner {
            Inner::CgroupV1(inner) => inner.delete_group(group_id)?,
            Inner::CgroupV2(inner) => inner.delete_group(group_id)?,
        };
        Ok(())
    }
}
