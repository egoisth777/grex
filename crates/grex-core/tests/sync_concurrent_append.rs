//! Regression: `grex sync` action appends must serialize through
//! [`ManifestLock`]. Prior to PR B the sync driver called `append_event`
//! directly, so two cooperating syncs could interleave lines in
//! `.grex/grex.jsonl`. This suite pins the lock-wrapped append path.
//!
//! Two threads sharing one event log each issue 50 appends. After join the
//! file must parse line-for-line as valid JSON with exactly 100 events.
//! The test uses the `__test_append_sync_event` hook so the locking logic
//! is exercised without a full pack-tree walk.

use std::path::PathBuf;
use std::sync::Arc;
use std::thread;

use grex_core::manifest::read_all;
use grex_core::sync::__test_append_sync_event;
use tempfile::tempdir;

const PER_THREAD: usize = 50;

#[test]
fn two_threads_sync_append_are_serialised() {
    let dir = tempdir().unwrap();
    let log = Arc::new(dir.path().join(".grex").join("grex.jsonl"));
    let lock = Arc::new(dir.path().join(".grex").join(".grex.lock"));

    let handles: Vec<_> = (0..2)
        .map(|tid| {
            let log = Arc::clone(&log);
            let lock = Arc::clone(&lock);
            thread::spawn(move || -> Vec<String> {
                let mut failures = Vec::new();
                for i in 0..PER_THREAD {
                    let pack = format!("t{tid}");
                    let action = format!("act-{i}");
                    if let Err(e) = __test_append_sync_event(&log, &lock, &pack, &action) {
                        failures.push(e);
                    }
                }
                failures
            })
        })
        .collect();

    for h in handles {
        let failures = h.join().unwrap();
        assert!(failures.is_empty(), "unexpected append failures: {failures:?}");
    }

    // 1. `read_all` must succeed — no torn lines.
    let events = read_all(&log).expect("event log parses cleanly");
    assert_eq!(events.len(), 2 * PER_THREAD, "total event count");

    // 2. Every line of the raw file must independently parse as JSON
    //    (defense-in-depth vs. read_all's heal-on-tail behaviour).
    let raw = std::fs::read_to_string(&*log).unwrap();
    let line_count = raw.lines().count();
    assert_eq!(line_count, 2 * PER_THREAD, "raw line count matches event count");
    for (i, line) in raw.lines().enumerate() {
        let _: serde_json::Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("line {i} is not valid JSON: {e}: `{line}`"));
    }
}

#[test]
fn sync_append_creates_parent_dir_lazily() {
    // First call on a fresh tempdir must create `.grex/` on demand.
    let dir = tempdir().unwrap();
    let log: PathBuf = dir.path().join(".grex").join("grex.jsonl");
    let lock: PathBuf = dir.path().join(".grex").join(".grex.lock");
    assert!(!log.parent().unwrap().exists(), "precondition: .grex/ absent");
    __test_append_sync_event(&log, &lock, "pack", "act").expect("first append");
    assert!(log.exists(), "log created");
    assert!(lock.exists(), "lock sidecar created");
}
