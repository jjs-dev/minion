//! This module is responsible for CGroup version detection
#[derive(Eq, PartialEq, Debug)]
pub(in crate::linux) enum CgroupVersion {
    /// Legacy
    V1,
    /// Unified
    V2,
}

impl CgroupVersion {
    pub(in crate::linux) fn detect() -> CgroupVersion {
        let stat = nix::sys::statfs::statfs("/sys/fs/cgroup")
            .expect("/sys/fs/cgroup is not root of cgroupfs");
        let ty = stat.filesystem_type();
        // man 2 statfs
        match ty.0 {
            0x0027_e0eb => return CgroupVersion::V1,
            0x6367_7270 => return CgroupVersion::V2,
            _ => (),
        };
        let p = std::path::Path::new("/sys/fs/cgroup");
        if p.join("cgroup.subtree_control").exists() {
            CgroupVersion::V2
        } else {
            CgroupVersion::V1
        }
    }
}
