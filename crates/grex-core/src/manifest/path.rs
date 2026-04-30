//! Canonical event-log path resolution and v1→v2 auto-migration.
//!
//! Single source of truth for "where does the event log live?". Every
//! caller that touches the event log MUST go through [`event_log_path`]
//! (compute the path) and [`ensure_event_log_migrated`] (run migration
//! before the first read or write).
//!
//! # Path
//!
//! - **v2.0.0+ canonical:** `<workspace>/.grex/events.jsonl`
//! - **v1.x legacy:** `<workspace>/grex.jsonl`
//!
//! The v1 location was visually conflated with the lockfile
//! (`<workspace>/.grex/grex.lock.jsonl`) because both shared the
//! `grex.*.jsonl` prefix. Renaming and relocating the event log inside
//! `.grex/` (symmetric with the lockfile) eliminates that confusion.
//!
//! # Migration
//!
//! [`ensure_event_log_migrated`] is idempotent and cheap when no migration
//! is needed. It runs at every entry point that may touch the event log.

use std::fs;
use std::path::{Path, PathBuf};

use super::error::ManifestError;

/// v2.0.0+ canonical event-log location, relative to the workspace root.
pub const EVENT_LOG_REL: &str = ".grex/events.jsonl";

/// v1.x legacy event-log location, relative to the workspace root.
/// Retained so [`ensure_event_log_migrated`] can detect a stale workspace.
pub const LEGACY_EVENT_LOG_REL: &str = "grex.jsonl";

/// Compute the canonical event-log path for `workspace`. Does **not**
/// touch the filesystem.
pub fn event_log_path(workspace: &Path) -> PathBuf {
    workspace.join(".grex").join("events.jsonl")
}

/// Walk up from `start` looking for an ancestor that contains a `.grex/`
/// marker (either the v2 events log, the v1 legacy log, or a `.grex/`
/// directory). Falls back to `start` if no marker is found, so a fresh
/// workspace just uses the caller's directory.
///
/// Used by the CLI verbs (`add`, `import`, `serve`) to fix the v1.x
/// cwd-relative bug: invoking `grex add` from a subdirectory of a
/// workspace must still write to the workspace's event log, not to a
/// cwd-rooted stray file.
pub fn find_workspace_root(start: &Path) -> PathBuf {
    let mut cur = start;
    loop {
        if cur.join(EVENT_LOG_REL).is_file()
            || cur.join(LEGACY_EVENT_LOG_REL).is_file()
            || cur.join(".grex").is_dir()
        {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => return start.to_path_buf(),
        }
    }
}

/// Auto-migrate a v1.x event log at `<workspace>/grex.jsonl` to the v2
/// canonical location `<workspace>/.grex/events.jsonl`. Returns the
/// canonical (v2) path either way.
///
/// Behavior:
/// - Both new and legacy present → warn, prefer new, leave legacy
///   in place for the operator to inspect / delete manually.
/// - Only legacy present → ensure `.grex/` exists, rename legacy → new,
///   log info.
/// - Only new present, or neither → no-op.
///
/// Cheap and idempotent; safe to call on every entry point.
///
/// # Errors
///
/// Returns [`ManifestError::Io`] if the rename or `.grex/` creation
/// fails. A failed migration aborts the calling operation rather than
/// silently writing to the legacy path — splitting the log across two
/// files would defeat the rename.
pub fn ensure_event_log_migrated(workspace: &Path) -> Result<PathBuf, ManifestError> {
    let new_path = event_log_path(workspace);
    let legacy_path = workspace.join(LEGACY_EVENT_LOG_REL);
    let new_exists = new_path.exists();
    let legacy_exists = legacy_path.exists();

    match (new_exists, legacy_exists) {
        (true, true) => {
            tracing::warn!(
                legacy = %legacy_path.display(),
                canonical = %new_path.display(),
                "v1.x event log AND v2 event log both present; using v2 and leaving v1 in place \
                 — please review and delete the legacy file"
            );
        }
        (false, true) => {
            if let Some(parent) = new_path.parent() {
                fs::create_dir_all(parent).map_err(ManifestError::Io)?;
            }
            fs::rename(&legacy_path, &new_path).map_err(ManifestError::Io)?;
            tracing::info!(
                from = %legacy_path.display(),
                to = %new_path.display(),
                "migrated v1.x event log to v2 canonical location"
            );
        }
        // (true, false) and (false, false): nothing to do.
        _ => {}
    }
    Ok(new_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn event_log_path_is_dotgrex_events_jsonl() {
        let d = tempdir().unwrap();
        let p = event_log_path(d.path());
        assert_eq!(p, d.path().join(".grex").join("events.jsonl"));
    }

    #[test]
    fn migrate_no_legacy_no_new_is_noop() {
        let d = tempdir().unwrap();
        let resolved = ensure_event_log_migrated(d.path()).unwrap();
        assert_eq!(resolved, event_log_path(d.path()));
        assert!(!resolved.exists());
        assert!(!d.path().join(LEGACY_EVENT_LOG_REL).exists());
    }

    #[test]
    fn migrate_legacy_only_renames_to_new() {
        let d = tempdir().unwrap();
        let legacy = d.path().join(LEGACY_EVENT_LOG_REL);
        fs::write(&legacy, b"{\"k\":\"v\"}\n").unwrap();
        let resolved = ensure_event_log_migrated(d.path()).unwrap();
        assert_eq!(resolved, event_log_path(d.path()));
        assert!(resolved.exists(), "new path must exist after migration");
        assert!(!legacy.exists(), "legacy path must be gone after migration");
        let body = fs::read_to_string(&resolved).unwrap();
        assert_eq!(body, "{\"k\":\"v\"}\n");
    }

    #[test]
    fn migrate_new_only_is_noop() {
        let d = tempdir().unwrap();
        let new_path = event_log_path(d.path());
        fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        fs::write(&new_path, b"already-migrated\n").unwrap();
        let resolved = ensure_event_log_migrated(d.path()).unwrap();
        assert_eq!(resolved, new_path);
        assert_eq!(fs::read_to_string(&resolved).unwrap(), "already-migrated\n");
        assert!(!d.path().join(LEGACY_EVENT_LOG_REL).exists());
    }

    #[test]
    fn migrate_both_present_prefers_new_keeps_legacy() {
        let d = tempdir().unwrap();
        let new_path = event_log_path(d.path());
        fs::create_dir_all(new_path.parent().unwrap()).unwrap();
        fs::write(&new_path, b"new\n").unwrap();
        let legacy = d.path().join(LEGACY_EVENT_LOG_REL);
        fs::write(&legacy, b"legacy\n").unwrap();
        let resolved = ensure_event_log_migrated(d.path()).unwrap();
        assert_eq!(resolved, new_path);
        // New is preferred; legacy stays for the operator.
        assert_eq!(fs::read_to_string(&resolved).unwrap(), "new\n");
        assert!(legacy.exists(), "legacy must be left in place when both present");
    }

    #[test]
    fn find_workspace_root_walks_up_to_dotgrex_dir() {
        let d = tempdir().unwrap();
        let ws = d.path();
        fs::create_dir_all(ws.join(".grex")).unwrap();
        let sub = ws.join("a").join("b");
        fs::create_dir_all(&sub).unwrap();
        let found = find_workspace_root(&sub);
        // Canonicalise both sides so platform-prefix differences (Windows
        // `\\?\` vs vanilla) don't trip the comparison.
        assert_eq!(fs::canonicalize(&found).unwrap(), fs::canonicalize(ws).unwrap(),);
    }

    #[test]
    fn find_workspace_root_walks_up_to_legacy_log() {
        let d = tempdir().unwrap();
        let ws = d.path();
        fs::write(ws.join(LEGACY_EVENT_LOG_REL), b"").unwrap();
        let sub = ws.join("a");
        fs::create_dir_all(&sub).unwrap();
        let found = find_workspace_root(&sub);
        assert_eq!(fs::canonicalize(&found).unwrap(), fs::canonicalize(ws).unwrap(),);
    }

    #[test]
    fn find_workspace_root_falls_back_to_start_when_no_marker() {
        let d = tempdir().unwrap();
        let found = find_workspace_root(d.path());
        assert_eq!(found, d.path());
    }
}
