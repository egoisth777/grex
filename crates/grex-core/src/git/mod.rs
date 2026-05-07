//! Decoupled git backend surface used by the pack walker and exec path.
//!
//! Pack children reference git remotes (`url`, `path`, `ref` — see
//! `inst/pack-spec.md` §children). The walker needs to clone, fetch, and
//! checkout these remotes; the exec path pins commits. Every one of those
//! callers goes through the [`GitBackend`] trait rather than the `gix` crate
//! directly, so:
//!
//! - tests can substitute an in-memory mock
//! - a future IPC or CLI-shell backend can plug in without rewriting callers
//! - backend-specific error types stay out of the public API (see
//!   `error::GitError` which uses `String` detail fields)
//!
//! The default implementation, [`GixBackend`], wraps the pure-Rust `gix`
//! crate. Auth is the gix default: system SSH keys and anonymous HTTPS.
//! Credential prompting, SSH-agent integration, shallow clones, submodules,
//! and concurrent-fetch coordination are all **out of scope** for this slice
//! and will land in later M3 slices.

pub mod error;
pub mod gix_backend;

use std::path::{Path, PathBuf};

pub use self::error::GitError;
pub use self::gix_backend::GixBackend;
// `BackendLockCtx` is part of the `GitBackend` trait surface (v1.3.2 B11);
// re-export at module root so consumers don't need to thread the inline
// `git::BackendLockCtx` import alongside the trait import.

/// Result of a successful clone.
///
/// `head_sha` is always the 40-char lowercase hex SHA of the commit HEAD was
/// left pointing at after checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClonedRepo {
    /// Filesystem path of the cloned working tree.
    pub path: PathBuf,
    /// HEAD commit SHA, 40-char lowercase hex.
    pub head_sha: String,
}

/// Per-call backend-lock context (v1.3.2 B11).
///
/// The per-repo backend lock lives at
/// `<parent_meta>/.grex/locks/<child_path>.backend.lock` (parent-owned) so
/// it (a) survives a `rm -rf <dest>` rebuild and (b) keeps every lock
/// artifact under one `.grex/` namespace per meta. This struct threads the
/// `(parent_meta, child_path)` pair through the [`GitBackend`] surface so
/// the backend can compute the lock path without re-deriving it from
/// `dest` (which would lose intermediate `/` segments on slash-paths).
///
/// `child_path` is the LITERAL manifest-declared path (e.g. `tools/foo`)
/// — POSIX forward slashes, no slug encoding. Intermediate directories
/// under `<parent_meta>/.grex/locks/` are auto-created on first acquire.
#[derive(Debug, Clone, Copy)]
pub struct BackendLockCtx<'a> {
    /// Workdir of the meta-pack that owns the child clone (`meta_dir` in
    /// the walker terminology). Lock files live under
    /// `<parent_meta>/.grex/locks/`.
    pub parent_meta: &'a Path,
    /// Manifest-declared child path (e.g. `"tools/foo"`). Used verbatim
    /// as the suffix of the backend-lock filename.
    pub child_path: &'a str,
}

impl<'a> BackendLockCtx<'a> {
    /// Construct a new lock context.
    #[must_use]
    pub const fn new(parent_meta: &'a Path, child_path: &'a str) -> Self {
        Self { parent_meta, child_path }
    }
}

/// Owned counterpart of [`BackendLockCtx`]. Provides a convenient way for
/// tests and callers that don't otherwise carry a parent-meta + child-path
/// pair to derive one from a flat `dest` path: `parent_meta = dest.parent()`,
/// `child_path = dest.file_name()`. Borrow with [`BackendLockCtxOwned::as_ctx`]
/// at the trait-method call site.
///
/// v1.3.2 B11 — added so the per-repo backend lock lands under
/// `<parent>/.grex/locks/<name>.backend.lock` for callers that have only a
/// `dest` path on hand. Production code paths (walker phase1) construct
/// [`BackendLockCtx`] directly with the manifest-declared `child.path()`.
#[derive(Debug, Clone)]
pub struct BackendLockCtxOwned {
    /// Owned parent-meta path.
    pub parent_meta: PathBuf,
    /// Owned child path (POSIX-style, may contain `/`).
    pub child_path: String,
}

impl BackendLockCtxOwned {
    /// Derive a lock context from a flat `dest` path. Used by tests that
    /// don't otherwise carry a parent-meta — `dest.parent()` plays the
    /// role of `parent_meta` and `dest.file_name()` the role of
    /// `child_path`. Falls back to `"."` when the parent component is
    /// absent. A missing filename becomes an empty `child_path`, which the
    /// backend lock path validator rejects instead of silently routing to a
    /// generic `"repo"` lock.
    #[must_use]
    pub fn from_dest(dest: &Path) -> Self {
        let parent_meta =
            dest.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        let child_path =
            dest.file_name().and_then(|s| s.to_str()).map_or_else(String::new, str::to_string);
        Self { parent_meta, child_path }
    }

    /// Borrow as a [`BackendLockCtx`] for trait-method call sites.
    #[must_use]
    pub fn as_ctx(&self) -> BackendLockCtx<'_> {
        BackendLockCtx::new(&self.parent_meta, &self.child_path)
    }
}

/// Stable surface for all git operations grex needs.
///
/// Implementors must be `Send + Sync` so a single instance can be handed to
/// the scheduler as `Arc<dyn GitBackend>`. All methods are synchronous — the
/// slice 3 design deliberately avoids async to keep the trait object-safe and
/// the default backend runtime-free.
///
/// Errors are carried as [`GitError`]; the enum is `#[non_exhaustive]` so
/// future variants (credentials, submodules, …) won't break implementors.
pub trait GitBackend: Send + Sync {
    /// Short human-readable name of the backend, e.g. `"gix"`. Used in logs
    /// and diagnostics; not parsed programmatically.
    fn name(&self) -> &'static str;

    /// Clone `url` into `dest`.
    ///
    /// # Contract
    ///
    /// - If `dest` exists and is non-empty → [`GitError::DestinationNotEmpty`].
    /// - If `r#ref` is `Some`, check out that ref after the clone finishes.
    /// - If `r#ref` is `None`, leave the working tree on the remote's default
    ///   HEAD.
    ///
    /// `lock_ctx` carries `(parent_meta, child_path)` so the backend can
    /// compute the per-repo lock path under `<parent_meta>/.grex/locks/`
    /// (v1.3.2 B11). Implementations that do not lock may ignore the
    /// argument.
    ///
    /// # Errors
    ///
    /// Any clone-, network-, or checkout-layer failure maps to a
    /// [`GitError`] variant — see that enum for the taxonomy.
    fn clone(
        &self,
        url: &str,
        dest: &Path,
        r#ref: Option<&str>,
        lock_ctx: BackendLockCtx<'_>,
    ) -> Result<ClonedRepo, GitError>;

    /// Fetch from the default remote (`origin`) into an existing repo at
    /// `dest`. Leaves the working tree untouched.
    ///
    /// `lock_ctx` carries the parent-meta + child-path so the per-repo
    /// lock can be computed under `<parent_meta>/.grex/locks/` (v1.3.2 B11).
    ///
    /// # Errors
    ///
    /// Returns [`GitError::NotARepository`] when `dest` is not a git repo,
    /// or [`GitError::FetchFailed`] on any network- or ref-update failure.
    fn fetch(&self, dest: &Path, lock_ctx: BackendLockCtx<'_>) -> Result<(), GitError>;

    /// Resolve `r#ref` (branch, tag, or SHA) and update the working tree at
    /// `dest` to match. Refuses to run if the working tree has uncommitted
    /// changes.
    ///
    /// `lock_ctx` carries the parent-meta + child-path so the per-repo
    /// lock can be computed under `<parent_meta>/.grex/locks/` (v1.3.2 B11).
    ///
    /// # Errors
    ///
    /// - [`GitError::NotARepository`] when `dest` is not a git repo.
    /// - [`GitError::DirtyWorkingTree`] when there are uncommitted changes.
    /// - [`GitError::RefNotFound`] when the ref cannot be resolved.
    /// - [`GitError::CheckoutFailed`] for any other checkout-layer failure.
    fn checkout(
        &self,
        dest: &Path,
        r#ref: &str,
        lock_ctx: BackendLockCtx<'_>,
    ) -> Result<(), GitError>;

    /// Return HEAD at `dest` as a 40-char lowercase hex SHA.
    ///
    /// # Errors
    ///
    /// [`GitError::NotARepository`] when `dest` is not a git repo;
    /// [`GitError::Internal`] wraps any unexpected head-resolution failure.
    fn head_sha(&self, dest: &Path) -> Result<String, GitError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_lock_ctx_owned_from_dest_does_not_fallback_to_repo() {
        let ctx = BackendLockCtxOwned::from_dest(Path::new("/"));

        assert_eq!(ctx.child_path, "");
    }
}
