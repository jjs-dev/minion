//! Implements Cgroup Driver for V1 cgroups
use crate::linux::cgroup::{CgroupError, ResourceLimits};
use std::{
    os::unix::io::{IntoRawFd, RawFd},
    path::PathBuf,
};
impl super::Driver {
    fn v1_write_file(
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

    fn v1_read_file(
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

    fn v1_create_cgroup(&self, cgroup_id: &str, subsys_name: &str) -> Result<(), CgroupError> {
        let path = self.get_path_for_cgroup_legacy_subsystem(subsys_name, cgroup_id);

        std::fs::create_dir_all(&path).map_err(|cause| CgroupError::CreateCgroupDir { path, cause })
    }

    pub(super) fn setup_cgroups_v1(
        &self,
        limits: &ResourceLimits,
        cgroup_id: &str,
    ) -> Result<Vec<RawFd>, CgroupError> {
        // configure cpuacct subsystem
        self.v1_create_cgroup(cgroup_id, "cpuacct")?;

        // configure pids subsystem
        self.v1_create_cgroup(cgroup_id, "pids")?;
        self.v1_write_file(cgroup_id, "pids", "pids.max", limits.pids_max.into())?;

        // configure memory subsystem
        self.v1_create_cgroup(cgroup_id, "memory")?;
        self.v1_write_file(cgroup_id, "memory", "memory.swappiness", 0)?;
        self.v1_write_file(
            cgroup_id,
            "memory",
            "memory.limit_in_bytes",
            limits.memory_max,
        )?;

        // we return handles to tasksfiles for main cgroups
        // so, though zygote itself and children are in chroot, and cannot access cgroupfs, they will be able to add themselves to cgroups
        ["cpuacct", "memory", "pids"]
            .iter()
            .map(|subsys_name| {
                let p = self.get_path_for_cgroup_legacy_subsystem(subsys_name, cgroup_id);
                let p = p.join("tasks");
                let h = std::fs::OpenOptions::new()
                    .write(true)
                    .open(&p)
                    .map_err(|cause| CgroupError::OpenFile { path: p, cause })?
                    .into_raw_fd();
                nix::unistd::dup(h).map_err(|cause| CgroupError::DuplicateFd { cause })
            })
            .collect::<Result<Vec<_>, _>>()
    }

    pub(super) fn get_memory_usage_v1(&self, cgroup_id: &str) -> Result<u64, CgroupError> {
        let usage = self
            .v1_read_file(cgroup_id, "memory", "memory.max_usage_in_bytes")?
            .trim()
            .parse()
            .unwrap();
        Ok(usage)
    }

    pub(super) fn get_cpu_usage_v1(&self, cgroup_id: &str) -> Result<u64, CgroupError> {
        let usage = self
            .v1_read_file(cgroup_id, "cpuacct", "cpuacct.usage")?
            .trim()
            .parse()
            .unwrap();
        Ok(usage)
    }

    pub(super) fn drop_cgroup_v1(&self, cgroup_id: &str, subsystems: &[&str]) {
        for subsys in subsystems {
            std::fs::remove_dir(self.get_path_for_cgroup_legacy_subsystem(subsys, cgroup_id)).ok();
        }
    }

    pub(super) fn get_cgroup_tasks_file_path_v1(&self, cgroup_id: &str) -> PathBuf {
        self.get_path_for_cgroup_legacy_subsystem("pids", cgroup_id)
            .join("tasks")
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
