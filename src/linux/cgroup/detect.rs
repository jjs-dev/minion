//! This module is responsible for CGroup version detection

use std::path::PathBuf;

#[derive(Eq, PartialEq, Debug)]
pub(in crate::linux) enum CgroupVersion {
    /// Legacy
    V1,
    /// Unified
    V2,
}

impl CgroupVersion {
    pub(in crate::linux) fn detect(hint: Option<PathBuf>) -> (CgroupVersion, PathBuf) {
        let path = hint
            .or_else(|| Some(std::env::var_os("MINION_CGROUPFS")?.into()))
            .unwrap_or_else(|| "/sys/fs/cgroup".into());
        let stat = nix::sys::statfs::statfs(&path)
            .unwrap_or_else(|_| panic!("{} is not root of cgroupfs", path.display()));
        let ty = stat.filesystem_type();
        // man 2 statfs
        match ty.0 {
            0x0027_e0eb => return (CgroupVersion::V1, path),
            0x6367_7270 => return (CgroupVersion::V2, path),
            _ => (),
        };
        let vers = if path.join("cgroup.subtree_control").exists() {
            CgroupVersion::V2
        } else {
            CgroupVersion::V1
        };

        if cfg!(minion_ci) {
            let expected_version = match std::env::var("CI_CGROUPS").unwrap().as_str() {
                "cgroup-v1" => CgroupVersion::V1,
                "cgroup-v2" => CgroupVersion::V2,
                _ => panic!(),
            };

            assert_eq!(vers, expected_version);
        }
        (vers, path)
    }
}
