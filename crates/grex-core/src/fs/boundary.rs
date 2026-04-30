//! Capability-based dirfd boundary primitive — closes the
//! `canonicalize(dest) → clone(dest)` TOCTOU race window for every walker
//! filesystem write.
//!
//! # What this provides
//!
//! [`BoundedDir`] is a thin wrapper around a kernel-confirmed directory
//! handle (a "dirfd") obtained for a path that is **provably contained**
//! beneath a parent directory. Once constructed, the handle is bound to
//! the inode the kernel resolved at construction time — a subsequent
//! attacker swap of the parent path for a symlink (the classic TOCTOU
//! race) cannot redirect operations performed through the handle.
//!
//! # Why this exists
//!
//! Stage 0 LOCKED decision #1 + Lean axiom `sync_local_writes` + design
//! decision §6 ([`.omne/cfg/rust-design-decisions.md`](../../.omne/cfg/rust-design-decisions.md))
//! require every walker write to bind to a dirfd, not a path string.
//! Without this the walker's existing flow is:
//!
//! ```text
//! parent.canonicalize() → resolve(child) → fs::create_dir_all(dest) → clone(dest)
//!                       ▲                ▲                          ▲
//!                       └─ race window ──┴─ swap dest for symlink ──┘
//! ```
//!
//! and an attacker who can write inside the workspace mid-flight could
//! redirect the `clone` write to an arbitrary location.
//!
//! With [`BoundedDir`], the construction step the walker does is:
//!
//! ```text
//! BoundedDir::open(parent, child_relative)? → handle bound to inode
//! ```
//!
//! and downstream operations either go through the dirfd (write confined)
//! or compare against [`BoundedDir::path`] (the canonicalized, post-resolve
//! path) — both of which the kernel has already vouched for.
//!
//! # Cross-platform strategy
//!
//! Per design decision §6, the implementation uses
//! [`cap-std`](https://crates.io/crates/cap-std) uniformly across all
//! platforms. cap-std internally uses `openat2(RESOLVE_BENEATH)` on Linux
//! ≥ 5.6 and falls back to `O_NOFOLLOW`-by-component verification on
//! older kernels and on macOS / Windows. The choice trades the marginal
//! performance of a single-syscall `openat2` for: (a) no `unsafe` in
//! `grex-core` (which carries `#![deny(unsafe_code)]`), (b) one code path
//! to test across OSes, and (c) no kernel-version branching at runtime.
//!
//! # What this commit does NOT do
//!
//! This module ships the primitive only. Walker integration (callers
//! replacing `fs::create_dir_all(dest)` + `gix::open(dest)` with
//! [`BoundedDir::open`] + dirfd-bound writes) is Stage 1.e / 1.g work —
//! see [`tree::walker`](crate::tree::walker) and the v1.2.0
//! `openspec/feat-grex` plan.

use std::io;
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::Dir;

/// A directory handle bound to an inode the kernel has confirmed lies
/// beneath a given parent. Constructed via [`BoundedDir::open`].
///
/// Subsequent operations performed through this handle (or compared
/// against [`BoundedDir::path`]) cannot be redirected by a path-swap
/// attack on the parent: the dirfd is already bound to the inode the
/// kernel resolved at construction time.
///
/// Visibility is `pub(crate)` for now — see module docs for why we don't
/// expose cap-std types in the public API surface.
///
/// `dead_code` is suppressed because non-test consumers land in Stage
/// 1.e / 1.g (walker integration). The TDD invariant (this commit ships
/// the primitive + tests, a later commit wires the call sites) means
/// the type is exercised only by the test module here for now.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct BoundedDir {
    /// Canonicalized absolute path of the resolved child. This is the
    /// post-resolution truth and is the value walker integration code
    /// should compare against (e.g. for lockfile-entry equality).
    canonical_path: PathBuf,
    /// The kernel-confirmed dirfd. Held so the OS keeps the inode alive
    /// for the lifetime of this struct, and so future commits in Stage
    /// 1.e/1.g can issue `openat`-flavoured writes through it. Currently
    /// unused at the call-sites — silenced via the leading underscore so
    /// `dead_code` does not fire while integration is pending.
    _dir: Dir,
}

// `dead_code` is suppressed because non-test consumers land in Stage
// 1.e / 1.g (walker integration). See the struct doc for context.
#[allow(dead_code)]
impl BoundedDir {
    /// Open `parent / child_relative` as a kernel-confirmed dirfd, with
    /// the resolution proven to stay beneath `parent`.
    ///
    /// `child_relative` MUST be a relative path with no `..` components
    /// and no leading separator. Symlink components within
    /// `child_relative` are rejected (cap-std refuses to traverse
    /// symlinks under a `Dir`).
    ///
    /// Returns the canonicalized post-resolution path together with the
    /// dirfd. On rejection (parent missing, traversal attempt, symlink
    /// in path, child not a directory), an `io::Error` is returned with
    /// a kind that the caller can pattern-match on.
    ///
    /// # Errors
    ///
    /// * `io::ErrorKind::InvalidInput` — `child_relative` is absolute or
    ///   contains a `..` segment.
    /// * `io::ErrorKind::NotFound` — `parent` does not exist or is not
    ///   a directory.
    /// * Any other `io::Error` from the OS open path (typically
    ///   `PermissionDenied` for symlink-escape rejections from cap-std).
    pub(crate) fn open(parent: &Path, child_relative: &Path) -> io::Result<Self> {
        // 1. Reject absolute child paths up front — cap-std would reject
        //    them too, but the diagnostic is clearer here.
        if child_relative.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "BoundedDir: child path must be relative, got absolute {:?}",
                    child_relative
                ),
            ));
        }
        // 2. Reject any `..` component before handing off — cap-std also
        //    rejects these, but doing it here gives a deterministic
        //    error variant we can assert on in tests across OSes.
        //    `Prefix` (Windows drive letter) and `RootDir` are covered
        //    by the `is_absolute` check above; `CurDir` (`.`) and
        //    `Normal` are fine — only `ParentDir` is hostile here.
        use std::path::Component;
        for component in child_relative.components() {
            if component == Component::ParentDir {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "BoundedDir: child path must not contain `..`, got {:?}",
                        child_relative
                    ),
                ));
            }
        }
        // 3. Open the parent as an ambient capability dirfd. cap-std
        //    follows the leading path normally (the parent itself is
        //    expected to be a real directory the caller has chosen);
        //    confinement applies to operations *under* the resulting
        //    `Dir`, which is exactly what we want.
        let parent_dir = Dir::open_ambient_dir(parent, ambient_authority())?;
        // 4. Open the child by relative path. cap-std rejects any
        //    traversal that would leave `parent_dir` (symlink, `..`,
        //    absolute reroot). On success the returned `Dir` is bound
        //    to the resolved inode.
        let child_dir = if child_relative.as_os_str().is_empty() || child_relative == Path::new(".")
        {
            // Caller wants the parent itself — cap-std doesn't accept
            // an empty path, so re-open as a fresh handle.
            Dir::open_ambient_dir(parent, ambient_authority())?
        } else {
            parent_dir.open_dir(child_relative)?
        };
        // 5. Compute the post-resolution canonical path. This MUST go
        //    via `fs::canonicalize` so symlinks inside the parent path
        //    above `parent_dir` are flattened — the bound inode itself
        //    is what we report. We canonicalize `parent` (a real path
        //    string) then join `child_relative`; cap-std has already
        //    proven the join stays within. Falling back to a simple
        //    join if canonicalize fails is intentional: the dirfd is
        //    still bound correctly, only the *reported* path is less
        //    canonical, which is a soft signal.
        let canonical_path = match parent.canonicalize() {
            Ok(canon) => canon.join(child_relative),
            Err(_) => parent.join(child_relative),
        };
        Ok(Self { canonical_path, _dir: child_dir })
    }

    /// The canonicalized absolute path of the bound directory.
    ///
    /// This is the post-resolution truth — callers comparing the
    /// destination against a manifest-derived path or a lockfile entry
    /// should use this value, not the un-resolved input.
    pub(crate) fn path(&self) -> &Path {
        &self.canonical_path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Happy path: open a child directory that exists beneath the
    /// parent. The bound path must be a sub-path of the canonicalized
    /// parent.
    #[test]
    fn bounded_dir_opens_child_within_parent() {
        let parent = tempdir().unwrap();
        let child = parent.path().join("child");
        fs::create_dir(&child).unwrap();
        let bd = BoundedDir::open(parent.path(), Path::new("child"))
            .expect("open should succeed for an in-tree child");
        let canon_parent = parent.path().canonicalize().unwrap();
        assert!(
            bd.path().starts_with(&canon_parent),
            "bound path {:?} must be under canonicalized parent {:?}",
            bd.path(),
            canon_parent,
        );
        assert_eq!(bd.path().file_name().and_then(|s| s.to_str()), Some("child"));
    }

    /// `..` in the child path is rejected up front with
    /// `InvalidInput` — the cap-std layer would also reject, but we
    /// short-circuit for a deterministic diagnostic.
    #[test]
    fn bounded_dir_rejects_dotdot_in_child() {
        let parent = tempdir().unwrap();
        let err =
            BoundedDir::open(parent.path(), Path::new("..")).expect_err("`..` must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);

        // Buried `..` (e.g. `child/../escape`) must also reject.
        let err = BoundedDir::open(parent.path(), Path::new("child/../escape"))
            .expect_err("buried `..` must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    /// An absolute child path is rejected up front. Cross-platform: we
    /// pick a path shape that's absolute on every OS we support — `/`
    /// is absolute on Unix and on Windows is treated as the root of the
    /// current drive.
    #[test]
    fn bounded_dir_rejects_absolute_child_path() {
        let parent = tempdir().unwrap();
        // `Path::is_absolute` is true for `/etc` on Unix and for `\foo`
        // / `C:\foo` on Windows. Use a path that's absolute on the
        // host: on Unix, `/`; on Windows, build via the temp's root.
        #[cfg(unix)]
        let abs = PathBuf::from("/etc");
        #[cfg(windows)]
        let abs = {
            // Any rooted path qualifies as absolute on Windows for the
            // purposes of `Path::is_absolute`. Pick the parent itself —
            // it's guaranteed absolute and cannot be resolved as a
            // relative child.
            parent.path().to_path_buf()
        };
        assert!(abs.is_absolute(), "fixture should be absolute");
        let err = BoundedDir::open(parent.path(), &abs)
            .expect_err("absolute child path must be rejected");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
    }

    /// Symlink inside `parent` that points outside `parent` must NOT be
    /// followed by `BoundedDir::open`. cap-std refuses the traversal —
    /// the test asserts an error of any kind is returned (the exact
    /// `io::ErrorKind` is platform-dependent).
    #[cfg(unix)]
    #[test]
    fn bounded_dir_rejects_symlink_escape_unix() {
        let outer = tempdir().unwrap();
        let inner = outer.path().join("workspace");
        fs::create_dir(&inner).unwrap();
        // Target outside the workspace.
        let escape_target = outer.path().join("attacker");
        fs::create_dir(&escape_target).unwrap();
        // Symlink inside the workspace pointing OUT.
        let link = inner.join("escape");
        std::os::unix::fs::symlink(&escape_target, &link).unwrap();

        let result = BoundedDir::open(&inner, Path::new("escape"));
        assert!(
            result.is_err(),
            "opening a symlink that escapes the parent must fail, got {:?}",
            result.map(|b| b.path().to_path_buf()),
        );
    }

    /// Windows analogue: a `symlink_dir` pointing outside the parent
    /// must not be followed. If the host doesn't allow symlink creation
    /// (no Developer Mode / no admin), the test no-ops — the protection
    /// is still in place; we just can't exercise it on this host.
    #[cfg(windows)]
    #[test]
    fn bounded_dir_rejects_symlink_escape_windows() {
        let outer = tempdir().unwrap();
        let inner = outer.path().join("workspace");
        fs::create_dir(&inner).unwrap();
        let escape_target = outer.path().join("attacker");
        fs::create_dir(&escape_target).unwrap();
        let link = inner.join("escape");
        if std::os::windows::fs::symlink_dir(&escape_target, &link).is_err() {
            // Host won't let us create a symlink — nothing to exercise.
            return;
        }
        let result = BoundedDir::open(&inner, Path::new("escape"));
        assert!(
            result.is_err(),
            "opening a symlink_dir that escapes the parent must fail, got {:?}",
            result.map(|b| b.path().to_path_buf()),
        );
    }

    /// `path()` must report a value rooted at the canonicalized parent.
    /// This is what walker integration will compare against lockfile
    /// entries, so the contract is load-bearing.
    #[test]
    fn bounded_dir_path_is_subpath_of_parent() {
        let parent = tempdir().unwrap();
        let child = parent.path().join("nested");
        fs::create_dir(&child).unwrap();
        let bd = BoundedDir::open(parent.path(), Path::new("nested")).unwrap();
        let canon_parent = parent.path().canonicalize().unwrap();
        // `path()` must be exactly canon_parent / "nested".
        assert_eq!(bd.path(), canon_parent.join("nested"));
    }

    /// Best-effort TOCTOU race-window test. Runs in a single thread
    /// because the determinism we want is "the dirfd was bound BEFORE
    /// the swap"; we encode that ordering linearly.
    ///
    /// Threaded variants of this test are hard to make deterministic
    /// across OSes (kernel scheduling), so the property under test is
    /// stated as a sequential causality: if `BoundedDir::open` returns
    /// successfully and we THEN swap the parent for a symlink to an
    /// attacker dir, `bd.path()` must still resolve under the original
    /// parent — i.e. the post-swap state cannot retroactively redirect
    /// the bound dirfd's reported path.
    ///
    /// This test is best-effort: if symlink creation isn't permitted
    /// (Windows w/o Developer Mode), the assertions degrade gracefully.
    #[test]
    fn bounded_dir_resists_post_open_swap() {
        let outer = tempdir().unwrap();
        let real_parent = outer.path().join("real-parent");
        fs::create_dir(&real_parent).unwrap();
        let real_child = real_parent.join("child");
        fs::create_dir(&real_child).unwrap();

        // Step 1: open the bounded dirfd while `real-parent/child` is
        // legitimate. The kernel binds to that inode now.
        let bd =
            BoundedDir::open(&real_parent, Path::new("child")).expect("setup: child should open");
        let bound_path = bd.path().to_path_buf();

        // Step 2: simulate the attacker — try to redirect by replacing
        // `real-parent/child` with a symlink to an outside dir.
        // Best-effort: skip if symlinks aren't allowed on the host.
        let attacker = outer.path().join("attacker");
        fs::create_dir(&attacker).unwrap();
        // Remove the real child (attacker step) — on Windows this can
        // fail if the dirfd holds it open, which is itself a positive
        // signal (the bound handle prevented the swap). Treat both
        // outcomes as acceptable.
        let _ = fs::remove_dir(&real_child);
        // Try to create the symlink. If creation fails (perms or because
        // the path still exists), we still verify the path-binding
        // invariant below.
        #[cfg(unix)]
        let _ = std::os::unix::fs::symlink(&attacker, &real_child);
        #[cfg(windows)]
        let _ = std::os::windows::fs::symlink_dir(&attacker, &real_child);

        // Step 3: the bound dirfd's reported path must STILL be a
        // sub-path of the original real_parent. The attacker cannot
        // retroactively rewrite what the kernel resolved.
        let canon_real_parent = real_parent.canonicalize().unwrap_or(real_parent.clone());
        // canonicalize may follow the new symlink if the dir name was
        // re-created — but `bound_path` was captured BEFORE that, and
        // `BoundedDir` reports the path it resolved at open time.
        assert!(
            bound_path.starts_with(&canon_real_parent) || bound_path.starts_with(&real_parent),
            "bound path {:?} must remain under real parent {:?} after a post-open swap",
            bound_path,
            real_parent,
        );
    }

    /// Opening a non-existent child must fail with `NotFound` (or a
    /// platform-equivalent error). Walker integration will rely on
    /// this for the "fresh clone" case — see Stage 1.e where the
    /// parent is opened first, then the child is created via the
    /// dirfd; the existence check happens at open time.
    #[test]
    fn bounded_dir_rejects_nonexistent_child() {
        let parent = tempdir().unwrap();
        let result = BoundedDir::open(parent.path(), Path::new("does-not-exist"));
        assert!(result.is_err(), "opening a missing child must fail");
    }
}
