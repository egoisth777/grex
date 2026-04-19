//! Error type for manifest operations.

use thiserror::Error;

/// Errors surfaced by manifest append, read, fold, and compact.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// I/O failure while reading or writing the manifest file.
    #[error("manifest i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// Serialization failure when writing an event.
    #[error("manifest serialize error: {0}")]
    Serialize(serde_json::Error),

    /// A non-trailing line failed to parse — the file is corrupted and
    /// cannot be recovered automatically.
    ///
    /// Contains the 1-based line number and the underlying parse error.
    #[error("manifest corrupted at line {line}: {source}")]
    Corruption {
        /// 1-based line number of the offending event.
        line: usize,
        /// Underlying JSON parse error.
        #[source]
        source: serde_json::Error,
    },
}
