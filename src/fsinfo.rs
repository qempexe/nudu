//! Facts about the filesystem a path lives on: its type and how full it is.

use std::fs;
use std::path::Path;

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
        Some(Self {
            fs_type: mount_type(path).unwrap_or_else(|| "?".into()),
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

/// Finds the type of the mount that contains `path`: the entry in
/// /proc/self/mounts with the longest mount point that is a prefix of it.
fn mount_type(path: &Path) -> Option<String> {
    let mounts = fs::read_to_string("/proc/self/mounts").ok()?;
    mounts
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let _device = fields.next()?;
            let mount_point = unescape(fields.next()?);
            let fs_type = fields.next()?;
            path.starts_with(&mount_point)
                .then(|| (mount_point.len(), fs_type.to_string()))
        })
        .max_by_key(|(len, _)| *len)
        .map(|(_, fs_type)| fs_type)
}

/// /proc/mounts writes spaces and tabs in paths as octal escapes (`\040`).
fn unescape(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\134", "\\")
}
