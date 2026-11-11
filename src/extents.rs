//! Which parts of a file share their data with other files.
//!
//! On btrfs (and XFS with reflinks), a snapshot or a `cp --reflink` copy
//! does not duplicate data: both files point at the same blocks on disk.
//! `du` counts those blocks once per file, so totals look bigger than the
//! disk really is, and deleting such a file frees nothing while the other
//! copy remains.
//!
//! The kernel tells us which pieces ("extents") of a file are shared
//! through the FIEMAP ioctl. It works for normal users, no root needed,
//! on any file they can open for reading.

use std::fs::File;
use std::io;
use std::os::fd::AsRawFd;
use std::path::Path;

/// `_IOWR('f', 11, struct fiemap)`: ask for a file's extent map.
const FS_IOC_FIEMAP: libc::c_ulong = 0xC020_660B;
/// This extent is the last one of the file.
const FIEMAP_EXTENT_LAST: u32 = 0x1;
/// This extent's data is also used by another file (snapshot, reflink).
const FIEMAP_EXTENT_SHARED: u32 = 0x2000;

/// How many extents we ask for per call.
const BATCH: usize = 64;

/// One extent, exactly as the kernel lays it out (`struct fiemap_extent`).
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Extent {
    logical: u64,
    physical: u64,
    length: u64,
    _reserved64: [u64; 2],
    flags: u32,
    _reserved: [u32; 3],
}

/// The request header (`struct fiemap`) followed by room for the answer.
#[repr(C)]
struct Request {
    start: u64,
    length: u64,
    flags: u32,
    mapped_extents: u32,
    extent_count: u32,
    _reserved: u32,
    extents: [Extent; BATCH],
}

/// Filesystems where data can be shared between files.
pub fn can_share(fs_type: &str) -> bool {
    matches!(fs_type, "btrfs" | "xfs" | "bcachefs")
}

/// Bytes of `path`'s data that are shared with other files, capped at
/// `disk` (its space on disk, which can be less for compressed data).
pub fn shared_bytes(path: &Path, disk: u64) -> io::Result<u64> {
    let file = File::open(path)?;
    let mut shared = 0u64;
    let mut start = 0u64;

    loop {
        let mut req = Request {
            start,
            length: u64::MAX - start,
            // No FIEMAP_FLAG_SYNC: forcing every file to disk would make
            // the scan crawl, and data not written yet can't be shared.
            flags: 0,
            mapped_extents: 0,
            extent_count: BATCH as u32,
            _reserved: 0,
            extents: [Extent::default(); BATCH],
        };
        // SAFETY: `req` is a correctly laid out `struct fiemap` with room
        // for `extent_count` extents, and it outlives the call.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), FS_IOC_FIEMAP as _, &mut req) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }

        let found = &req.extents[..req.mapped_extents as usize];
        for extent in found {
            if extent.flags & FIEMAP_EXTENT_SHARED != 0 {
                shared += extent.length;
            }
        }
        match found.last() {
            Some(last) if last.flags & FIEMAP_EXTENT_LAST == 0 => {
                start = last.logical + last.length;
            }
            // Done: the last extent was reached, or the file has none.
            _ => break,
        }
    }
    Ok(shared.min(disk))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn struct_sizes_match_the_kernel() {
        // struct fiemap_extent is 56 bytes, struct fiemap's header 32.
        assert_eq!(std::mem::size_of::<Extent>(), 56);
        assert_eq!(std::mem::size_of::<Request>(), 32 + 56 * BATCH);
    }

    #[test]
    fn plain_files_share_nothing() {
        let path = std::env::temp_dir().join(format!("nudu-fiemap-{}", std::process::id()));
        std::fs::write(&path, vec![7u8; 100_000]).unwrap();
        // On ext4/tmpfs nothing is shared (tmpfs may not support FIEMAP at all).
        if let Ok(shared) = shared_bytes(&path, 100_000) {
            assert_eq!(shared, 0);
        }
        std::fs::remove_file(path).unwrap();
    }
}
