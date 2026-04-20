//! Error taxonomy for the [`crate::tree`] walker.
//!
//! Errors carry `PathBuf` and `String` detail fields rather than boxing
//! underlying loader or parser errors. Keeping leaky types out of the public
//! surface means adding a new loader backend (IPC, in-memory, http) in a
//! future slice stays non-breaking.

use std::path::PathBuf;

use thiserror::Error;

use crate::git::GitError;

/// Errors raised during a pack-tree walk.
///
/// Marked `#[non_exhaustive]` so later slices (credentials, submodules,
/// partial walks) can add variants without breaking consumers.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum TreeError {
    /// The walker expected a `pack.yaml` at the given location but could not
    /// find one (or its enclosing `.grex/` directory was missing).
    #[error("pack manifest not found at `{0}`")]
    ManifestNotFound(PathBuf),

    /// The manifest file existed but could not be read from disk.
    #[error("failed to read pack manifest: {0}")]
    ManifestRead(String),

    /// The manifest file was read but did not parse as a valid `pack.yaml`.
    #[error("failed to parse pack manifest at `{path}`: {detail}")]
    ManifestParse {
        /// On-disk location of the manifest that failed to parse.
        path: PathBuf,
        /// Backend-provided failure detail.
        detail: String,
    },

    /// A git operation (clone, fetch, checkout, …) failed while hydrating a
    /// child pack. The underlying [`GitError`] is preserved in full.
    #[error("git error during walk: {0}")]
    Git(#[from] GitError),

    /// A cycle was detected during the walk. `chain` lists the pack URLs (or
    /// paths for the root) from the outermost pack down to the recurrence.
    #[error("cycle detected in pack graph: {chain:?}")]
    CycleDetected {
        /// Ordered chain of pack identities that forms the cycle.
        chain: Vec<String>,
    },

    /// A cloned child's `pack.yaml` declared a `name` that does not match
    /// what the parent pack expected for that `children:` entry.
    #[error("pack name `{got}` does not match expected `{expected}` for child at `{path}`")]
    PackNameMismatch {
        /// Name declared in the child's own manifest.
        got: String,
        /// Name the parent expected (derived from the child entry's
        /// effective path).
        expected: String,
        /// On-disk location of the offending child.
        path: PathBuf,
    },
}
