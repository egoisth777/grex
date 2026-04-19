//! Atomic file replacement via temp-file + rename.
//!
//! The temp file is always created in the **same directory** as the target so
//! the final `rename` stays on the same filesystem (required for atomicity on
//! POSIX; `MoveFileExW` handles same-volume atomic replace on Windows).
//!
//! # Crash safety
//!
//! * If a crash happens before `rename`, the original file (if any) is
//!   untouched. A partially written `.tmp` may remain — callers may choose to
//!   clean it on the next open, but leaving it is safe.
//! * `rename` on an existing target is atomic on all supported platforms
//!   (Linux/macOS/Windows). Readers either see the old or new contents,
//!   never a mix.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Atomically replace `path` with `bytes`.
///
/// Writes to `<path>.tmp` in the same directory then renames into place. The
/// parent directory must exist; it will **not** be created.
///
/// # Errors
///
/// Returns [`io::Error`] if the write or rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = tmp_path(path);
    // Best-effort cleanup of any stale temp from a prior crash.
    let _ = fs::remove_file(&tmp);
    fs::write(&tmp, bytes)?;
    // `fs::rename` replaces the target atomically on same-volume paths.
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Rename failed — don't leave garbage around.
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Build the sibling temp path `<path>.tmp`.
fn tmp_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn write_succeeds() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        atomic_write(&p, b"hello").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"hello");
    }

    #[test]
    fn existing_file_overwritten() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, b"old").unwrap();
        atomic_write(&p, b"new").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"new");
    }

    #[test]
    fn temp_file_cleaned_on_success() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        atomic_write(&p, b"x").unwrap();
        let tmp = tmp_path(&p);
        assert!(!tmp.exists(), "temp file should be renamed away");
    }

    #[test]
    fn stale_temp_from_prior_crash_is_replaced() {
        // Simulate a crash that left <path>.tmp around. Next atomic_write
        // must still succeed and produce correct contents.
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(tmp_path(&p), b"garbage").unwrap();
        atomic_write(&p, b"fresh").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"fresh");
        assert!(!tmp_path(&p).exists());
    }
}
