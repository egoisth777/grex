//! File-scoped read/write lock backed by `fd-lock`.
//!
//! # Semantics
//!
//! Both Unix and Windows lock a **dedicated sidecar file** (`lock_path`),
//! not the manifest itself. Cooperating parties serialize through the
//! sidecar; reads and writes to the manifest happen by path inside the
//! critical section.
//!
//! * **Unix**: advisory `flock(2)`-style lock.
//! * **Windows**: `LockFileEx` — mandatory on the locked handle. Because
//!   the locked handle is the sidecar (not the manifest), the mandatory
//!   semantics do **not** propagate to the manifest file itself.
//!
//! # Known gap (Windows)
//!
//! A non-grex writer on Windows that opens the manifest directly — bypassing
//! `ManifestLock` — is **not** blocked. An earlier attempt (Fix 4) tried to
//! lock the manifest handle itself so `LockFileEx` would exclude bypass
//! writers, but that breaks our cooperating `append_event(path)` API, which
//! reopens the manifest inside the critical section; the in-process append
//! would itself be denied by the mandatory byte-range lock. Until the append
//! API changes to write through the locked handle, the Windows lock is
//! effectively advisory for bypass writers. Pinned by
//! `windows_advisory_vs_mandatory_lock`.
//!
//! The lock is released when the held guard is dropped.

use fd_lock::RwLock;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::Path;

/// A cross-process lock guarding manifest reads and writes.
///
/// Open once per process, then call [`ManifestLock::read`] /
/// [`ManifestLock::write`] around the critical section.
pub struct ManifestLock {
    inner: RwLock<File>,
}

impl ManifestLock {
    /// Open a [`ManifestLock`] that serializes on `lock_path`.
    ///
    /// `manifest_path` is accepted for API symmetry with the write path and
    /// for a future migration to locking the manifest handle directly. It
    /// is currently unused.
    pub fn open(manifest_path: &Path, lock_path: &Path) -> io::Result<Self> {
        let _ = manifest_path;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        Ok(Self { inner: RwLock::new(file) })
    }

    /// Run `f` while holding a **shared** read lock.
    ///
    /// Blocks until the lock is acquired. Other readers may hold the lock
    /// concurrently; writers are excluded.
    pub fn read<R>(&mut self, f: impl FnOnce() -> R) -> io::Result<R> {
        let _guard = self.inner.read()?;
        Ok(f())
    }

    /// Run `f` while holding an **exclusive** write lock.
    ///
    /// Blocks until the lock is acquired. All other readers and writers are
    /// excluded.
    pub fn write<R>(&mut self, f: impl FnOnce() -> R) -> io::Result<R> {
        let _guard = self.inner.write()?;
        Ok(f())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn open_creates_lock_file() {
        let dir = tempdir().unwrap();
        let m = dir.path().join("grex.jsonl");
        let p = dir.path().join(".grex.lock");
        let _l = ManifestLock::open(&m, &p).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn read_runs_closure() {
        let dir = tempdir().unwrap();
        let m = dir.path().join("grex.jsonl");
        let p = dir.path().join(".grex.lock");
        let mut l = ManifestLock::open(&m, &p).unwrap();
        let v = l.read(|| 42u32).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn write_runs_closure() {
        let dir = tempdir().unwrap();
        let m = dir.path().join("grex.jsonl");
        let p = dir.path().join(".grex.lock");
        let mut l = ManifestLock::open(&m, &p).unwrap();
        let v = l.write(|| "ok").unwrap();
        assert_eq!(v, "ok");
    }
}
