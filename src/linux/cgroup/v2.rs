//! Implements Cgroup Driver for V2 cgroups
use crate::linux::cgroup::{CgroupError, Driver, ResourceLimits};
use std::{
    os::unix::io::{IntoRawFd, RawFd},
    path::PathBuf,
};

impl Driver {
    fn v2_write_file(&self, cgroup_id: &str, file_name: &str, num: u64) -> Result<(), CgroupError> {
        let path = self.get_path_for_cgroup_unified(cgroup_id).join(file_name);
        let mut buf = itoa::Buffer::new();
        let data = buf.format(num);

        std::fs::write(&path, data).map_err(|cause| CgroupError::Write { path, cause })
    }

    fn v2_read_file(&self, cgroup_id: &str, file_name: &str) -> Result<String, CgroupError> {
        let path = self.get_path_for_cgroup_unified(cgroup_id).join(file_name);

        std::fs::read_to_string(&path).map_err(|cause| CgroupError::Read { path, cause })
    }

    pub(super) fn setup_cgroups_v2(
        &self,
        limits: &ResourceLimits,
        cgroup_id: &str,
    ) -> Result<RawFd, CgroupError> {
        let cgroup_path = self.get_path_for_cgroup_unified(cgroup_id);
        std::fs::create_dir_all(&cgroup_path).map_err(|cause| CgroupError::CreateCgroupDir {
            path: cgroup_path.clone(),
            cause,
        })?;

        // TODO: should we ignore this error?
        std::fs::write(
            cgroup_path.parent().unwrap().join("cgroup.subtree_control"),
            "+pids +cpu +memory",
        )
        .ok();

        self.v2_write_file(cgroup_id, "pids.max", limits.pids_max.into())?;
        self.v2_write_file(cgroup_id, "memory.max", limits.memory_max)?;

        let tasks_file_path = cgroup_path.join("cgroup.procs");
        let h = std::fs::OpenOptions::new()
            .write(true)
            .open(&tasks_file_path)
            .map_err(|cause| CgroupError::OpenFile {
                path: tasks_file_path.clone(),
                cause,
            })?;
        nix::unistd::dup(h.into_raw_fd()).map_err(|cause| CgroupError::DuplicateFd { cause })
    }

    pub(super) fn get_cpu_usage_v2(&self, cgroup_id: &str) -> Result<u64, CgroupError> {
        let stat_data = self.v2_read_file(cgroup_id, "cpu.stat")?;
        let mut val = u64::max_value();
        for line in stat_data.lines() {
            if line.starts_with("usage_usec") {
                let usage = line
                    .trim_start_matches("usage_usec ")
                    .trim_end_matches('\n');
                if let Ok(v) = usage.parse() {
                    val = v;
                    // multiply by 1000 to convert from microseconds to nanoseconds
                    val *= 1000;
                }
            }
        }
        Ok(val)
    }

    pub(super) fn drop_cgroup_v2(&self, cgroup_id: &str) {
        std::fs::remove_dir(self.get_path_for_cgroup_unified(cgroup_id)).ok();
    }

    fn get_cgroup_prefix(&self) -> PathBuf {
        let mut p = self.cgroupfs_path.clone();
        for comp in &self.cgroup_prefix {
            p.push(comp);
        }
        p
    }

    fn get_path_for_cgroup_unified(&self, cgroup_id: &str) -> PathBuf {
        self.get_cgroup_prefix()
            .join(format!("sandbox.{}", cgroup_id))
    }
}
