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
#[serde(from = "LockEntryRepr")]
pub struct LockEntry {
    /// Pack identifier — matches the manifest id.
    pub id: String,
    /// Parent-relative POSIX path of this pack's manifest within its
    /// parent's `manifest.children`. Required for v1.2.0 distributed
    /// lockfile resolution: each entry knows where its manifest lives
    /// relative to the parent meta, so the walker can place the dest
    /// correctly even when the same id appears nested.
    ///
    /// **Read-fallback**: v1.1.1 lockfiles do not carry this field; on
    /// deserialize a missing `path` is filled with `id` (the v1.1.1 1:1
    /// id↔folder convention). See `LockEntryRepr` and
    /// `openspec/feat-grex/spec.md` (v1.2.0 distributed lockfile).
    ///
    /// **Validation**: must be parent-relative POSIX (no `..`, no
    /// absolute, no backslash, non-empty). Use [`LockEntry::validate_path`].
    pub path: String,
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
    ///
    /// **v1.3.2 retirement (W1):** the field is no longer emitted by the
    /// writer (`skip_serializing_if = "skip_synthetic_always"` always
    /// skips). v1.1.x lockfiles that still carry `synthetic: true`
    /// continue to deserialise via `#[serde(default)]`, so legacy
    /// readers (doctor's `check_synthetic_packs`, ls's `~` glyph) keep
    /// working until Phase 2c retires them. Fresh on-disk lockfiles
    /// produced by v1.3.2+ contain no `synthetic` key for any entry.
    /// See `pack-spec.md §v1.2.0` (sync-time auto-synthesis retired).
    #[serde(default, skip_serializing_if = "skip_synthetic_always")]
    pub synthetic: bool,
}

/// Always-skip predicate for the v1.3.2-retired `LockEntry.synthetic`
/// field. Returning `true` unconditionally tells `serde` to omit the
/// field from every serialized entry, regardless of in-memory state.
/// The field is preserved in the struct so legacy v1.1.x lockfile
/// reads (which may carry `synthetic: true`) still deserialise cleanly
/// via `#[serde(default)]`.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn skip_synthetic_always(_: &bool) -> bool {
    true
}

/// Wire-format shadow used solely for deserialization. Carries `path`
/// as `Option<String>` so v1.1.1 lockfiles (no `path` field) parse
/// successfully; `From<LockEntryRepr> for LockEntry` then derives the
/// missing path from `id`.
///
/// Stage 1.h (`--migrate-lockfile`, default-OFF) will rewrite v1.1.1
/// lockfiles to carry the explicit `path`; until then the read-fallback
/// keeps every existing on-disk lockfile readable.
#[derive(Deserialize)]
struct LockEntryRepr {
    id: String,
    #[serde(default)]
    path: Option<String>,
    sha: String,
    branch: String,
    installed_at: DateTime<Utc>,
    actions_hash: String,
    schema_version: String,
    #[serde(default)]
    synthetic: bool,
}

impl From<LockEntryRepr> for LockEntry {
    fn from(r: LockEntryRepr) -> Self {
        // v1.1.1 read-fallback: missing `path` → derive from `id`. In
        // v1.1.1 the pack id == folder name (1:1), so `id` is the
        // correct parent-relative path for legacy entries.
        let path = r.path.unwrap_or_else(|| r.id.clone());
        Self {
            id: r.id,
            path,
            sha: r.sha,
            branch: r.branch,
            installed_at: r.installed_at,
            actions_hash: r.actions_hash,
            schema_version: r.schema_version,
            synthetic: r.synthetic,
        }
    }
}

impl LockEntry {
    /// Construct a new entry with every required field. The `synthetic`
    /// flag defaults to `false`; callers that need a synthetic entry
    /// should set the field directly after construction (the field is
    /// `pub`).
    ///
    /// `path` defaults to `id` to preserve v1.1.1 caller compatibility
    /// (v1.1.1's 1:1 id↔folder convention). Callers that need a
    /// distinct manifest path should set the field directly after
    /// construction.
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
        let id = id.into();
        let path = id.clone();
        Self {
            id,
            path,
            sha: sha.into(),
            branch: branch.into(),
            installed_at,
            actions_hash: actions_hash.into(),
            schema_version: schema_version.into(),
            synthetic: false,
        }
    }

    /// Validate that `path` is a parent-relative POSIX path.
    ///
    /// Rejects:
    /// - empty string
    /// - any `..` segment (parent traversal)
    /// - absolute paths: leading `/`, or a Windows drive prefix like `C:`
    /// - backslash separators (Windows-style — POSIX only)
    ///
    /// Accepts: simple names (`foo`), nested POSIX (`a/b/c`).
    pub fn validate_path(path: &str) -> Result<(), LockfileError> {
        if path.is_empty() {
            return Err(LockfileError::InvalidPath {
                path: path.to_string(),
                reason: "path must not be empty",
            });
        }
        if path.contains('\\') {
            return Err(LockfileError::InvalidPath {
                path: path.to_string(),
                reason: "path must use POSIX `/` separator (no `\\`)",
            });
        }
        if path.starts_with('/') {
            return Err(LockfileError::InvalidPath {
                path: path.to_string(),
                reason: "path must be parent-relative (no leading `/`)",
            });
        }
        // Windows-drive prefix detection: `C:`, `c:`, etc. A colon in
        // position 1 with an ASCII letter at position 0 is the drive
        // marker; reject it.
        if path.len() >= 2 {
            let mut chars = path.chars();
            let c0 = chars.next().unwrap();
            let c1 = chars.next().unwrap();
            if c0.is_ascii_alphabetic() && c1 == ':' {
                return Err(LockfileError::InvalidPath {
                    path: path.to_string(),
                    reason: "path must be parent-relative (no drive prefix)",
                });
            }
        }
        for segment in path.split('/') {
            if segment == ".." {
                return Err(LockfileError::InvalidPath {
                    path: path.to_string(),
                    reason: "path must not contain `..` segments",
                });
            }
        }
        Ok(())
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

    /// `LockEntry.path` failed validation. v1.2.0 distributed-lockfile
    /// invariant: paths must be parent-relative POSIX (no `..`, no
    /// absolute, no backslash, non-empty).
    #[error("invalid lockfile entry path `{path}`: {reason}")]
    InvalidPath {
        /// The offending path string.
        path: String,
        /// Human-readable reason the path was rejected.
        reason: &'static str,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 27, 10, 0, 0).unwrap()
    }

    fn sample(id: &str, path: &str) -> LockEntry {
        let mut e = LockEntry::new(id, "deadbeef", "main", ts(), "h", "1");
        e.path = path.into();
        e
    }

    /// v1.2.0 — explicit `path` field survives a JSON round-trip.
    #[test]
    fn test_lockentry_path_field_round_trip() {
        let entry = sample("nested-child", "subdir/nested-child");
        let line = serde_json::to_string(&entry).expect("serialize");
        assert!(
            line.contains(r#""path":"subdir/nested-child""#),
            "serialized form must carry explicit path field, got: {line}"
        );
        let back: LockEntry = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(back, entry);
        assert_eq!(back.path, "subdir/nested-child");
    }

    /// v1.1.1 forward-compat — a v1.1.1-shaped JSON line (no `path`
    /// field) deserialises with `path` derived from `id` via the
    /// read-fallback. Existing on-disk lockfiles remain readable.
    #[test]
    fn test_lockentry_v1_1_1_read_fallback() {
        let line = r#"{"id":"alpha","sha":"abc","branch":"main","installed_at":"2026-04-27T10:00:00Z","actions_hash":"h","schema_version":"1"}"#;
        let entry: LockEntry = serde_json::from_str(line).expect("v1.1.1 line must deserialize");
        assert_eq!(entry.id, "alpha");
        assert_eq!(
            entry.path, "alpha",
            "missing path must be derived from id for v1.1.1 lockfiles",
        );
        assert!(!entry.synthetic);
    }

    /// v1.1.1 read-fallback also works for synthetic entries.
    #[test]
    fn test_lockentry_v1_1_1_read_fallback_synthetic() {
        let line = r#"{"id":"plain-git","sha":"deadbeef","branch":"main","installed_at":"2026-04-27T10:00:00Z","actions_hash":"","schema_version":"1","synthetic":true}"#;
        let entry: LockEntry =
            serde_json::from_str(line).expect("v1.1.1 synthetic must deserialize");
        assert_eq!(entry.path, "plain-git");
        assert!(entry.synthetic);
    }

    /// Validation: parent-traversal `..` segments are rejected.
    #[test]
    fn test_lockentry_path_validation_rejects_parent_traversal() {
        assert!(LockEntry::validate_path("../escape").is_err());
        assert!(LockEntry::validate_path("foo/../bar").is_err());
        assert!(LockEntry::validate_path("..").is_err());
    }

    /// Validation: absolute paths (POSIX or Windows-drive) are rejected.
    #[test]
    fn test_lockentry_path_validation_rejects_absolute() {
        assert!(LockEntry::validate_path("/foo").is_err());
        assert!(LockEntry::validate_path("/").is_err());
        assert!(LockEntry::validate_path("C:/foo").is_err());
        assert!(LockEntry::validate_path("C:\\foo").is_err());
    }

    /// Validation: backslash separators are rejected — POSIX only.
    #[test]
    fn test_lockentry_path_must_be_posix_separator() {
        assert!(LockEntry::validate_path("foo\\bar").is_err());
        // sanity: valid POSIX paths pass
        assert!(LockEntry::validate_path("foo/bar").is_ok());
        assert!(LockEntry::validate_path("plain-git-child").is_ok());
        assert!(LockEntry::validate_path("a/b/c").is_ok());
    }

    /// Validation: the empty string is not a valid path.
    #[test]
    fn test_lockentry_path_validation_rejects_empty() {
        assert!(LockEntry::validate_path("").is_err());
    }
}
