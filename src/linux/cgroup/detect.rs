//! This module is responsible for CGroup version detection

use crate::linux::cgroup::{CgroupError, Driver, ResourceLimits};
use rand::Rng;

#[derive(Eq, PartialEq, Debug, Clone, Copy)]
pub(in crate::linux) enum CgroupVersion {
    /// Legacy
    V1,
    /// Unified
    V2,
}

impl Driver {
    /// Checks that this Driver instance works correctly.
    /// This is used for cgroup version auto-detection and system checking.
    pub(in crate::linux) fn smoke_check(&self) -> Result<(), CgroupError> {
        let mut group_id = "minion-cgroup-access-check-".to_string();
        let mut rng = rand::thread_rng();
        for _ in 0..5 {
            group_id.push(rng.sample(rand::distributions::Alphanumeric) as char);
        }
        let cgroup = self.create_group(
            &group_id,
            &ResourceLimits {
                memory_max: 1 << 30,
                pids_max: 1024,
            },
        )?;

        cgroup
            .check_access()
            .map_err(|cause| CgroupError::Join { cause })?;
        Ok(())
    }
}
