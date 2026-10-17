//! Walks a directory tree and builds an in-memory model of it.
//!
//! Every file and directory becomes an [`Entry`]. Directories hold their
//! children and carry the summed sizes of everything below them, so the UI
//! can show totals without recomputing anything.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

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
    pub children: Vec<Entry>,
}

#[derive(Debug, Clone)]
pub struct ScanOptions {
    /// Stay on the filesystem of the starting directory (like `ncdu -x`).
    pub one_filesystem: bool,
}

/// State shared across the whole walk.
struct Walker {
    opts: ScanOptions,
    root_dev: u64,
    /// (device, inode) pairs of hard-linked files we have already counted.
    seen_links: HashSet<(u64, u64)>,
}

/// Scans `path` and returns the root entry of the tree.
pub fn scan(path: &Path, opts: ScanOptions) -> std::io::Result<Entry> {
    let meta = fs::symlink_metadata(path)?;
    let mut walker = Walker {
        opts,
        root_dev: meta.dev(),
        seen_links: HashSet::new(),
    };
    let name = path.as_os_str().to_owned();
    Ok(walker.visit(path, name, &meta))
}

impl Walker {
    fn visit(&mut self, path: &Path, name: OsString, meta: &fs::Metadata) -> Entry {
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
            children: Vec::new(),
        };

        // A file with several hard links shares one copy of its data.
        // Count it the first time we meet it, and as zero afterwards.
        if kind != Kind::Dir && meta.nlink() > 1 {
            if !self.seen_links.insert((meta.dev(), meta.ino())) {
                entry.apparent = 0;
                entry.disk = 0;
            }
        }

        if kind == Kind::Dir {
            let crosses_fs = meta.dev() != self.root_dev;
            if !(self.opts.one_filesystem && crosses_fs) {
                self.read_children(path, &mut entry);
            }
        }

        entry
    }

    fn read_children(&mut self, path: &Path, dir: &mut Entry) {
        let reader = match fs::read_dir(path) {
            Ok(r) => r,
            Err(_) => {
                dir.error = true;
                return;
            }
        };

        for item in reader {
            let Ok(item) = item else {
                dir.error = true;
                continue;
            };
            let child_path = item.path();
            // symlink_metadata: never follow symlinks, just like ncdu.
            let Ok(meta) = fs::symlink_metadata(&child_path) else {
                dir.error = true;
                continue;
            };
            let child = self.visit(&child_path, item.file_name(), &meta);

            dir.apparent += child.apparent;
            dir.disk += child.disk;
            dir.items += child.items + 1;
            dir.mtime = dir.mtime.max(child.mtime);
            dir.error |= child.error;
            dir.children.push(child);
        }

        // Biggest first: the default ncdu order.
        dir.children.sort_by(|a, b| b.disk.cmp(&a.disk));
    }
}
