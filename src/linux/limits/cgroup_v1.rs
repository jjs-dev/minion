//! Implements Cgroup Driver for V1 cgroups
use crate::linux::limits::{
    cgroup_common::{CgroupEnter, CgroupError},
    InternalResourceUsageData, ResourceLimitImpl, ResourceLimits,
};
use std::{ffi::OsString, os::unix::io::IntoRawFd, path::PathBuf};

#[derive(Debug)]
pub(super) struct CgroupV1 {
    pub(super) cgroupfs_path: PathBuf,
    pub(super) cgroup_prefix: Vec<OsString>,
}

impl CgroupV1 {
    fn write_file(
        &self,
        cgroup_id: &str,
        subsys_name: &str,
        file_name: &str,
        num: u64,
    ) -> Result<(), CgroupError> {
        let path = self
            .get_path_for_cgroup_legacy_subsystem(subsys_name, cgroup_id)
            .join(file_name);
        let mut buf = itoa::Buffer::new();
        let data = buf.format(num);

        std::fs::write(&path, data).map_err(|cause| CgroupError::Write { path, cause })
    }

    fn read_file(
        &self,
        cgroup_id: &str,
        subsys_name: &str,
        file_name: &str,
    ) -> Result<String, CgroupError> {
        let path = self
            .get_path_for_cgroup_legacy_subsystem(subsys_name, cgroup_id)
            .join(file_name);

        std::fs::read_to_string(&path).map_err(|cause| CgroupError::Read { path, cause })
    }

    fn create_cgroup(&self, cgroup_id: &str, subsys_name: &str) -> Result<(), CgroupError> {
        let path = self.get_path_for_cgroup_legacy_subsystem(subsys_name, cgroup_id);

        std::fs::create_dir_all(&path).map_err(|cause| CgroupError::CreateCgroupDir { path, cause })
    }

    fn get_memory_usage(&self, cgroup_id: &str) -> Result<u64, CgroupError> {
        let usage = self
            .read_file(cgroup_id, "memory", "memory.max_usage_in_bytes")?
            .trim()
            .parse()
            .unwrap();
        Ok(usage)
    }

    fn get_cpu_usage(&self, cgroup_id: &str) -> Result<u64, CgroupError> {
        let usage = self
            .read_file(cgroup_id, "cpuacct", "cpuacct.usage")?
            .trim()
            .parse()
            .unwrap();
        Ok(usage)
    }

    fn drop_cgroup(&self, cgroup_id: &str, subsystems: &[&str]) {
        for subsys in subsystems {
            std::fs::remove_dir(self.get_path_for_cgroup_legacy_subsystem(subsys, cgroup_id)).ok();
        }
    }

    fn get_path_for_cgroup_legacy_subsystem(&self, subsys_name: &str, cgroup_id: &str) -> PathBuf {
        let mut p = self.cgroupfs_path.clone();
        p.push(subsys_name);
        for comp in &self.cgroup_prefix {
            p.push(comp);
        }
        p.push(format!("sandbox.{}", cgroup_id));
        p
    }
}

impl ResourceLimitImpl for CgroupV1 {
    type Enter = CgroupEnter;

    type Error = CgroupError;

    fn create_group(
        &self,
        group_id: &str,
        limits: &ResourceLimits,
    ) -> Result<Self::Enter, Self::Error> {
        // configure cpuacct subsystem
        self.create_cgroup(group_id, "cpuacct")?;

        // configure pids subsystem
        self.create_cgroup(group_id, "pids")?;
        self.write_file(group_id, "pids", "pids.max", limits.pids_max.into())?;

        // configure memory subsystem
        self.create_cgroup(group_id, "memory")?;
        self.write_file(group_id, "memory", "memory.swappiness", 0)?;
        self.write_file(
            group_id,
            "memory",
            "memory.limit_in_bytes",
            limits.memory_max,
        )?;

        // we return handles to tasksfiles for main cgroups
        // so, though zygote itself and children are in chroot, and cannot access cgroupfs, they will be able to add themselves to cgroups
        let handles = ["cpuacct", "memory", "pids"]
            .iter()
            .map(|subsys_name| {
                let p = self.get_path_for_cgroup_legacy_subsystem(subsys_name, group_id);
                let p = p.join("tasks");
                let h = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&p)
                    .map_err(|cause| CgroupError::OpenFile { path: p, cause })?
                    .into_raw_fd();
                nix::unistd::dup(h).map_err(|cause| CgroupError::DuplicateFd { cause })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CgroupEnter::V1(handles))
    }

    fn resource_usage(&self, group_id: &str) -> Result<InternalResourceUsageData, Self::Error> {
        Ok(InternalResourceUsageData {
            time: (self.get_cpu_usage(group_id)?),
            memory: Some(self.get_memory_usage(group_id)?),
        })
    }

    fn delete_group(&self, group_id: &str) -> Result<(), Self::Error> {
        self.drop_cgroup(group_id, &["pids", "memory", "cpuacct"]);
        Ok(())
    }
}
