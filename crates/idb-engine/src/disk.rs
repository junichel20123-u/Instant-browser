//! Disk space checks and filesystem-aware preallocation.

use std::fs::File;
use std::path::Path;

use rustix::fs::{FallocateFlags, fallocate, fstatfs, statvfs};

const EXT4: u32 = 0xEF53;
const XFS: u32 = 0x5846_5342;
const BTRFS: u32 = 0x9123_683E;
const TMPFS: u32 = 0x0102_1994;

/// Bytes available to an unprivileged user on the filesystem holding `dir`.
pub fn free_bytes(dir: &Path) -> std::io::Result<u64> {
    let st = statvfs(dir)?;
    Ok(st.f_bavail.saturating_mul(st.f_frsize))
}

/// Whether `needed` bytes (plus a safety margin) fit in `dir`.
pub fn has_room(dir: &Path, needed: u64, margin: u64) -> std::io::Result<bool> {
    Ok(free_bytes(dir)? >= needed.saturating_add(margin))
}

/// Filesystems where `fallocate` is fast and does not block for long.
fn fast_fallocate(file: &File) -> bool {
    // `f_type`'s integer type differs across targets; the magic fits in u32.
    fstatfs(file).is_ok_and(|st| {
        let kind = st.f_type as u32;
        matches!(kind, EXT4 | XFS | BTRFS | TMPFS)
    })
}

/// Sets the final length and, on filesystems that support it cheaply,
/// reserves the blocks with `fallocate`. Other filesystems (FAT, exFAT,
/// NTFS-3G, FUSE) only get `set_len`, which never blocks for minutes.
/// Run it off the async runtime.
pub fn preallocate(file: &File, len: u64) -> std::io::Result<()> {
    file.set_len(len)?;
    if len > 0
        && fast_fallocate(file)
        && let Err(e) = fallocate(file, FallocateFlags::empty(), 0, len)
    {
        tracing::debug!("fallocate not used: {e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preallocates_and_reports_space() {
        let d = tempfile::tempdir().unwrap();
        let f = File::create(d.path().join("x")).unwrap();
        preallocate(&f, 1 << 20).unwrap();
        assert_eq!(f.metadata().unwrap().len(), 1 << 20);
        assert!(free_bytes(d.path()).unwrap() > 0);
        assert!(!has_room(d.path(), u64::MAX / 2, 0).unwrap());
        assert!(has_room(d.path(), 1, 0).unwrap());
    }
}
