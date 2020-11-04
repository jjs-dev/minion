/// Implements Cgroup Driver - high-level cgroup manager
mod detect;
mod v1;
mod v2;

use crate::linux::util::Fd;
use std::{ffi::OsString, path::PathBuf};

// used by crate::linux::check
pub(in crate::linux) use detect::CgroupVersion;

/// Information, sufficient for joining a cgroup.
pub(in crate::linux) enum JoinHandle {
    /// Fds of `tasks` file in each hierarchy.
    V1(Vec<Fd>),
    /// Fd of `cgroup.procs` file in cgroup dir.
    V2(Fd),
}

impl JoinHandle {
    fn with(&self, f: impl FnOnce(&mut dyn Iterator<Item = Fd>)) {
        let mut slice_iter;
        let mut once_iter;
        let it: &mut dyn std::iter::Iterator<Item = Fd> = match &self {
            Self::V1(handles) => {
                slice_iter = handles.iter().copied();
                &mut slice_iter
            }
            Self::V2(handle) => {
                once_iter = std::iter::once(*handle);
                &mut once_iter
            }
        };
        f(it);
    }
    pub(super) fn join_self(&self) {
        let my_pid = std::process::id();
        let my_pid = format!("{}", my_pid);
        self.with(|it| {
            for fd in it {
                nix::unistd::write(fd, my_pid.as_bytes()).expect("Couldn't join cgroup");
            }
        });
    }
}

impl Drop for JoinHandle {
    fn drop(&mut self) {
        self.with(|it| {
            for fd in it {
                nix::unistd::close(fd).ok();
            }
        })
    }
}

/// Abstracts all cgroups manipulations.
/// Each Backend has exactly one Driver.
#[derive(Debug)]
pub(in crate::linux) struct Driver {
    cgroupfs_path: PathBuf,
    cgroup_prefix: Vec<OsString>,
    version: detect::CgroupVersion,
}

/// Represents resource limits imposed on sandbox
pub(in crate::linux) struct ResourceLimits {
    pub(in crate::linux) pids_max: u32,
    pub(in crate::linux) memory_max: u64,
}

impl Driver {
    pub(in crate::linux) fn new(
        settings: &crate::linux::Settings,
    ) -> Result<Driver, crate::linux::Error> {
        // TODO: take cgroupfs as prefix
        let (cgroup_version, cgroupfs_path) = detect::CgroupVersion::detect(None);
        let mut cgroup_prefix = Vec::new();
        for comp in settings.cgroup_prefix.components() {
            if let std::path::Component::Normal(n) = comp {
                cgroup_prefix.push(n.to_os_string());
            }
        }
        Ok(Driver {
            version: cgroup_version,
            cgroup_prefix,
            cgroupfs_path,
        })
    }
    pub(in crate::linux) fn create_group(
        &self,
        cgroup_id: &str,
        limits: &ResourceLimits,
    ) -> JoinHandle {
        match self.version {
            CgroupVersion::V1 => JoinHandle::V1(self.setup_cgroups_v1(limits, cgroup_id)),
            CgroupVersion::V2 => JoinHandle::V2(self.setup_cgroups_v2(limits, cgroup_id)),
        }
    }

    pub(in crate::linux) fn get_cpu_usage(&self, cgroup_id: &str) -> u64 {
        match self.version {
            CgroupVersion::V1 => self.get_cpu_usage_v1(cgroup_id),
            CgroupVersion::V2 => self.get_cpu_usage_v2(cgroup_id),
        }
    }

    pub(in crate::linux) fn get_memory_usage(&self, cgroup_id: &str) -> Option<u64> {
        match self.version {
            // memory cgroup v2 does not provide way to get peak memory usage.
            // `memory.current` contains only current usage.
            CgroupVersion::V2 => None,
            CgroupVersion::V1 => Some(self.get_memory_usage_v1(cgroup_id)),
        }
    }
    pub(in crate::linux) fn get_cgroup_tasks_file_path(&self, cgroup_id: &str) -> PathBuf {
        match self.version {
            CgroupVersion::V1 => self.get_cgroup_tasks_file_path_v1(cgroup_id),
            CgroupVersion::V2 => self.get_cgroup_tasks_file_path_v2(cgroup_id),
        }
    }

    pub(in crate::linux) fn drop_cgroup(&self, cgroup_id: &str, legacy_subsystems: &[&str]) {
        match self.version {
            CgroupVersion::V1 => self.drop_cgroup_v1(cgroup_id, legacy_subsystems),
            CgroupVersion::V2 => self.drop_cgroup_v2(cgroup_id),
        }
    }
}
