//! Error type for variable expansion.
//!
//! All failures produced by [`crate::vars::expand`] surface as
//! [`VarExpandError`]. Every variant carries a byte offset into the input
//! string so CLI callers can highlight the offending position, and — where
//! applicable — the variable name involved. Messages are designed to stand
//! alone via `Display` (no additional context needed from the caller).

use thiserror::Error;

/// Errors produced by [`crate::vars::expand`].
///
/// Each variant is designed to be self-describing when rendered through
/// `Display` (via `thiserror`), so `eprintln!("{err}")` is sufficient for a
/// CLI diagnostic. The `offset` field always points at the first byte of the
/// offending placeholder (`$`, `${`, or `%`), never inside it.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VarExpandError {
    /// A well-formed placeholder referenced a variable name that was not
    /// present in the [`crate::vars::VarEnv`].
    #[error("variable `{name}` is not set (at byte offset {offset})")]
    MissingVariable {
        /// Variable name as written in the input.
        name: String,
        /// Byte offset of the opening sigil (`$` or `%`).
        offset: usize,
    },

    /// A placeholder contained a name that does not match
    /// `^[A-Za-z_][A-Za-z0-9_]*$`.
    #[error("invalid variable name {got:?} (at byte offset {offset}): must match ^[A-Za-z_][A-Za-z0-9_]*$")]
    InvalidVariableName {
        /// Raw name bytes as scanned from the input (lossy if non-UTF-8).
        got: String,
        /// Byte offset of the opening sigil.
        offset: usize,
    },

    /// A `${` placeholder was never closed with `}` before end of input.
    #[error("unclosed `${{...}}` expansion starting at byte offset {offset}")]
    UnclosedBraceExpansion {
        /// Byte offset of the opening `$`.
        offset: usize,
    },

    /// A `%` opening sigil was never matched by a closing `%` before end of
    /// input.
    #[error("unclosed `%...%` expansion starting at byte offset {offset}")]
    UnclosedPercentExpansion {
        /// Byte offset of the opening `%`.
        offset: usize,
    },

    /// A braced placeholder had an empty name: `${}`.
    #[error("empty `${{}}` expansion at byte offset {offset}")]
    EmptyBraceExpansion {
        /// Byte offset of the opening `$`.
        offset: usize,
    },
}
