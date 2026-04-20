//! Decoupled git backend surface used by the pack walker and exec path.
//!
//! Pack children reference git remotes (`url`, `path`, `ref` — see
//! `.omne/cfg/pack-spec.md` §children). The walker needs to clone, fetch, and
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
    /// # Errors
    ///
    /// Any clone-, network-, or checkout-layer failure maps to a
    /// [`GitError`] variant — see that enum for the taxonomy.
    fn clone(&self, url: &str, dest: &Path, r#ref: Option<&str>) -> Result<ClonedRepo, GitError>;

    /// Fetch from the default remote (`origin`) into an existing repo at
    /// `dest`. Leaves the working tree untouched.
    ///
    /// # Errors
    ///
    /// Returns [`GitError::NotARepository`] when `dest` is not a git repo,
    /// or [`GitError::FetchFailed`] on any network- or ref-update failure.
    fn fetch(&self, dest: &Path) -> Result<(), GitError>;

    /// Resolve `r#ref` (branch, tag, or SHA) and update the working tree at
    /// `dest` to match. Refuses to run if the working tree has uncommitted
    /// changes.
    ///
    /// # Errors
    ///
    /// - [`GitError::NotARepository`] when `dest` is not a git repo.
    /// - [`GitError::DirtyWorkingTree`] when there are uncommitted changes.
    /// - [`GitError::RefNotFound`] when the ref cannot be resolved.
    /// - [`GitError::CheckoutFailed`] for any other checkout-layer failure.
    fn checkout(&self, dest: &Path, r#ref: &str) -> Result<(), GitError>;

    /// Return HEAD at `dest` as a 40-char lowercase hex SHA.
    ///
    /// # Errors
    ///
    /// [`GitError::NotARepository`] when `dest` is not a git repo;
    /// [`GitError::Internal`] wraps any unexpected head-resolution failure.
    fn head_sha(&self, dest: &Path) -> Result<String, GitError>;
}
