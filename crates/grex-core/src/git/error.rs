//! Error taxonomy for the [`crate::git`] backend.
//!
//! Errors carry `String` detail fields rather than boxing underlying `gix`
//! error types. Keeping the public API free of leaky backend types means the
//! trait surface stays stable across backend swaps and `#[non_exhaustive]`
//! can be safely extended in future slices (e.g. auth, shallow, submodules).

use std::path::PathBuf;

use thiserror::Error;

/// Errors produced by any [`crate::git::GitBackend`] implementation.
///
/// The enum is `#[non_exhaustive]` so future slices (credentials, submodules,
/// partial fetch) can add variants without a breaking change.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum GitError {
    /// Clone was asked to populate a destination that already contains files.
    #[error("destination `{0}` is not empty")]
    DestinationNotEmpty(PathBuf),

    /// Operation was asked to act on a path that does not hold a git repo.
    #[error("path `{0}` is not a git repository")]
    NotARepository(PathBuf),

    /// A ref (branch, tag, or SHA) could not be resolved in the repository.
    #[error("ref `{0}` not found in repository")]
    RefNotFound(String),

    /// Checkout refused because the working tree has uncommitted changes.
    #[error("working tree has uncommitted changes at `{0}`")]
    DirtyWorkingTree(PathBuf),

    /// Clone failed at the backend layer. `detail` carries the backend
    /// message verbatim for operator diagnosis.
    #[error("clone from `{url}` failed: {detail}")]
    CloneFailed {
        /// Remote URL that was being cloned.
        url: String,
        /// Backend-provided failure detail.
        detail: String,
    },

    /// Fetch failed at the backend layer.
    #[error("fetch failed at `{0}`: {1}")]
    FetchFailed(PathBuf, String),

    /// Checkout of a resolved ref failed to apply to the working tree.
    #[error("checkout of `{r#ref}` failed: {detail}")]
    CheckoutFailed {
        /// Ref name (branch, tag, or SHA) that was being checked out.
        r#ref: String,
        /// Backend-provided failure detail.
        detail: String,
    },

    /// Catch-all for unexpected backend errors. Carries the detail string so
    /// the caller can log or surface it without losing information.
    #[error("gix internal error: {0}")]
    Internal(String),
}
