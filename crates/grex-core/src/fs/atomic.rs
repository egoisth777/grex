//! Atomic file replacement via temp-file + rename.
//!
//! The temp file is always created in the **same directory** as the target so
//! the final `rename` stays on the same filesystem (required for atomicity on
//! POSIX; `MoveFileExW` handles same-volume atomic replace on Windows).
//!
//! # Symlink handling
//!
//! * **Unix**: if `path` is a symlink, we resolve it via `fs::canonicalize`
//!   and write to the resolved pointee. The symlink itself is preserved; its
//!   target file is replaced atomically. This matches what most tools expect
//!   when writing to a path that happens to be a link.
//! * **Windows**: symlinks are uncommon and require elevated privileges by
//!   default. Current behavior is preserved: `rename` replaces the link with
//!   a regular file. A `tracing::warn!` is emitted when this happens so the
//!   caller can notice.
//!
//! # Concurrent writers
//!
//! The temp path is uniquified per writer using pid + monotonic nanos so two
//! processes/threads writing to the same target cannot step on each other's
//! temp file. Each writer gets its own `<path>.tmp.<pid>.<nanos>`; the final
//! rename still wins atomically.
//!
//! # Crash safety
//!
//! * If a crash happens before `rename`, the original file (if any) is
//!   untouched. A partially written `.tmp.<pid>.<nanos>` may remain — callers
//!   may choose to clean it on the next open, but leaving it is safe.
//! * `rename` on an existing target is atomic on all supported platforms
//!   (Linux/macOS/Windows). Readers either see the old or new contents,
//!   never a mix.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-local monotonic tiebreaker for the temp-path suffix.
///
/// System clock resolution is coarse on some platforms (Windows in particular,
/// where `SystemTime::now()` may advance in ~15 ms steps). Two temp paths
/// minted inside the same tick would collide; this counter breaks the tie.
static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomically replace `path` with `bytes`.
///
/// Writes to a uniquified sibling temp file then renames into place. The
/// parent directory must exist; it will **not** be created.
///
/// On Unix, if `path` is a symlink, its pointee is resolved and replaced —
/// the symlink itself is preserved. On Windows the symlink is replaced with
/// a regular file (with a `tracing::warn!`).
///
/// # Errors
///
/// Returns [`io::Error`] if the write or rename fails.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_target(path);
    let tmp = tmp_path(&target);
    // Write to the uniquified temp — prior-crash leftovers from OTHER
    // writers have different suffixes, so we don't touch them.
    fs::write(&tmp, bytes)?;
    // `fs::rename` replaces the target atomically on same-volume paths.
    match fs::rename(&tmp, &target) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Rename failed — don't leave garbage around.
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Resolve the target path to write.
///
/// On Unix, a symlink is canonicalized so the pointee is replaced, not the
/// link. On Windows or if canonicalization fails (e.g. the target does not
/// exist yet), `path` is used as-is.
#[cfg(unix)]
fn resolve_target(path: &Path) -> PathBuf {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => match fs::canonicalize(path) {
            Ok(resolved) => resolved,
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "atomic_write: failed to canonicalize symlink; replacing link with regular file"
                );
                path.to_path_buf()
            }
        },
        _ => path.to_path_buf(),
    }
}

#[cfg(windows)]
fn resolve_target(path: &Path) -> PathBuf {
    if let Ok(meta) = fs::symlink_metadata(path) {
        if meta.file_type().is_symlink() {
            tracing::warn!(
                path = %path.display(),
                "atomic_write on Windows replaces symlink with a regular file; pointee untouched"
            );
        }
    }
    path.to_path_buf()
}

/// Build a uniquified sibling temp path `<path>.tmp.<pid>.<nanos>.<ctr>`.
///
/// `pid` separates processes; `nanos` + `ctr` separate writers within one
/// process even when the system clock has coarse resolution.
fn tmp_path(path: &Path) -> PathBuf {
    let pid = std::process::id();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let ctr = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".tmp.{pid}.{nanos:x}.{ctr:x}"));
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
        // No specific tmp name is predictable anymore (pid/nanos/ctr suffix),
        // but no tmp file from OUR writer should remain after a successful
        // atomic_write — only the final target should exist in the parent.
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        atomic_write(&p, b"x").unwrap();
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();
        assert_eq!(entries, vec![std::ffi::OsString::from("a.txt")]);
    }

    #[test]
    fn stale_temp_from_prior_crash_does_not_block_write() {
        // A prior-crash leftover has a different pid/nanos suffix than the
        // current writer, so atomic_write must not care about it: the new
        // write uses a fresh suffix and succeeds regardless.
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        // Hand-construct a plausible prior-crash temp path.
        let mut stale = p.as_os_str().to_owned();
        stale.push(".tmp.99999.deadbeef.0");
        let stale = PathBuf::from(stale);
        fs::write(&stale, b"garbage").unwrap();
        atomic_write(&p, b"fresh").unwrap();
        assert_eq!(fs::read(&p).unwrap(), b"fresh");
        // The unrelated stale file is left alone — that's the explicit
        // design trade-off for crash safety with concurrent writers.
        assert!(stale.exists(), "stale temp from a foreign writer is left untouched");
    }

    #[test]
    fn tmp_paths_are_unique_per_call() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        let t1 = tmp_path(&p);
        let t2 = tmp_path(&p);
        assert_ne!(t1, t2, "consecutive tmp_path calls must differ");
    }
}
