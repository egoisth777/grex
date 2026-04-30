//! v1.2.0 — distributed per-meta lockfile read / write / fold.
//!
//! Each meta (a directory containing `.grex/pack.yaml`) owns its own
//! `<meta>/.grex/grex.lock.jsonl`, recording entries for **its direct
//! children only**. The walker is the sole writer; this module exposes
//! the meta-shaped read / write / fold surface.
//!
//! # Why a distinct module
//!
//! `lockfile/io.rs` carries the v1.x flat-path API that takes an explicit
//! `&Path` to the lockfile file. The v1.2.0 walker thinks in *metas*, not
//! files: every recursion frame has a `meta_dir`, and the lockfile path
//! falls out from that. Splitting the meta-shaped surface here keeps the
//! flat API intact for legacy call sites and gives the v1.2.0 walker a
//! more typed seam.
//!
//! # Lean theorems backed
//!
//! * **W2 distributed_isolation** — each meta's lockfile entries refer
//!   only to that meta's direct children. [`read_meta_lockfile`] returns
//!   the entries for one frame; the fold ([`read_lockfile_tree`]) is the
//!   only API that crosses meta boundaries, and it preserves the
//!   per-meta partition by construction.
//! * **F1 fold_tree_lockfile_partition** — folding the tree yields a
//!   disjoint union of every meta's entries. No entry is double-counted
//!   because the walk visits each meta exactly once and only emits that
//!   meta's own lockfile contents.

use std::path::{Path, PathBuf};

use super::entry::{LockEntry, LockfileError};
use super::io::read_lockfile;
use crate::fs::atomic_write;

/// Resolve the per-meta lockfile path:
/// `<meta_dir>/.grex/grex.lock.jsonl`.
///
/// Pure path construction — no I/O. Mirrors the `<meta>/.grex/` sidecar
/// convention that `pack.yaml`, `events.jsonl`, and the lockfile share.
#[must_use]
pub fn meta_lockfile_path(meta_dir: &Path) -> PathBuf {
    meta_dir.join(".grex").join("grex.lock.jsonl")
}

/// Read the per-meta lockfile, returning its entries as a `Vec`.
///
/// * Missing file → `Ok(Vec::new())`. A meta with no children, or a
///   meta that has not yet been synced, naturally has no lockfile.
/// * Parse errors are fatal (atomic-write contract — see
///   [`super::io::read_lockfile`]).
///
/// Entries are returned in deterministic order (sorted by id) so callers
/// that hash or diff folded output get stable results without an extra
/// sort step.
///
/// # Errors
///
/// Returns [`LockfileError::Io`] for non-`NotFound` IO failures and
/// [`LockfileError::Corruption`] for malformed lines.
pub fn read_meta_lockfile(meta_dir: &Path) -> Result<Vec<LockEntry>, LockfileError> {
    let path = meta_lockfile_path(meta_dir);
    let map = read_lockfile(&path)?;
    let mut out: Vec<LockEntry> = map.into_values().collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(out)
}

/// Atomically replace the per-meta lockfile with the given entries.
///
/// Creates `<meta_dir>/.grex/` if it does not yet exist so that callers
/// can write a fresh per-meta lockfile without a separate `mkdir` step.
/// The file write itself is atomic via [`atomic_write`].
///
/// # Errors
///
/// Returns [`LockfileError::Io`] for IO failures (directory creation or
/// the atomic-write rename) and [`LockfileError::Serialize`] for JSON
/// serialization failures.
pub fn write_meta_lockfile(meta_dir: &Path, entries: &[LockEntry]) -> Result<(), LockfileError> {
    let dir = meta_dir.join(".grex");
    if !dir.exists() {
        std::fs::create_dir_all(&dir).map_err(LockfileError::Io)?;
    }
    let path = dir.join("grex.lock.jsonl");

    // Sort by id for byte-stable output (matches `write_lockfile`'s
    // contract). Callers that already hand in a sorted slice pay only a
    // O(n) verification cost via the comparator's lazy short-circuit.
    let mut ids: Vec<&LockEntry> = entries.iter().collect();
    ids.sort_by(|a, b| a.id.cmp(&b.id));
    let mut buf = String::new();
    for entry in ids {
        let line = serde_json::to_string(entry).map_err(LockfileError::Serialize)?;
        buf.push_str(&line);
        buf.push('\n');
    }
    atomic_write(&path, buf.as_bytes())?;
    Ok(())
}

/// Fold a meta-tree into the disjoint union of every meta's lockfile
/// entries.
///
/// Walks the on-disk meta tree starting at `root_meta`:
///
/// 1. Read `root_meta`'s per-meta lockfile (its direct children).
/// 2. For each child `c` declared in `<root_meta>/.grex/pack.yaml`,
///    if `<root_meta>/<c.path>/.grex/pack.yaml` exists, recurse.
/// 3. Concatenate every meta's entries.
///
/// **Disjointness** is guaranteed by construction: each meta's lockfile
/// records only its direct children (W2), so two distinct metas can
/// never both list the same `(id, path)` entry. Tests assert this via
/// the `path` field — a child's path under the root meta is unique and
/// every entry carries its parent meta's parent-relative path.
///
/// Metas with a missing or unreadable `pack.yaml` are silently skipped:
/// the fold is read-only and tolerant of partial trees so `doctor` and
/// `ls` can still render a best-effort view. Lockfile parse errors are
/// surfaced — corruption never silently truncates the fold output.
///
/// # Errors
///
/// Returns the first [`LockfileError`] encountered while reading any
/// per-meta lockfile.
pub fn read_lockfile_tree(root_meta: &Path) -> Result<Vec<LockEntry>, LockfileError> {
    let mut out: Vec<LockEntry> = Vec::new();
    fold_meta_recursive(root_meta, &mut out)?;
    Ok(out)
}

/// Visit one meta frame: append its lockfile entries, then recurse into
/// every declared child whose dest carries a `pack.yaml`.
fn fold_meta_recursive(meta_dir: &Path, out: &mut Vec<LockEntry>) -> Result<(), LockfileError> {
    let entries = read_meta_lockfile(meta_dir)?;
    out.extend(entries);

    // Discover declared children via the manifest. A meta without a
    // pack.yaml is a leaf for the fold — return without descending.
    let manifest_path = meta_dir.join(".grex").join("pack.yaml");
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        // Non-NotFound IO errors are tolerated for fold robustness —
        // parse failure on the manifest is not the lockfile's concern.
        Err(_) => return Ok(()),
    };
    let manifest = match crate::pack::parse(&raw) {
        Ok(m) => m,
        Err(_) => return Ok(()),
    };

    for child in &manifest.children {
        let segment = child.path.clone().unwrap_or_else(|| child.effective_path());
        let child_meta = meta_dir.join(&segment);
        // Recurse only when the child carries its own manifest — plain-
        // git children (no `.grex/pack.yaml`) are leaves under v1.2.0.
        if child_meta.join(".grex").join("pack.yaml").is_file() {
            fold_meta_recursive(&child_meta, out)?;
        }
    }
    Ok(())
}

/// Detect whether `<meta_dir>/.grex/grex.lock.jsonl` is in the v1.1.1
/// shape (entries lacking the explicit `path` JSON field).
///
/// Returns:
///
/// * `Ok(false)` when the file is absent, empty, or every line carries
///   `"path":` (v1.2.0 shape).
/// * `Ok(true)` when the file exists and at least one line lacks the
///   `path` field.
/// * `Err(_)` for IO errors other than NotFound.
///
/// This is the discriminator used by the walker to decide whether to
/// fail with [`crate::tree::TreeError::LegacyLockfileDetected`] (when
/// the operator has not opted into migration) or to invoke the
/// [`super::migrate_v1_1_1`] module.
///
/// Detection is content-based, not path-based: v1.1.1 and v1.2.0 share
/// the same on-disk filename. The presence (or absence) of the `path`
/// JSON field is the only schema discriminator since v1.2.0 always
/// serializes `path` while v1.1.1 never wrote it.
///
/// # Errors
///
/// Returns [`LockfileError::Io`] for non-NotFound IO failures while
/// opening or reading the lockfile.
pub fn detect_legacy_lockfile(meta_dir: &Path) -> Result<bool, LockfileError> {
    let path = meta_lockfile_path(meta_dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(LockfileError::Io(e)),
    };
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        // Cheap structural check: look for `"path":` token. Lockfile
        // entries are flat JSON objects with no nested `path` keys, so
        // a substring match is unambiguous here.
        if !trimmed.contains("\"path\":") {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 29, 10, 0, 0).unwrap()
    }

    fn entry(id: &str, path: &str) -> LockEntry {
        let mut e = LockEntry::new(id, "deadbeef", "main", ts(), "h", "1");
        e.path = path.into();
        e
    }

    /// AC: per-meta lockfile path resolves to `<meta>/.grex/grex.lock.jsonl`.
    #[test]
    fn test_per_meta_lockfile_path_resolution() {
        let dir = tempdir().unwrap();
        let meta = dir.path().join("alpha");
        let p = meta_lockfile_path(&meta);
        assert_eq!(p, meta.join(".grex").join("grex.lock.jsonl"));
    }

    /// AC: reading a meta with no `.grex/grex.lock.jsonl` returns an
    /// empty list.
    #[test]
    fn test_per_meta_read_empty_meta() {
        let dir = tempdir().unwrap();
        let entries = read_meta_lockfile(dir.path()).expect("missing → empty");
        assert!(entries.is_empty());
    }

    /// AC: writing then reading v1.2.0-shaped entries round-trips and
    /// preserves the explicit `path` field.
    #[test]
    fn test_per_meta_read_v1_2_0_format() {
        let dir = tempdir().unwrap();
        let entries = vec![entry("alpha", "alpha"), entry("beta", "beta")];
        write_meta_lockfile(dir.path(), &entries).unwrap();
        let back = read_meta_lockfile(dir.path()).unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].id, "alpha");
        assert_eq!(back[0].path, "alpha");
        assert_eq!(back[1].id, "beta");
    }

    /// AC: write is atomic — the on-disk file is never observed in a
    /// half-written state. We assert the post-condition: after the
    /// write returns, the file exists with the full content.
    #[test]
    fn test_per_meta_write_atomic() {
        let dir = tempdir().unwrap();
        let entries = vec![entry("a", "a")];
        write_meta_lockfile(dir.path(), &entries).unwrap();
        let p = meta_lockfile_path(dir.path());
        assert!(p.is_file(), "lockfile must exist after write");
        let raw = std::fs::read_to_string(&p).unwrap();
        // Atomic-write surface is byte-for-byte: full payload must end
        // with a newline (one entry → one line + trailing `\n`).
        assert!(raw.ends_with('\n'), "atomic write must persist full payload");
        assert!(raw.contains("\"id\":\"a\""));
    }

    /// AC: folding a multi-meta tree yields a disjoint union of every
    /// meta's entries — no double-counting, no cross-meta leakage.
    #[test]
    fn test_lockfile_fold_tree_disjoint_partition() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // Root meta has children `alpha` and `beta` (alpha is itself a
        // meta with grandchild `gamma`; beta is a leaf meta with no
        // children).
        std::fs::create_dir_all(root.join(".grex")).unwrap();
        std::fs::write(
            root.join(".grex").join("pack.yaml"),
            "schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n  - url: https://example.invalid/alpha.git\n    path: alpha\n  - url: https://example.invalid/beta.git\n    path: beta\n",
        )
        .unwrap();
        write_meta_lockfile(root, &[entry("alpha", "alpha"), entry("beta", "beta")]).unwrap();

        let alpha = root.join("alpha");
        std::fs::create_dir_all(alpha.join(".grex")).unwrap();
        std::fs::write(
            alpha.join(".grex").join("pack.yaml"),
            "schema_version: \"1\"\nname: alpha\ntype: meta\nchildren:\n  - url: https://example.invalid/gamma.git\n    path: gamma\n",
        )
        .unwrap();
        write_meta_lockfile(&alpha, &[entry("gamma", "gamma")]).unwrap();

        let beta = root.join("beta");
        std::fs::create_dir_all(beta.join(".grex")).unwrap();
        std::fs::write(
            beta.join(".grex").join("pack.yaml"),
            "schema_version: \"1\"\nname: beta\ntype: meta\n",
        )
        .unwrap();
        // beta has no lockfile (leaf meta with zero children).

        let folded = read_lockfile_tree(root).unwrap();
        let ids: Vec<&str> = folded.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(
            ids.iter().filter(|id| **id == "alpha").count(),
            1,
            "alpha must appear exactly once",
        );
        assert_eq!(
            ids.iter().filter(|id| **id == "beta").count(),
            1,
            "beta must appear exactly once",
        );
        assert_eq!(
            ids.iter().filter(|id| **id == "gamma").count(),
            1,
            "gamma must appear exactly once",
        );
        assert_eq!(folded.len(), 3, "fold must be disjoint (no doubles)");
    }

    /// AC: the legacy detector flags a v1.1.1-shaped lockfile (entries
    /// lacking the explicit `path` field).
    #[test]
    fn test_legacy_v1_1_1_detected() {
        let dir = tempdir().unwrap();
        let lock_dir = dir.path().join(".grex");
        std::fs::create_dir_all(&lock_dir).unwrap();
        // v1.1.1-shaped line — no `"path":` token.
        let v1_1_1_line = r#"{"id":"alpha","sha":"abc","branch":"main","installed_at":"2026-04-27T10:00:00Z","actions_hash":"h","schema_version":"1"}"#;
        std::fs::write(lock_dir.join("grex.lock.jsonl"), format!("{}\n", v1_1_1_line)).unwrap();
        assert!(
            detect_legacy_lockfile(dir.path()).unwrap(),
            "v1.1.1-shaped lockfile must be detected as legacy",
        );
    }

    /// AC: a v1.2.0 lockfile (every entry carries `path`) is NOT legacy.
    #[test]
    fn test_legacy_detector_clears_v1_2_0_lockfile() {
        let dir = tempdir().unwrap();
        write_meta_lockfile(dir.path(), &[entry("alpha", "alpha")]).unwrap();
        assert!(
            !detect_legacy_lockfile(dir.path()).unwrap(),
            "v1.2.0 lockfile must not be flagged legacy",
        );
    }

    /// AC: a meta with no lockfile is not legacy (nothing to migrate).
    #[test]
    fn test_legacy_detector_missing_file_is_not_legacy() {
        let dir = tempdir().unwrap();
        assert!(!detect_legacy_lockfile(dir.path()).unwrap());
    }
}
