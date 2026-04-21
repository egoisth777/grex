//! PR E — halt-state persistence + teardown resume.
//!
//! Six focused regressions pin the new surface:
//!
//! 1. `sync::run` writes [`Event::ActionStarted`] **before** the executor
//!    runs, and [`Event::ActionCompleted`] afterwards on the success path.
//! 2. On executor failure the sync driver writes an [`Event::ActionHalted`]
//!    record to the manifest log before returning.
//! 3. `SyncError::Halted(HaltedContext)` carries pack + action index +
//!    error so CLI renderers can pretty-print the halt cause.
//! 4. [`scan_recovery`] finds `<dst>.grex.bak` orphans under the pack root.
//! 5. [`scan_recovery`] flags an `ActionStarted` with no matching
//!    completed/halted peer in the event log as a dangling start.
//! 6. An `exec` action returning non-zero under `on_fail: error` surfaces
//!    its captured stderr in [`ExecError::ExecNonZero::stderr`].

use std::fs;

use chrono::Utc;
use grex_core::execute::ExecError;
use grex_core::manifest::{append_event, read_all, Event};
use grex_core::sync::{run as sync_run, scan_recovery, HaltedContext, SyncError, SyncOptions};
use tempfile::TempDir;

// -------- fixture helpers ------------------------------------------------

/// Write a minimal `<root>/.grex/pack.yaml` with the given inline body. The
/// body text is whatever YAML the test wants under the manifest root.
fn write_pack_yaml(root: &std::path::Path, body: &str) {
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex").join("pack.yaml"), body).unwrap();
}

/// Body for a no-op pack (actions: []).
const NOOP_PACK: &str =
    "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\nactions: []\n";

/// Body for a pack with a single `mkdir` action the test can observe.
/// YAML strings are embedded with explicit quoting so Windows paths with
/// drive letters (backslashes, colons) survive serialisation.
fn mkdir_pack(path: &str) -> String {
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    format!(
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\nactions:\n  - mkdir:\n      path: \"{escaped}\"\n"
    )
}

/// Body for a pack with a single `exec` action that will fail on any
/// platform (false on Unix, /C "exit 1" on Windows). The exec shell path
/// is used so we also exercise the stderr capture.
#[cfg(windows)]
fn failing_exec_pack() -> String {
    concat!(
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\n",
        "actions:\n",
        "  - exec:\n",
        "      shell: true\n",
        "      cmd_shell: \"echo boom 1>&2 && exit 3\"\n",
        "      on_fail: error\n",
    )
    .to_string()
}

#[cfg(not(windows))]
fn failing_exec_pack() -> String {
    concat!(
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\n",
        "actions:\n",
        "  - exec:\n",
        "      shell: true\n",
        "      cmd_shell: \"echo boom 1>&2; exit 3\"\n",
        "      on_fail: error\n",
    )
    .to_string()
}

// -------- 1. pre-action event is written BEFORE execute ------------------

/// Success path: for a one-action pack the manifest must contain (in
/// order) `ActionStarted`, a legacy `Sync` summary step, and
/// `ActionCompleted`. No `ActionHalted`.
#[test]
fn sync_pre_action_event_written_before_execute() {
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().join("pack");
    let target = tmp.path().join("target-dir");
    write_pack_yaml(&pack_root, &mkdir_pack(target.to_str().unwrap()));
    let workspace = tmp.path().join("ws");

    let opts = SyncOptions::new().with_workspace(Some(workspace));
    let report = sync_run(&pack_root, &opts).expect("sync ok");
    assert!(report.halted.is_none(), "no halt on success path");

    let log = pack_root.join(".grex").join("grex.jsonl");
    let events = read_all(&log).expect("log parses");
    let started = events
        .iter()
        .position(|e| matches!(e, Event::ActionStarted { .. }))
        .expect("ActionStarted present");
    let completed = events
        .iter()
        .position(|e| matches!(e, Event::ActionCompleted { .. }))
        .expect("ActionCompleted present");
    assert!(started < completed, "started must precede completed");
    assert!(
        !events.iter().any(|e| matches!(e, Event::ActionHalted { .. })),
        "no halt event on success"
    );
}

// -------- 2. halt writes ActionHalted -----------------------------------

#[test]
fn sync_halted_event_written_on_error() {
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().join("pack");
    write_pack_yaml(&pack_root, &failing_exec_pack());
    let workspace = tmp.path().join("ws");

    let opts = SyncOptions::new().with_workspace(Some(workspace));
    let report = sync_run(&pack_root, &opts).expect("sync returns report even on halt");
    assert!(report.halted.is_some(), "must halt");

    let log = pack_root.join(".grex").join("grex.jsonl");
    let events = read_all(&log).expect("log parses");
    assert!(
        events.iter().any(
            |e| matches!(e, Event::ActionStarted { action_name, .. } if action_name == "exec")
        ),
        "ActionStarted(exec) present"
    );
    let halted = events.iter().find(|e| matches!(e, Event::ActionHalted { .. }));
    assert!(halted.is_some(), "ActionHalted present");
    if let Some(Event::ActionHalted { action_idx, error_summary, .. }) = halted {
        assert_eq!(*action_idx, 0);
        assert!(!error_summary.is_empty());
    }
}

// -------- 3. HaltedContext carries useful fields ------------------------

#[test]
fn sync_halted_context_carries_pack_action_error() {
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().join("pack");
    write_pack_yaml(&pack_root, &failing_exec_pack());
    let workspace = tmp.path().join("ws");

    let opts = SyncOptions::new().with_workspace(Some(workspace));
    let report = sync_run(&pack_root, &opts).expect("report");
    let Some(SyncError::Halted(ctx)) = report.halted else {
        panic!("expected Halted variant, got {:?}", report.halted);
    };
    let HaltedContext { pack, action_idx, action_name, error, .. } = *ctx;
    assert_eq!(pack, "root");
    assert_eq!(action_idx, 0);
    assert_eq!(action_name, "exec");
    assert!(matches!(error, ExecError::ExecNonZero { .. }));
}

// -------- 4. scan finds orphan backups ----------------------------------

#[test]
fn recovery_scan_finds_orphan_backups() {
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().to_path_buf();
    // Seed: a .grex.bak sitting under the pack root with no matching
    // original — classic symlink-rollback orphan signature.
    let orphan = pack_root.join("config.yaml.grex.bak");
    fs::write(&orphan, b"stale backup").unwrap();
    // Also a timestamped tombstone from rmdir --backup.
    let tombstone = pack_root.join("old-config.grex.bak.1700000000");
    fs::write(&tombstone, b"tombstone").unwrap();

    let log = pack_root.join(".grex").join("grex.jsonl");
    let report = scan_recovery(&pack_root, &log).expect("scan ok");
    assert!(report.orphan_backups.iter().any(|p| p == &orphan));
    assert!(report.orphan_tombstones.iter().any(|p| p == &tombstone));
    assert!(report.dangling_starts.is_empty());
}

// -------- 5. scan finds dangling ActionStarted --------------------------

#[test]
fn recovery_scan_finds_dangling_starts() {
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().to_path_buf();
    let log = pack_root.join(".grex").join("grex.jsonl");
    fs::create_dir_all(log.parent().unwrap()).unwrap();

    // One "clean" pair + one "dangling" lone ActionStarted.
    let clean_start = Event::ActionStarted {
        ts: Utc::now(),
        pack: "pk".into(),
        action_idx: 0,
        action_name: "mkdir".into(),
    };
    let clean_completed = Event::ActionCompleted {
        ts: Utc::now(),
        pack: "pk".into(),
        action_idx: 0,
        result_summary: "PerformedChange".into(),
    };
    let dangling = Event::ActionStarted {
        ts: Utc::now(),
        pack: "pk".into(),
        action_idx: 1,
        action_name: "symlink".into(),
    };
    for ev in [&clean_start, &clean_completed, &dangling] {
        append_event(&log, ev).unwrap();
    }

    let report = scan_recovery(&pack_root, &log).expect("scan ok");
    assert_eq!(report.dangling_starts.len(), 1, "exactly one dangling start");
    let d = &report.dangling_starts[0];
    assert_eq!(d.pack, "pk");
    assert_eq!(d.action_idx, 1);
    assert_eq!(d.action_name, "symlink");
}

// -------- 6. exec non-zero captures stderr ------------------------------

#[test]
fn exec_nonzero_captures_stderr() {
    // Build a pack that runs a shell exec writing a marker to stderr and
    // exiting non-zero. The halted-context error must be `ExecNonZero`
    // with the marker in its captured-stderr field.
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().join("pack");
    #[cfg(windows)]
    let body = concat!(
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\n",
        "actions:\n",
        "  - exec:\n",
        "      shell: true\n",
        "      cmd_shell: \"echo GREX_STDERR_MARKER 1>&2 && exit 3\"\n",
        "      on_fail: error\n",
    );
    #[cfg(not(windows))]
    let body = concat!(
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\n",
        "actions:\n",
        "  - exec:\n",
        "      shell: true\n",
        "      cmd_shell: \"echo GREX_STDERR_MARKER 1>&2; exit 3\"\n",
        "      on_fail: error\n",
    );
    write_pack_yaml(&pack_root, body);
    let workspace = tmp.path().join("ws");
    let opts = SyncOptions::new().with_workspace(Some(workspace));
    let report = sync_run(&pack_root, &opts).expect("report");
    let Some(SyncError::Halted(ctx)) = report.halted else {
        panic!("expected Halted, got {:?}", report.halted);
    };
    let HaltedContext { error, .. } = *ctx;
    match error {
        ExecError::ExecNonZero { status, stderr, .. } => {
            assert_eq!(status, 3);
            assert!(
                stderr.contains("GREX_STDERR_MARKER"),
                "expected captured stderr to contain marker, got: {stderr:?}"
            );
        }
        other => panic!("expected ExecNonZero, got {other:?}"),
    }
}

// -------- bonus: no-op sync writes no spurious audit events -------------

#[test]
fn noop_pack_writes_no_action_events() {
    let tmp = TempDir::new().unwrap();
    let pack_root = tmp.path().join("pack");
    write_pack_yaml(&pack_root, NOOP_PACK);
    let workspace = tmp.path().join("ws");

    let opts = SyncOptions::new().with_workspace(Some(workspace));
    sync_run(&pack_root, &opts).expect("sync ok");

    let log = pack_root.join(".grex").join("grex.jsonl");
    let events = if log.exists() { read_all(&log).unwrap() } else { Vec::new() };
    assert!(!events.iter().any(|e| matches!(e, Event::ActionStarted { .. })));
    assert!(!events.iter().any(|e| matches!(e, Event::ActionCompleted { .. })));
    assert!(!events.iter().any(|e| matches!(e, Event::ActionHalted { .. })));
}
