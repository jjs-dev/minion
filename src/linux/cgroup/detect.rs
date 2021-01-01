//! This module is responsible for CGroup version detection

use std::path::{Path, PathBuf};

#[derive(Eq, PartialEq, Debug)]
pub(in crate::linux) enum CgroupVersion {
    /// Legacy
    V1,
    /// Unified
    V2,
}

impl CgroupVersion {
    fn from_path(path: &Path) -> Option<(CgroupVersion, PathBuf)> {
        let stat = nix::sys::statfs::statfs(path).ok()?;
        let ty = stat.filesystem_type();
        // man 2 statfs
        match ty.0 {
            0x0027_e0eb => return Some((CgroupVersion::V1, path.to_path_buf())),
            0x6367_7270 => return Some((CgroupVersion::V2, path.to_path_buf())),
            _ => (),
        };
        let vers = if path.join("cgroup.subtree_control").exists() {
            CgroupVersion::V2
        } else {
            // TODO: better checks.
            CgroupVersion::V1
        };
        tracing::info!(mount_point = %path.display(), version = ?vers, "detected cgroups");
        Some((vers, path.to_path_buf()))
    }
    pub(in crate::linux) fn detect(
        path: &Path,
    ) -> Result<(CgroupVersion, PathBuf), crate::linux::Error> {
        Self::from_path(path).ok_or(crate::linux::Error::Cgroups)
    }
}
