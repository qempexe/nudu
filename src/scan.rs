//! Walks a directory tree and builds an in-memory model of it.
//!
//! Every file and directory becomes an [`Entry`]. Directories hold their
//! children and carry the summed sizes of everything below them, so the UI
//! can show totals without recomputing anything.
//!
//! The walk runs on all CPU cores: the entries of each directory are
//! examined in parallel with rayon, and so are their subdirectories.
//!
//! When a folder sits on a different device than its parent, mounts.rs
//! decides whether it is still the same filesystem (a btrfs subvolume like
//! `/home`) or something to skip (another disk, `/proc`, snapshots).

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rayon::prelude::*;

use crate::age::{self, AgeTotals, Bucket};
use crate::filetype::{self, Category, TypeTotals};
use crate::mounts::{self, Mount, MountTable};
use crate::owners::Owner;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug)]
pub struct Entry {
    pub name: OsString,
    pub kind: Kind,
    /// What `ls -l` reports: the file's logical length.
    pub apparent: u64,
    /// Space actually allocated on disk (blocks * 512).
    pub disk: u64,
    /// Last modification time, seconds since the Unix epoch.
    pub mtime: i64,
    /// Number of entries below this one (for directories).
    pub items: u64,
    /// True if we could not read this entry (or part of it).
    pub error: bool,
    /// Set on folders we deliberately did not descend into.
    pub skipped: Option<Skipped>,
    /// Set on folders where another filesystem is mounted (its type,
    /// like `ext4`), whether we scanned into it or not.
    pub other_fs: Option<String>,
    /// For folders: bytes on disk per kind of file, everything below.
    /// Boxed so files, the vast majority of entries, stay small.
    pub types: Option<Box<TypeTotals>>,
    /// For folders: bytes on disk per age bucket, everything below.
    pub ages: Option<Box<AgeTotals>>,
    /// Who this belongs to (filled in by owners.rs after the scan).
    pub owner: Owner,
    pub children: Vec<Entry>,
}

impl Entry {
    /// Bytes on disk per kind of file, for this entry and all below it.
    pub fn type_totals(&self) -> TypeTotals {
        if let Some(types) = &self.types {
            return **types;
        }
        let mut totals = TypeTotals::default();
        if self.kind != Kind::Dir {
            totals[Category::of(&self.name) as usize] = self.disk;
        }
        totals
    }

    /// Bytes on disk per age bucket, for this entry and all below it.
    pub fn age_totals(&self) -> AgeTotals {
        if let Some(ages) = &self.ages {
            return **ages;
        }
        let mut totals = AgeTotals::default();
        if self.kind != Kind::Dir {
            totals[Bucket::of(self.mtime) as usize] = self.disk;
        }
        totals
    }
}

/// How full a mounted filesystem is, read without scanning it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountUsage {
    pub fs_type: String,
    pub used: u64,
    pub total: u64,
}

/// Why a folder was not scanned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skipped {
    /// Another filesystem is mounted here. Enter scans it on demand.
    OtherFs(MountUsage),
    /// A kernel filesystem with no real files, like `proc`.
    Virtual(String),
    /// btrfs snapshots: their data is shared with the live files.
    Snapshots,
}

#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// Stay on the filesystem of the starting directory (like `du -x`).
    pub one_filesystem: bool,
    /// Mount points to scan even when `one_filesystem` is set: drives the
    /// user opened, so a rescan keeps showing their contents.
    pub include: Vec<PathBuf>,
}

/// Live counters the UI reads while a scan is running.
/// Every field is safe to read and write from any thread.
#[derive(Default)]
pub struct Progress {
    pub items: AtomicU64,
    pub bytes: AtomicU64,
    pub errors: AtomicU64,
    /// The directory most recently entered, for display.
    pub current: Mutex<PathBuf>,
    /// Set by the UI to stop the scan early.
    pub cancel: AtomicBool,
}

impl Progress {
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// What to do at a filesystem boundary.
enum Crossing {
    /// Go in. `other_fs` names the filesystem if it differs from the start.
    Scan { other_fs: Option<String> },
    Skip(Skipped),
}

/// Reads how full the filesystem mounted at `path` is (one statvfs call).
pub fn mount_usage(path: &Path, fs_type: &str) -> MountUsage {
    let (used, total) = rustix::fs::statvfs(path)
        .map(|s| {
            let total = s.f_blocks * s.f_frsize;
            (total.saturating_sub(s.f_bfree * s.f_frsize), total)
        })
        .unwrap_or((0, 0));
    MountUsage {
        fs_type: fs_type.to_string(),
        used,
        total,
    }
}

/// State shared by all threads during the walk.
struct Walker<'a> {
    opts: ScanOptions,
    mounts: MountTable,
    /// The mount the scan started in.
    root_mount: Option<Mount>,
    /// (device, inode) pairs of hard-linked files we have already counted.
    seen_links: Mutex<HashSet<(u64, u64)>>,
    progress: &'a Progress,
}

/// Scans `path` and returns the root entry of the tree.
/// Returns `None` if the scan was cancelled through `progress`.
pub fn scan(path: &Path, opts: ScanOptions, progress: &Progress) -> std::io::Result<Option<Entry>> {
    let meta = fs::symlink_metadata(path)?;
    let mounts = MountTable::load();
    let walker = Walker {
        opts,
        root_mount: mounts.containing(path).cloned(),
        mounts,
        seen_links: Mutex::new(HashSet::new()),
        progress,
    };
    let name = path.as_os_str().to_owned();
    let root = walker.visit(path, name, &meta, meta.dev());
    Ok((!progress.cancelled()).then_some(root))
}

impl Walker<'_> {
    /// `parent_dev` is the device of the folder containing `path`; a change
    /// of device means we are crossing into another mount or subvolume.
    fn visit(&self, path: &Path, name: OsString, meta: &fs::Metadata, parent_dev: u64) -> Entry {
        let ft = meta.file_type();
        let kind = if ft.is_dir() {
            Kind::Dir
        } else if ft.is_symlink() {
            Kind::Symlink
        } else if ft.is_file() {
            Kind::File
        } else {
            Kind::Other
        };

        let mut entry = Entry {
            name,
            kind,
            apparent: meta.len(),
            disk: meta.blocks() * 512,
            mtime: meta.mtime(),
            items: 0,
            error: false,
            skipped: None,
            other_fs: None,
            types: None,
            ages: None,
            owner: Owner::None,
            children: Vec::new(),
        };

        // A file with several hard links shares one copy of its data.
        // Count it the first time we meet it, and as zero afterwards.
        if kind != Kind::Dir && meta.nlink() > 1 {
            let first_time = self
                .seen_links
                .lock()
                .unwrap()
                .insert((meta.dev(), meta.ino()));
            if !first_time {
                entry.apparent = 0;
                entry.disk = 0;
            }
        }

        self.progress.items.fetch_add(1, Ordering::Relaxed);
        self.progress.bytes.fetch_add(entry.disk, Ordering::Relaxed);

        if kind == Kind::Dir {
            if meta.dev() != parent_dev {
                match self.boundary(path) {
                    Crossing::Scan { other_fs } => entry.other_fs = other_fs,
                    Crossing::Skip(why) => entry.skipped = Some(why),
                }
            }
            if entry.skipped.is_none() {
                self.read_children(path, &mut entry, meta.dev());
            }
        }

        entry
    }

    /// Called when `path` is on another device than its parent.
    fn boundary(&self, path: &Path) -> Crossing {
        if mounts::is_snapshot_dir(path) {
            return Crossing::Skip(Skipped::Snapshots);
        }
        let Some(mount) = self.mounts.at(path) else {
            // A new device without a mount: a btrfs subvolume nested inside
            // the tree (like /var/lib/machines). Same filesystem, so scan it.
            return Crossing::Scan { other_fs: None };
        };
        if mounts::is_virtual(&mount.fs_type) {
            return Crossing::Skip(Skipped::Virtual(mount.fs_type.clone()));
        }
        let same = self
            .root_mount
            .as_ref()
            .is_some_and(|root| mounts::same_filesystem(root, mount));
        if same {
            Crossing::Scan { other_fs: None }
        } else if !self.opts.one_filesystem || self.opts.include.iter().any(|p| p == path) {
            Crossing::Scan { other_fs: Some(mount.fs_type.clone()) }
        } else {
            Crossing::Skip(Skipped::OtherFs(mount_usage(path, &mount.fs_type)))
        }
    }

    fn read_children(&self, path: &Path, dir: &mut Entry, dev: u64) {
        if self.progress.cancelled() {
            return;
        }
        // try_lock: if another thread is updating it right now, just skip.
        if let Ok(mut current) = self.progress.current.try_lock() {
            current.clear();
            current.push(path);
        }

        let reader = match fs::read_dir(path) {
            Ok(r) => r,
            Err(_) => {
                self.mark_error(dir);
                return;
            }
        };

        // Listing a directory is quick; looking at each entry (stat) and
        // descending into subdirectories is the slow part, so that part
        // runs in parallel.
        let mut names = Vec::new();
        for item in reader {
            match item {
                Ok(item) => names.push(item.file_name()),
                Err(_) => self.mark_error(dir),
            }
        }

        let mut types = TypeTotals::default();
        let mut ages = AgeTotals::default();
        let results: Vec<Option<Entry>> = names
            .into_par_iter()
            .map(|name| {
                let child_path = path.join(&name);
                // symlink_metadata: never follow symlinks, so nothing is counted twice.
                let meta = fs::symlink_metadata(&child_path).ok()?;
                Some(self.visit(&child_path, name, &meta, dev))
            })
            .collect();

        for result in results {
            let Some(child) = result else {
                self.mark_error(dir);
                continue;
            };
            dir.apparent += child.apparent;
            dir.disk += child.disk;
            dir.items += child.items + 1;
            dir.mtime = dir.mtime.max(child.mtime);
            dir.error |= child.error;
            filetype::add(&mut types, &child.type_totals());
            age::add(&mut ages, &child.age_totals());
            dir.children.push(child);
        }

        dir.types = Some(Box::new(types));
        dir.ages = Some(Box::new(ages));

        // Biggest first: the default order.
        dir.children.sort_by(|a, b| b.disk.cmp(&a.disk));
    }

    fn mark_error(&self, dir: &mut Entry) {
        dir.error = true;
        self.progress.errors.fetch_add(1, Ordering::Relaxed);
    }
}
