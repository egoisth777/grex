//! File-scoped read/write lock backed by `fd-lock`.
//!
//! # Semantics
//!
//! * **Unix**: advisory `flock(2)`-style lock on the file descriptor. Shared
//!   (read) and exclusive (write) modes.
//! * **Windows**: `LockFileEx` — mandatory lock at the OS level. Same shared
//!   vs. exclusive semantics.
//!
//! The lock is released when the held guard is dropped. `ManifestLock` owns
//! the backing file; the lock path is **separate** from the data file to keep
//! lock contention off the hot read path.

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
    /// Open (and create if missing) the lock file at `lock_path`.
    ///
    /// The file itself holds no data — only the OS lock state on its fd.
    pub fn open(lock_path: &Path) -> io::Result<Self> {
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
        let p = dir.path().join(".grex.lock");
        let _l = ManifestLock::open(&p).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn read_runs_closure() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".grex.lock");
        let mut l = ManifestLock::open(&p).unwrap();
        let v = l.read(|| 42u32).unwrap();
        assert_eq!(v, 42);
    }

    #[test]
    fn write_runs_closure() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".grex.lock");
        let mut l = ManifestLock::open(&p).unwrap();
        let v = l.write(|| "ok").unwrap();
        assert_eq!(v, "ok");
    }
}
