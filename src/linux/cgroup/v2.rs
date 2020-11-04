//! Implements Cgroup Driver for V2 cgroups
use crate::linux::cgroup::{Driver, ResourceLimits};
use std::{
    os::unix::io::{IntoRawFd, RawFd},
    path::PathBuf,
};

impl Driver {
    pub(super) fn setup_cgroups_v2(&self, limits: &ResourceLimits, cgroup_id: &str) -> RawFd {
        let cgroup_path = self.get_path_for_cgroup_unified(cgroup_id);
        std::fs::create_dir_all(&cgroup_path).expect("failed to create cgroup");

        std::fs::write(
            cgroup_path.parent().unwrap().join("cgroup.subtree_control"),
            "+pids +cpu +memory",
        )
        .ok();

        std::fs::write(cgroup_path.join("pids.max"), format!("{}", limits.pids_max))
            .expect("failed to set pids.max limit");

        std::fs::write(
            cgroup_path.join("memory.max"),
            format!("{}", limits.memory_max),
        )
        .expect("failed to set memory limit");

        let tasks_file_path = cgroup_path.join("cgroup.procs");
        let h = std::fs::OpenOptions::new()
            .write(true)
            .open(&tasks_file_path)
            .unwrap_or_else(|err| {
                panic!(
                    "Failed to open tasks file {}: {}",
                    tasks_file_path.display(),
                    err
                )
            });
        nix::unistd::dup(h.into_raw_fd()).expect("dup failed")
    }

    pub(super) fn get_cpu_usage_v2(&self, cgroup_id: &str) -> u64 {
        let mut current_usage_file = self.get_path_for_cgroup_unified(cgroup_id);
        current_usage_file.push("cpu.stat");
        let stat_data =
            std::fs::read_to_string(current_usage_file).expect("failed to read cpu.stat");
        let mut val = 0;
        for line in stat_data.lines() {
            if line.starts_with("usage_usec") {
                let usage = line
                    .trim_start_matches("usage_usec ")
                    .trim_end_matches('\n');
                val = usage.parse().unwrap();
            }
        }
        // multiply by 1000 to convert from microseconds to nanoseconds
        val * 1000
    }

    pub(super) fn drop_cgroup_v2(&self, cgroup_id: &str) {
        std::fs::remove_dir(self.get_path_for_cgroup_unified(cgroup_id)).ok();
    }

    pub(super) fn get_cgroup_tasks_file_path_v2(&self, cgroup_id: &str) -> PathBuf {
        self.get_path_for_cgroup_unified(cgroup_id)
            .join("cgroup.procs")
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
