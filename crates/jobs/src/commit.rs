//! Atomic, no-replace commit of a finished output.

use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

/// Move `tmp` to `dst` without ever replacing an existing `dst`.
///
/// Primary path: `hard_link` fails with `AlreadyExists` if `dst` exists, and the link
/// appears atomically, so readers never see a partial file. Then the temp name is removed.
/// Works on NTFS, ReFS, ext4, Btrfs, XFS and APFS.
///
/// Fallback for file systems without hard links (FAT/exFAT, some network shares): claim the
/// name with `create_new` (atomic O_EXCL / CREATE_NEW), then rename over our own placeholder.
pub fn commit_no_replace(tmp: &Path, dst: &Path) -> io::Result<()> {
    match fs::hard_link(tmp, dst) {
        Ok(()) => {
            let _ = fs::remove_file(tmp);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
        Err(_) => {
            OpenOptions::new().write(true).create_new(true).open(dst)?;
            fs::rename(tmp, dst).inspect_err(|_| {
                let _ = fs::remove_file(dst);
            })
        }
    }
}

/// Where outputs go when the source folder isn't writable: `Downloads/Swirvo`.
pub fn fallback_dir() -> io::Result<PathBuf> {
    let home = std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .ok_or_else(|| io::Error::other("no home folder"))?;
    let dir = PathBuf::from(home).join("Downloads").join("Swirvo");
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_replace() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, dst) = (d.path().join("t"), d.path().join("d"));
        fs::write(&tmp, b"new").unwrap();
        fs::write(&dst, b"old").unwrap();
        assert_eq!(
            commit_no_replace(&tmp, &dst).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert_eq!(fs::read(&dst).unwrap(), b"old");
        fs::remove_file(&dst).unwrap();
        commit_no_replace(&tmp, &dst).unwrap();
        assert_eq!(fs::read(&dst).unwrap(), b"new");
        assert!(!tmp.exists());
    }
}
