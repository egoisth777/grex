//! Lockfile entry + error types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One resolved pack entry. Serialized as a single JSON line.
///
/// Marked `#[non_exhaustive]` so future audit fields (timestamps,
/// resolved-ref metadata, plugin signatures) can land without breaking
/// out-of-crate consumers that struct-literal-construct entries.
/// Within `grex-core` the existing struct-literal sites continue to
/// work unchanged; external callers should use [`LockEntry::new`] (and
/// the field-level `pub` mutators) instead of struct literals.
#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    /// Pack identifier — matches the manifest id.
    pub id: String,
    /// Resolved commit SHA at the time of install.
    pub sha: String,
    /// Branch or ref used to resolve `sha`.
    pub branch: String,
    /// Timestamp of the last successful install/sync.
    pub installed_at: DateTime<Utc>,
    /// Content hash of the declarative actions that ran. Empty for
    /// imperative packs.
    pub actions_hash: String,
    /// Schema version of this entry.
    pub schema_version: String,
    /// `true` when the walker synthesised this pack's manifest in-memory
    /// because the child had no `.grex/pack.yaml` but did carry a
    /// `.git/` (v1.1.1 plain-git children). `#[serde(default)]` keeps
    /// pre-v1.1.1 lockfiles forward-compatible — a missing field
    /// deserialises to `false`. See
    /// `openspec/changes/feat-v1.1.1-plain-git-children/design.md`.
    #[serde(default)]
    pub synthetic: bool,
}

impl LockEntry {
    /// Construct a new entry with every required field. The `synthetic`
    /// flag defaults to `false`; callers that need a synthetic entry
    /// should set the field directly after construction (the field is
    /// `pub`).
    ///
    /// Because [`LockEntry`] is `#[non_exhaustive]`, this is the
    /// canonical constructor for out-of-crate consumers — struct
    /// literals will not compile from outside the crate.
    #[must_use]
    pub fn new(
        id: impl Into<String>,
        sha: impl Into<String>,
        branch: impl Into<String>,
        installed_at: DateTime<Utc>,
        actions_hash: impl Into<String>,
        schema_version: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            sha: sha.into(),
            branch: branch.into(),
            installed_at,
            actions_hash: actions_hash.into(),
            schema_version: schema_version.into(),
            synthetic: false,
        }
    }
}

/// Errors surfaced by lockfile read/write.
#[derive(Debug, Error)]
pub enum LockfileError {
    /// I/O failure while reading or writing.
    #[error("lockfile i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// A line failed to parse. Lockfile corruption is always fatal — there
    /// is no torn-line recovery rule since writes are atomic.
    #[error("lockfile corrupted at line {line}: {source}")]
    Corruption {
        /// 1-based line number.
        line: usize,
        /// Underlying JSON parse error.
        #[source]
        source: serde_json::Error,
    },

    /// Serialization failure when writing.
    #[error("lockfile serialize error: {0}")]
    Serialize(serde_json::Error),
}
