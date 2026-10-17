//! The system's mount table, and deciding whether two mounts are the same
//! filesystem.
//!
//! Comparing device numbers is not enough: on btrfs every subvolume (`@`,
//! `@home`, snapshots...) reports its own device number, even though they
//! all live in one filesystem on one partition. So we look at what is
//! mounted where, from /proc/self/mountinfo.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    /// Where it is mounted, like `/home`.
    pub point: PathBuf,
    /// Filesystem type, like `btrfs`, `ext4`, `tmpfs`.
    pub fs_type: String,
    /// What is mounted: a device like `/dev/nvme0n1p2`, or a name like `tmpfs`.
    pub source: String,
    /// Device id of the filesystem (`major:minor`), shared by all its mounts.
    pub dev: String,
}

#[derive(Debug, Default)]
pub struct MountTable {
    mounts: Vec<Mount>,
    /// Mount point -> index into `mounts` of the mount visible there.
    by_point: HashMap<PathBuf, usize>,
}

impl MountTable {
    /// Reads the mount table of the running system.
    pub fn load() -> Self {
        fs::read_to_string("/proc/self/mountinfo")
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// Parses the contents of a mountinfo file. Each line looks like:
    ///
    /// ```text
    /// 27 26 0:25 /@home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p2 rw,subvol=/@home
    /// id parent dev root  point options   [tags] - type  source         super-options
    /// ```
    pub fn parse(text: &str) -> Self {
        let mut table = Self::default();
        for line in text.lines() {
            let Some((left, right)) = line.split_once(" - ") else {
                continue;
            };
            let left: Vec<&str> = left.split_whitespace().collect();
            let mut right = right.split_whitespace();
            let (Some(dev), Some(point)) = (left.get(2), left.get(4)) else {
                continue;
            };
            let (Some(fs_type), Some(source)) = (right.next(), right.next()) else {
                continue;
            };
            let mount = Mount {
                point: PathBuf::from(unescape(point)),
                fs_type: fs_type.to_string(),
                source: unescape(source),
                dev: dev.to_string(),
            };
            // Later lines are mounted on top of earlier ones at the same
            // point, so the last one wins.
            table
                .by_point
                .insert(mount.point.clone(), table.mounts.len());
            table.mounts.push(mount);
        }
        table
    }

    /// The mount whose filesystem has device number `dev` (from `stat`).
    ///
    /// Not every filesystem can be found this way: btrfs gives each
    /// subvolume an anonymous device number in `stat` that differs from
    /// the one in mountinfo. Callers must treat `None` as "unknown".
    pub fn by_device(&self, dev: u64) -> Option<&Mount> {
        let id = format!("{}:{}", rustix::fs::major(dev), rustix::fs::minor(dev));
        self.mounts.iter().rev().find(|m| m.dev == id)
    }

    /// The mount whose mount point is exactly `path`, if any.
    pub fn at(&self, path: &Path) -> Option<&Mount> {
        self.by_point.get(path).map(|&i| &self.mounts[i])
    }

    /// The mount that contains `path`: the longest mount point above it.
    pub fn containing(&self, path: &Path) -> Option<&Mount> {
        path.ancestors().find_map(|p| self.at(p))
    }

    /// Real storage a person would call a drive: disks, partitions, USB
    /// sticks, network shares. One row per filesystem, so a btrfs disk with
    /// five subvolume mounts shows once, at its shortest mount point.
    pub fn drives(&self) -> Vec<Drive> {
        let mut drives: Vec<Drive> = Vec::new();
        for (i, m) in self.mounts.iter().enumerate() {
            // Skip mounts hidden under a later mount at the same point.
            if self.by_point.get(&m.point) != Some(&i) || !is_drive(&m.fs_type) {
                continue;
            }
            match drives.iter_mut().find(|d| same_filesystem(&d.mount, m)) {
                Some(d) => {
                    d.extra_mounts += 1;
                    if m.point.as_os_str().len() < d.mount.point.as_os_str().len() {
                        d.mount = m.clone();
                    }
                }
                None => drives.push(Drive {
                    mount: m.clone(),
                    extra_mounts: 0,
                }),
            }
        }
        drives.sort_by(|a, b| a.mount.point.cmp(&b.mount.point));
        drives
    }

    /// Drives in `self` that are not in `before`: what just got mounted.
    pub fn added_since(&self, before: &MountTable) -> Vec<Drive> {
        self.drives()
            .into_iter()
            .filter(|d| {
                before
                    .at(&d.mount.point)
                    .is_none_or(|old| old.dev != d.mount.dev)
            })
            .collect()
    }

    /// Drives in `before` that are gone from `self`.
    pub fn removed_since(&self, before: &MountTable) -> Vec<Drive> {
        before.added_since(self)
    }
}

/// One filesystem in the drives list.
#[derive(Debug, Clone)]
pub struct Drive {
    pub mount: Mount,
    /// Other places the same filesystem is mounted (btrfs subvolumes...).
    pub extra_mounts: usize,
}

/// True for filesystems that store real files on some device.
fn is_drive(fs_type: &str) -> bool {
    !is_virtual(fs_type)
        && !matches!(
            fs_type,
            "tmpfs" | "devtmpfs" | "ramfs" | "overlay" | "squashfs" | "nsfs" | "tracefs"
        )
        // Desktop helper mounts, not storage.
        && !fs_type.starts_with("fuse.portal")
        && !fs_type.starts_with("fuse.gvfsd")
}

/// True when two mounts show parts of the same filesystem.
pub fn same_filesystem(a: &Mount, b: &Mount) -> bool {
    if a.dev == b.dev {
        return true;
    }
    if a.fs_type != b.fs_type {
        return false;
    }
    match a.fs_type.as_str() {
        // Subvolumes of one btrfs (or bcachefs) filesystem: same device.
        "btrfs" | "bcachefs" => a.source.starts_with('/') && a.source == b.source,
        // ZFS datasets of the same pool: `tank/home` and `tank/root`.
        "zfs" => pool(&a.source) == pool(&b.source),
        _ => false,
    }
}

fn pool(source: &str) -> &str {
    source.split('/').next().unwrap_or(source)
}

/// Filesystems that may not report how much disk space a file uses.
/// Many FUSE filesystems (sshfs, rclone, ...) say 0 blocks for every file.
pub fn may_lack_block_counts(fs_type: &str) -> bool {
    fs_type.starts_with("fuse.")
}

/// Kernel filesystems that hold no real files: always skipped.
pub fn is_virtual(fs_type: &str) -> bool {
    matches!(
        fs_type,
        "proc"
            | "sysfs"
            | "devpts"
            | "cgroup"
            | "cgroup2"
            | "securityfs"
            | "debugfs"
            | "tracefs"
            | "configfs"
            | "fusectl"
            | "pstore"
            | "bpf"
            | "efivarfs"
            | "mqueue"
            | "hugetlbfs"
            | "autofs"
            | "binfmt_misc"
            | "rpc_pipefs"
            | "nsfs"
    )
}

/// Snapshot folders made by snapper and Timeshift. Snapshots share their
/// data with the live files, so counting them would inflate every total.
pub fn is_snapshot_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some(".snapshots" | "timeshift-btrfs")
    )
}

/// mountinfo writes spaces, tabs and backslashes in paths as octal escapes.
fn unescape(field: &str) -> String {
    field
        .replace("\\040", " ")
        .replace("\\011", "\t")
        .replace("\\012", "\n")
        .replace("\\134", "\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A typical Omarchy / Arch btrfs layout, plus a few other mounts.
    const SAMPLE: &str = "\
26 1 0:25 /@ / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw,compress=zstd:3,subvol=/@
27 26 0:25 /@home /home rw,relatime shared:2 - btrfs /dev/nvme0n1p2 rw,compress=zstd:3,subvol=/@home
28 26 0:25 /@snapshots /.snapshots rw,relatime shared:3 - btrfs /dev/nvme0n1p2 rw,subvol=/@snapshots
29 26 259:1 / /boot rw,relatime shared:4 - vfat /dev/nvme0n1p1 rw,fmask=0077
30 26 0:22 / /proc rw,nosuid shared:5 - proc proc rw
31 26 0:30 / /tmp rw,nosuid shared:6 - tmpfs tmpfs rw,size=8G
32 27 8:17 / /home/me/My\\040Drive rw,relatime shared:7 - ext4 /dev/sdb1 rw
";

    fn table() -> MountTable {
        MountTable::parse(SAMPLE)
    }

    #[test]
    fn finds_mounts_by_point_and_by_path() {
        let t = table();
        assert_eq!(t.at(Path::new("/home")).unwrap().fs_type, "btrfs");
        assert!(t.at(Path::new("/home/me")).is_none());
        assert_eq!(
            t.containing(Path::new("/home/me/x")).unwrap().point,
            Path::new("/home")
        );
        assert_eq!(
            t.containing(Path::new("/usr/lib")).unwrap().point,
            Path::new("/")
        );
    }

    #[test]
    fn unescapes_spaces_in_mount_points() {
        let t = table();
        assert_eq!(
            t.at(Path::new("/home/me/My Drive")).unwrap().fs_type,
            "ext4"
        );
    }

    #[test]
    fn btrfs_subvolumes_are_one_filesystem() {
        let t = table();
        let root = t.at(Path::new("/")).unwrap();
        assert!(same_filesystem(root, t.at(Path::new("/home")).unwrap()));
        assert!(same_filesystem(
            root,
            t.at(Path::new("/.snapshots")).unwrap()
        ));
    }

    #[test]
    fn btrfs_subvolumes_match_by_source_even_with_different_ids() {
        let mut a = table().at(Path::new("/")).unwrap().clone();
        let b = table().at(Path::new("/home")).unwrap().clone();
        a.dev = "0:99".into();
        assert!(same_filesystem(&a, &b));
    }

    #[test]
    fn different_filesystems_stay_apart() {
        let t = table();
        let root = t.at(Path::new("/")).unwrap();
        for other in ["/boot", "/proc", "/tmp", "/home/me/My Drive"] {
            assert!(
                !same_filesystem(root, t.at(Path::new(other)).unwrap()),
                "{other}"
            );
        }
    }

    #[test]
    fn tmpfs_mounts_are_not_merged_by_name() {
        let a = Mount {
            point: "/tmp".into(),
            fs_type: "tmpfs".into(),
            source: "tmpfs".into(),
            dev: "0:30".into(),
        };
        let b = Mount {
            point: "/run".into(),
            fs_type: "tmpfs".into(),
            source: "tmpfs".into(),
            dev: "0:31".into(),
        };
        assert!(!same_filesystem(&a, &b));
    }

    #[test]
    fn zfs_datasets_of_one_pool_belong_together() {
        let a = Mount {
            point: "/".into(),
            fs_type: "zfs".into(),
            source: "tank/root".into(),
            dev: "0:40".into(),
        };
        let b = Mount {
            point: "/home".into(),
            fs_type: "zfs".into(),
            source: "tank/home".into(),
            dev: "0:41".into(),
        };
        let c = Mount {
            point: "/data".into(),
            fs_type: "zfs".into(),
            source: "other/data".into(),
            dev: "0:42".into(),
        };
        assert!(same_filesystem(&a, &b));
        assert!(!same_filesystem(&a, &c));
    }

    #[test]
    fn drives_list_one_row_per_filesystem() {
        let drives = table().drives();
        let points: Vec<_> = drives.iter().map(|d| d.mount.point.clone()).collect();
        // btrfs shows once (at /, with /home and /.snapshots folded in),
        // proc and tmpfs are not drives.
        assert_eq!(
            points,
            [
                PathBuf::from("/"),
                "/boot".into(),
                "/home/me/My Drive".into()
            ]
        );
        assert_eq!(drives[0].extra_mounts, 2);
    }

    #[test]
    fn notices_new_and_removed_drives() {
        let without = MountTable::parse(SAMPLE);
        let usb = "40 26 8:33 / /run/media/me/USB rw shared:9 - vfat /dev/sdc1 rw\n";
        let with = MountTable::parse(&format!("{SAMPLE}{usb}"));

        // Plugging it in: `with` is now, `without` is before.
        let added = with.added_since(&without);
        assert_eq!(added.len(), 1);
        assert_eq!(added[0].mount.point, Path::new("/run/media/me/USB"));
        assert!(with.removed_since(&without).is_empty());

        // Pulling it out: `without` is now, `with` is before.
        let removed = without.removed_since(&with);
        assert_eq!(removed.len(), 1);
        assert!(without.added_since(&with).is_empty());
    }

    #[test]
    fn finds_a_mount_by_device_number() {
        // Uses the sample table, not this machine's: on btrfs, stat() gives
        // each subvolume its own anonymous device number that no mountinfo
        // line carries, so a test on the real root would depend on the host.
        let t = table();
        let proc_dev = rustix::fs::makedev(0, 22);
        assert_eq!(t.by_device(proc_dev).unwrap().point, Path::new("/proc"));
        let boot_dev = rustix::fs::makedev(259, 1);
        assert_eq!(t.by_device(boot_dev).unwrap().fs_type, "vfat");
        // A device nobody mounted (like a btrfs subvolume's anonymous one).
        assert!(t.by_device(rustix::fs::makedev(0, 999)).is_none());
    }

    #[test]
    fn only_fuse_may_lack_block_counts() {
        assert!(may_lack_block_counts("fuse.sshfs"));
        assert!(may_lack_block_counts("fuse.rclone"));
        assert!(!may_lack_block_counts("ext4"));
        assert!(!may_lack_block_counts("btrfs"));
        // ntfs-3g mounts as `fuseblk` and does report blocks.
        assert!(!may_lack_block_counts("fuseblk"));
    }

    #[test]
    fn classifies_virtual_and_snapshot_dirs() {
        assert!(is_virtual("proc") && is_virtual("sysfs"));
        assert!(!is_virtual("ext4") && !is_virtual("tmpfs"));
        assert!(is_snapshot_dir(Path::new("/.snapshots")));
        assert!(!is_snapshot_dir(Path::new("/home")));
    }
}
