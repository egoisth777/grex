//! Lockfile entry + error types.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One resolved pack entry. Serialized as a single JSON line.
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
