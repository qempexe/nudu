//! Deleting files and folders, and keeping the scanned tree in sync.
//!
//! After a delete we don't rescan everything: we take the entry out of its
//! folder and subtract its sizes from every folder above it. If a delete
//! only partly worked (say, a protected file inside), we rescan just that
//! one entry so the numbers stay true.
//!
//! Folders are removed by `remove_tree`, which never crosses into another
//! mounted filesystem.

use std::fs;
use std::io;
use std::path::Path;

use crate::mounts::MountTable;
use crate::tree;
use crate::scan::{self, Entry, Kind, Progress, ScanOptions};

/// What a delete did, for the status message.
pub struct Outcome {
    /// Bytes on disk that are now free.
    pub freed: u64,
    /// Set when something could not be removed.
    pub error: Option<io::Error>,
}

/// Deletes the child at `index` of the folder reached through `trail`
/// (child indexes from `root` down, see app.rs), on disk and in the tree.
pub fn delete(
    root: &mut Entry,
    trail: &[usize],
    dir_path: &Path,
    index: usize,
    opts: &ScanOptions,
) -> Outcome {
    // Copy out what we need, so the tree is free to change afterwards.
    let (name, kind, before) = {
        let entry = &tree::folder_mut(root, trail).children[index];
        (entry.name.clone(), entry.kind, entry.disk)
    };
    let path = dir_path.join(&name);

    let result = match kind {
        Kind::Dir => remove_tree(&path, &MountTable::load()),
        _ => fs::remove_file(&path),
    };

    match result {
        Ok(()) => {
            tree::remove_child(root, trail, index);
            Outcome { freed: before, error: None }
        }
        // Something is left (permissions, files in use...). Look again at
        // just this entry to learn what survived.
        Err(error) => {
            let freed = match rescan(&path, &name, opts) {
                Some(survivor) => {
                    let after = survivor.disk;
                    tree::replace_child(root, trail, index, survivor);
                    before.saturating_sub(after)
                }
                None => {
                    // It is gone after all.
                    tree::remove_child(root, trail, index);
                    before
                }
            };
            Outcome { freed, error: Some(error) }
        }
    }
}

/// Removes a folder and everything in it, like `rm -rf`, with two
/// differences from `std::fs::remove_dir_all`:
///
/// - it keeps going after an error, so everything that *can* go, goes,
///   and reports the first error at the end;
/// - it never enters a mount point, so deleting a folder that has a USB
///   drive mounted inside cannot wipe that drive.
fn remove_tree(path: &Path, mounts: &MountTable) -> io::Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if !meta.is_dir() {
        // Files, and symlinks themselves (never what they point to).
        return fs::remove_file(path);
    }
    if mounts.at(path).is_some() {
        return Err(io::Error::other(format!(
            "{} is a mount point, left untouched",
            path.display()
        )));
    }
    let mut first_error = None;
    for item in fs::read_dir(path)? {
        let result = item.and_then(|item| remove_tree(&item.path(), mounts));
        if let Err(e) = result {
            first_error.get_or_insert(e);
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => fs::remove_dir(path),
    }
}

fn rescan(path: &Path, name: &std::ffi::OsStr, opts: &ScanOptions) -> Option<Entry> {
    let progress = Progress::default();
    let mut entry = scan::scan(path, opts.clone(), &progress).ok()??;
    crate::owners::assign(&mut entry, path);
    entry.name = name.to_owned();
    Some(entry)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deletes_real_files_and_folders() {
        let base = std::env::temp_dir().join(format!("nudu-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("keep")).unwrap();
        fs::create_dir_all(base.join("junk/deep")).unwrap();
        fs::write(base.join("junk/deep/file"), vec![0u8; 50_000]).unwrap();
        fs::write(base.join("keep/file"), vec![0u8; 10_000]).unwrap();

        let opts = ScanOptions { one_filesystem: true, include: Vec::new() };
        let progress = Progress::default();
        let mut root = scan::scan(&base, opts.clone(), &progress).unwrap().unwrap();
        let junk = root.children.iter().position(|c| c.name == "junk").unwrap();
        let junk_size = root.children[junk].disk;
        let total = root.disk;

        let outcome = delete(&mut root, &[], &base, junk, &opts);
        assert!(outcome.error.is_none());
        assert_eq!(outcome.freed, junk_size);
        assert_eq!(root.disk, total - junk_size);
        assert!(!base.join("junk").exists());
        assert!(base.join("keep/file").exists());

        fs::remove_dir_all(&base).unwrap();
    }
}
