//! Error type for pack-manifest parsing.
//!
//! All Stage-A parse failures surface as [`PackParseError`]. Variants carry
//! enough context (offending key, value, depth, etc.) to produce actionable
//! messages without the caller needing to re-read the YAML.

use thiserror::Error;

/// Maximum nesting depth for `require` / `when` predicate trees. Exceeding
/// this limit yields [`PackParseError::RequireDepthExceeded`]; the cap exists
/// to bound recursive evaluation cost at execute time.
pub const MAX_REQUIRE_DEPTH: usize = 32;

/// Errors produced by [`crate::pack::parse`] and related entry points.
///
/// Each variant is designed to be self-describing when rendered through
/// `Display` (via `thiserror`), so `eprintln!("{err}")` is sufficient for a
/// CLI diagnostic. File-path context, when available, is attached by the
/// caller using [`PackParseError::with_source_path`].
///
/// Marked `#[non_exhaustive]` so new diagnostic variants can land without
/// breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum PackParseError {
    /// `schema_version` is present but not the supported literal `"1"`.
    #[error("unsupported pack schema_version {got:?}: this grex build only understands \"1\"")]
    InvalidSchemaVersion {
        /// Raw value seen in the manifest.
        got: String,
    },

    /// `name` does not match `^[a-z][a-z0-9-]*$`.
    #[error(
        "invalid pack name {got:?}: must match ^[a-z][a-z0-9-]*$ (lowercase letter first; then lowercase letters, digits, or hyphens)"
    )]
    InvalidName {
        /// Raw value seen in the manifest.
        got: String,
    },

    /// The raw YAML contains an anchor (`&`) or alias (`*`) node. grex
    /// rejects these at parse time as a security policy (cycle / billion-
    /// laughs mitigation).
    #[error("YAML anchors/aliases are not supported (security policy)")]
    YamlAliasRejected,

    /// A top-level key was present that is neither a known manifest field
    /// nor prefixed with `x-` (reserved-for-extension namespace).
    #[error(
        "unknown top-level key {key:?}: only documented fields and `x-*` extensions are accepted"
    )]
    UnknownTopLevelKey {
        /// Offending key name.
        key: String,
    },

    /// An action entry's single-key map names an action not in the Tier-1
    /// registry.
    #[error(
        "unknown action {key:?}: valid actions are symlink, env, mkdir, rmdir, require, when, exec"
    )]
    UnknownActionKey {
        /// Offending key name.
        key: String,
    },

    /// An action entry is an empty map (`- {}`).
    #[error("empty action entry: each list item must have exactly one action key")]
    EmptyActionEntry,

    /// An action entry has more than one top-level key, e.g.
    /// `- { symlink: ..., env: ... }`.
    #[error(
        "action entry has multiple keys {keys:?}: each list item must name exactly one action"
    )]
    MultipleActionKeys {
        /// All keys seen on the entry, in iteration order.
        keys: Vec<String>,
    },

    /// A `require` (or `when`) spec declares zero combiners when at least
    /// one is required, or more than one combiner at the same level.
    #[error(
        "require block must declare exactly one of `all_of`, `any_of`, `none_of` (got {count})"
    )]
    RequireCombinerArity {
        /// Number of combiner keys seen.
        count: usize,
    },

    /// A predicate entry is shaped wrong (not a single-key map, or the key
    /// names an unknown predicate).
    #[error("invalid predicate entry: {detail}")]
    InvalidPredicate {
        /// Human-readable detail.
        detail: String,
    },

    /// An `exec` spec violates the `cmd` XOR `cmd_shell` invariant.
    #[error(
        "exec args invariant violated (shell={shell}, cmd={cmd_present}, cmd_shell={cmd_shell_present}): \
when shell=false exactly `cmd` must be set; when shell=true exactly `cmd_shell` must be set"
    )]
    ExecCmdMutex {
        /// Value of the `shell` flag (default `false`).
        shell: bool,
        /// Whether `cmd` was present in the parsed spec.
        cmd_present: bool,
        /// Whether `cmd_shell` was present in the parsed spec.
        cmd_shell_present: bool,
    },

    /// The recursive predicate tree exceeded [`MAX_REQUIRE_DEPTH`].
    #[error("require/when predicate nesting depth {depth} exceeds maximum {max}")]
    RequireDepthExceeded {
        /// Observed depth.
        depth: usize,
        /// Configured maximum.
        max: usize,
    },

    /// Wrap an inner error with the offending source-file path so CLI
    /// callers can present `path: error` diagnostics.
    #[error("{path}: {source}")]
    WithPath {
        /// Source file path (display form — may be non-UTF-8 lossy).
        path: String,
        /// Underlying error.
        #[source]
        source: Box<PackParseError>,
    },

    /// Underlying `serde_yaml` deserialization error (malformed YAML, type
    /// mismatch, etc.).
    #[error("yaml parse error: {0}")]
    Inner(#[from] serde_yaml::Error),
}

impl PackParseError {
    /// Attach source-file context to an error. Intended for the entry point
    /// that reads a file from disk; Stage A's pure-parse API does not use
    /// it directly but surfaces it for consumers.
    #[must_use]
    pub fn with_source_path(self, path: impl Into<String>) -> Self {
        match self {
            // Avoid double-wrapping: keep the innermost error but replace
            // the path with the outermost caller's view.
            Self::WithPath { source, .. } => Self::WithPath { path: path.into(), source },
            other => Self::WithPath { path: path.into(), source: Box::new(other) },
        }
    }
}
