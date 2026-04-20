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
use std::path::{Path, PathBuf};

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

/// A non-blocking cross-process exclusive lock used to serialise
/// operations on a path-keyed resource (a workspace, a per-repo directory).
///
/// Unlike [`ManifestLock`] (which blocks on contention because the critical
/// section is small and cooperating), `ScopedLock` uses `try_lock_write` and
/// surfaces the busy condition to the caller. Callers decide whether to
/// fail fast or retry.
///
/// The lock file is created (`O_CREAT`) if missing and kept open for the
/// lifetime of the struct. A `.lock` suffix is conventional but not required
/// — any path will do. The lock is released on drop.
///
/// # Layering vs `ManifestLock`
///
/// `ManifestLock` wraps a blocking read/write critical section around a
/// manifest append path. `ScopedLock` is a try-lock guard held for an entire
/// operation (e.g. `sync::run`, `GixBackend::checkout`) where waiting would
/// be the wrong UX — the user likely launched two processes by accident and
/// needs to see the collision, not block on a second terminal they forgot
/// about.
pub struct ScopedLock {
    inner: RwLock<File>,
    path: PathBuf,
}

impl ScopedLock {
    /// Open (and create if missing) the sidecar lock file at `lock_path`.
    /// Does **not** acquire the lock — call [`ScopedLock::try_acquire`].
    ///
    /// # Errors
    ///
    /// Returns any [`io::Error`] from `OpenOptions::open`.
    pub fn open(lock_path: &Path) -> io::Result<Self> {
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)?;
        Ok(Self { inner: RwLock::new(file), path: lock_path.to_path_buf() })
    }

    /// Acquire the exclusive write lock, blocking until it is free.
    ///
    /// Use for per-resource serialisation where the right behaviour under
    /// contention is to wait (e.g. two syncs both wanting to `fetch` the
    /// same clone — the second simply runs after the first finishes).
    ///
    /// # Errors
    ///
    /// Propagates any OS-level lock error from `fd-lock`.
    pub fn acquire(&mut self) -> io::Result<fd_lock::RwLockWriteGuard<'_, File>> {
        self.inner.write()
    }

    /// Try to acquire the exclusive write lock without blocking.
    ///
    /// Returns `Ok(Some(guard))` on success, `Ok(None)` if another process /
    /// thread already holds the lock, or `Err(e)` on an unexpected OS error.
    pub fn try_acquire(&mut self) -> io::Result<Option<fd_lock::RwLockWriteGuard<'_, File>>> {
        match self.inner.try_write() {
            Ok(guard) => Ok(Some(guard)),
            Err(e) => {
                // fd-lock exposes the contended condition via `WouldBlock`
                // on Unix and `ERROR_LOCK_VIOLATION`/`WouldBlock` on Windows.
                // Map both to `Ok(None)` so callers distinguish "busy" from
                // "I/O went wrong".
                if e.kind() == io::ErrorKind::WouldBlock {
                    Ok(None)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Return the filesystem path of the sidecar lock file. Useful for
    /// error messages — e.g. "remove `<path>` if stale".
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for ScopedLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScopedLock").field("path", &self.path).finish()
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

    #[test]
    fn scoped_lock_creates_parent() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("nested").join(".grex.sync.lock");
        let _l = ScopedLock::open(&p).unwrap();
        assert!(p.exists());
    }

    #[test]
    fn scoped_lock_try_acquire_succeeds_once() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".grex.sync.lock");
        let mut l = ScopedLock::open(&p).unwrap();
        let g = l.try_acquire().unwrap();
        assert!(g.is_some(), "first acquire must succeed");
    }

    #[test]
    fn scoped_lock_second_acquire_reports_busy() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".grex.sync.lock");
        let mut l1 = ScopedLock::open(&p).unwrap();
        let mut l2 = ScopedLock::open(&p).unwrap();
        let _g1 = l1.try_acquire().unwrap().expect("first acquires");
        let g2 = l2.try_acquire().unwrap();
        assert!(g2.is_none(), "second acquire must report busy while first held");
    }

    #[test]
    fn scoped_lock_reacquire_after_drop() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".grex.sync.lock");
        let mut l1 = ScopedLock::open(&p).unwrap();
        {
            let _g = l1.try_acquire().unwrap().expect("held");
        }
        let mut l2 = ScopedLock::open(&p).unwrap();
        let g2 = l2.try_acquire().unwrap();
        assert!(g2.is_some(), "lock reacquires after first guard drops");
    }
}
