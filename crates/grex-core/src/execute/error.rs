//! Error taxonomy for the execute phase.

use thiserror::Error;

use crate::vars::VarExpandError;

/// Errors surfaced by [`crate::execute::ActionExecutor::execute`]
/// implementations.
///
/// Marked `#[non_exhaustive]` so slice 5b can add wet-run-specific variants
/// (`FsIo`, `SymlinkCreate`, `SpawnFailed`, `ChildExit`, ...) without
/// breaking downstream `match` arms.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum ExecError {
    /// Variable expansion failed on a specific field of an action.
    #[error("variable expansion failed in field `{field}`: {source}")]
    VarExpand {
        /// Short field identifier (e.g. `"symlink.dst"`).
        field: &'static str,
        /// Underlying expansion error.
        #[source]
        source: VarExpandError,
    },
    /// An expanded string yielded a path shape grex cannot use (empty,
    /// non-UTF-8 surrogate pair, etc.).
    #[error("invalid path after expansion: `{0}`")]
    InvalidPath(String),
    /// A `require` action evaluated to false with `on_fail: error`.
    #[error("require predicate failed: {detail}")]
    RequireFailed {
        /// Human-readable summary of which predicate(s) did not hold.
        detail: String,
    },
    /// An `exec` action had an internally inconsistent post-expansion shape.
    #[error("exec validation failed: {0}")]
    ExecInvalid(String),
}
