//! v1.x → v2.0 event-log migration regression.
//!
//! Pins the auto-migration contract documented in
//! `crates/grex-core/src/manifest/path.rs`:
//!
//! 1. A workspace whose only event log is the v1.x `<ws>/grex.jsonl`
//!    must be migrated in-place on first access, leaving `<ws>/.grex/
//!    events.jsonl` as the sole event log and removing the legacy file.
//! 2. Subsequent migrations are no-ops (idempotent).
//! 3. When BOTH paths exist (rare race / partial migration), the
//!    canonical v2 path wins and the legacy file is left in place for
//!    operator inspection.

use chrono::Utc;
use grex_core::manifest::{
    append_event, ensure_event_log_migrated, event_log_path, read_all, Event, EVENT_LOG_REL,
    LEGACY_EVENT_LOG_REL, SCHEMA_VERSION,
};
use std::fs;
use tempfile::tempdir;

fn sample_add(id: &str) -> Event {
    Event::Add {
        ts: Utc::now(),
        id: id.into(),
        url: "u".into(),
        path: id.into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    }
}

#[test]
fn legacy_event_log_is_renamed_into_dotgrex_on_first_access() {
    let dir = tempdir().unwrap();
    let ws = dir.path();

    // Seed a v1.x workspace: a single legacy event log at the root,
    // no `.grex/` directory.
    let legacy = ws.join(LEGACY_EVENT_LOG_REL);
    let mut body = String::new();
    body.push_str(&serde_json::to_string(&sample_add("alpha")).unwrap());
    body.push('\n');
    body.push_str(&serde_json::to_string(&sample_add("beta")).unwrap());
    body.push('\n');
    fs::write(&legacy, body).unwrap();

    // First access — migration runs.
    let resolved = ensure_event_log_migrated(ws).unwrap();
    assert_eq!(resolved, event_log_path(ws), "resolved path must be v2 canonical");
    assert!(resolved.exists(), "v2 event log must exist after migration");
    assert!(!legacy.exists(), "v1.x event log must be gone after migration");

    // Content survived the move.
    let events = read_all(&resolved).unwrap();
    assert_eq!(events.len(), 2, "both events must round-trip through migration");
    let ids: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            Event::Add { id, .. } => Some(id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(ids, vec!["alpha", "beta"]);
}

#[test]
fn migration_is_idempotent() {
    let dir = tempdir().unwrap();
    let ws = dir.path();

    // Seed and migrate.
    fs::write(ws.join(LEGACY_EVENT_LOG_REL), b"").unwrap();
    let _ = ensure_event_log_migrated(ws).unwrap();

    // Append a real event so the file has known content.
    append_event(&event_log_path(ws), &sample_add("gamma")).unwrap();
    let after_first = fs::read(event_log_path(ws)).unwrap();

    // Second migration is a no-op.
    let _ = ensure_event_log_migrated(ws).unwrap();
    let after_second = fs::read(event_log_path(ws)).unwrap();
    assert_eq!(after_first, after_second, "second migration must not touch the file");
    assert!(!ws.join(LEGACY_EVENT_LOG_REL).exists());
}

#[test]
fn both_paths_present_prefers_v2_keeps_legacy() {
    let dir = tempdir().unwrap();
    let ws = dir.path();

    // Seed both — simulates a partial migration or operator copy/paste.
    fs::create_dir_all(ws.join(".grex")).unwrap();
    fs::write(ws.join(EVENT_LOG_REL), b"v2-content\n").unwrap();
    fs::write(ws.join(LEGACY_EVENT_LOG_REL), b"v1-content\n").unwrap();

    let resolved = ensure_event_log_migrated(ws).unwrap();
    assert_eq!(resolved, event_log_path(ws));
    assert_eq!(
        fs::read_to_string(&resolved).unwrap(),
        "v2-content\n",
        "v2 must be preferred when both present"
    );
    assert!(
        ws.join(LEGACY_EVENT_LOG_REL).exists(),
        "legacy must be left in place for the operator to review"
    );
}

#[test]
fn fresh_workspace_migration_is_pure_noop() {
    let dir = tempdir().unwrap();
    let ws = dir.path();

    // Neither legacy nor v2 exists.
    let resolved = ensure_event_log_migrated(ws).unwrap();
    assert_eq!(resolved, event_log_path(ws));
    assert!(!resolved.exists(), "no event log on fresh workspace");
    assert!(!ws.join(LEGACY_EVENT_LOG_REL).exists());
    // Critically: the `.grex/` directory must NOT be pre-created — it
    // is only made on first append.
    assert!(!ws.join(".grex").exists(), "fresh-workspace migration must not create .grex/");
}
