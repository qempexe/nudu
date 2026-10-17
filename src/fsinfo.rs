//! Facts about the filesystem a path lives on: its type and how full it is.

use std::path::Path;

use crate::mounts::MountTable;

#[derive(Debug, Clone)]
pub struct FsInfo {
    /// Filesystem type as the kernel names it: `btrfs`, `ext4`, `tmpfs`, ...
    pub fs_type: String,
    /// Bytes in use on the whole filesystem.
    pub used: u64,
    /// Total size of the filesystem in bytes.
    pub total: u64,
}

impl FsInfo {
    /// Looks up the filesystem holding `path`. `path` should be absolute.
    pub fn for_path(path: &Path) -> Option<Self> {
        let stats = rustix::fs::statvfs(path).ok()?;
        let block = stats.f_frsize;
        let total = stats.f_blocks * block;
        let used = total.saturating_sub(stats.f_bfree * block);
        let fs_type = MountTable::load()
            .containing(path)
            .map(|m| m.fs_type.clone())
            .unwrap_or_else(|| "?".into());
        Some(Self {
            fs_type,
            used,
            total,
        })
    }

    /// Fraction of the filesystem in use, between 0.0 and 1.0.
    pub fn used_ratio(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.used as f64 / self.total as f64
        }
    }
}
