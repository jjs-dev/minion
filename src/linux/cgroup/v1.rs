//! Implements Cgroup Driver for V1 cgroups
use crate::linux::cgroup::ResourceLimits;
use std::os::unix::io::{IntoRawFd, RawFd};
use std::path::PathBuf;
impl super::Driver {
    pub(super) fn setup_cgroups_v1(&self, limits: &ResourceLimits, cgroup_id: &str) -> Vec<RawFd> {
        // configure cpuacct subsystem
        let cpuacct_cgroup_path = self.get_path_for_cgroup_legacy_subsystem("cpuacct", cgroup_id);
        std::fs::create_dir_all(&cpuacct_cgroup_path).expect("failed to create cpuacct cgroup");

        // configure pids subsystem
        let pids_cgroup_path = self.get_path_for_cgroup_legacy_subsystem("pids", cgroup_id);
        std::fs::create_dir_all(&pids_cgroup_path).expect("failed to create pids cgroup");

        std::fs::write(
            pids_cgroup_path.join("pids.max"),
            format!("{}", limits.pids_max),
        )
        .expect("failed to enable pids limit");

        // configure memory subsystem
        let mem_cgroup_path = self.get_path_for_cgroup_legacy_subsystem("memory", cgroup_id);

        std::fs::create_dir_all(&mem_cgroup_path).expect("failed to create memory cgroup");
        std::fs::write(mem_cgroup_path.join("memory.swappiness"), "0")
            .expect("failed to disallow swapping");

        std::fs::write(
            mem_cgroup_path.join("memory.limit_in_bytes"),
            format!("{}", limits.memory_max),
        )
        .expect("failed to enable memory limiy");

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
                    .unwrap_or_else(|err| {
                        panic!("Couldn't open tasks file {}: {}", p.display(), err)
                    })
                    .into_raw_fd();
                nix::unistd::dup(h).expect("dup failed")
            })
            .collect::<Vec<_>>()
    }

    pub(super) fn get_memory_usage_v1(&self, cgroup_id: &str) -> u64 {
        let mut current_usage_file = self.get_path_for_cgroup_legacy_subsystem("memory", cgroup_id);
        current_usage_file.push("memory.max_usage_in_bytes");
        std::fs::read_to_string(current_usage_file)
            .expect("cannot read memory usage")
            .trim()
            .parse::<u64>()
            .unwrap()
    }

    pub(super) fn get_cpu_usage_v1(&self, cgroup_id: &str) -> u64 {
        let current_usage_file = self.get_path_for_cgroup_legacy_subsystem("cpuacct", cgroup_id);
        let current_usage_file = current_usage_file.join("cpuacct.usage");
        std::fs::read_to_string(current_usage_file)
            .expect("Couldn't load cpu usage")
            .trim()
            .parse::<u64>()
            .unwrap()
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
