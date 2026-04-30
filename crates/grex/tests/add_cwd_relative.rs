//! v2.0 regression: `grex add` from a subdirectory of a workspace must
//! still write to the workspace's event log, NOT to a stray
//! `<cwd>/.grex/events.jsonl` in the subdir.
//!
//! Pre-v2 the CLI defaulted to `<cwd>/grex.jsonl` unconditionally, so
//! invoking `grex add` from `<ws>/sub/` produced `<ws>/sub/grex.jsonl`
//! instead of appending to `<ws>/grex.jsonl`. Post-v2 the CLI walks up
//! from cwd to find the nearest `.grex/` marker and routes the append
//! to `<ws>/.grex/events.jsonl`.

mod common;

use common::grex;
use std::fs;

#[test]
fn add_from_subdirectory_writes_to_workspace_event_log() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    // Mark `ws` as the workspace root by creating `.grex/`. The CLI's
    // `find_workspace_root` walks up looking for this marker.
    fs::create_dir_all(ws.join(".grex")).unwrap();

    // Run `grex add` from a subdirectory of the workspace. Pre-v2 this
    // would have written to `<ws>/sub/grex.jsonl`; post-v2 it must
    // write to `<ws>/.grex/events.jsonl`.
    let sub = ws.join("sub");
    fs::create_dir_all(&sub).unwrap();

    grex().current_dir(&sub).args(["add", "https://example.com/org/repo.git"]).assert().success();

    // The workspace's event log holds the registration.
    let workspace_log = ws.join(".grex").join("events.jsonl");
    assert!(workspace_log.is_file(), "workspace event log must exist at canonical path");
    let body = fs::read_to_string(&workspace_log).expect("read workspace event log");
    assert!(
        body.contains(r#""id":"repo""#),
        "workspace event log must carry the registration row, got: {body}"
    );

    // CRITICAL: no stray `events.jsonl` (or legacy `grex.jsonl`)
    // landed in the subdirectory.
    assert!(
        !sub.join(".grex").join("events.jsonl").exists(),
        "subdirectory must not host a stray event log",
    );
    assert!(!sub.join("grex.jsonl").exists(), "subdirectory must not host a v1.x stray event log",);
}

#[test]
fn add_from_workspace_root_with_legacy_log_auto_migrates() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();

    // Seed a v1.x workspace: legacy event log at the root, no `.grex/`.
    let legacy = ws.join("grex.jsonl");
    fs::write(&legacy, "").unwrap();

    grex().current_dir(ws).args(["add", "https://example.com/org/repo.git"]).assert().success();

    // Migration ran: new path is the canonical one, legacy is gone.
    let canonical = ws.join(".grex").join("events.jsonl");
    assert!(canonical.is_file(), "v2 canonical event log must exist after migration");
    assert!(!legacy.exists(), "v1.x event log must be removed by auto-migration");
    let body = fs::read_to_string(&canonical).unwrap();
    assert!(body.contains(r#""id":"repo""#));
}
