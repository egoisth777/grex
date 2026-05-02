//! v1.2.5 T-R1 — quarantine retention + restore integration tests.
//!
//! Exercises the v1.2.5 retention-policy contract documented in the
//! OpenSpec design.md (`openspec/changes/feat-v1.2.5-cleanup-deadlock-quarantine/design.md`)
//! §"Quarantine GC + restore + retention algorithm":
//!
//! - GC sweep deletes only trash entries whose embedded ISO8601
//!   timestamp is older than `retain_days`. Retained entries (newer
//!   than the cutoff) are left intact.
//! - `retain_days = 0` is the explicit "no GC" case (preserves the
//!   v1.2.1 indefinite-retention default behavior).
//! - `restore_quarantine` round-trips a snapshot back to its original
//!   dest and emits a `QuarantineRestored` audit event.
//!
//! The age of a trash entry is read from the **directory name** (per
//! `parse_iso8601_quarantine` in design.md), NOT from the on-disk
//! mtime. So fixture setup just needs to materialise trash subdirs
//! with appropriately-dated names — no `filetime` crate or
//! filesystem-mtime hacking required.
//!
//! ## Coverage matrix
//!
//! | # | Test fn                                | Asserts                                                     |
//! |---|----------------------------------------|-------------------------------------------------------------|
//! | 1 | `sync_retains_recent_quarantine`        | fresh + borderline survive; stale deleted; 1 GCSwept event  |
//! | 2 | `sync_retention_zero_keeps_all`         | all 3 entries survive when `retain_days = 0`                |
//! | 3 | `quarantine_restore_round_trip`         | restore moves trash → dest + emits `QuarantineRestored`     |
//!
//! ## W4 dependency status
//!
//! At the time this test was authored, the W4 worker (quarantine GC +
//! restore + retention API) had not yet landed. The test compiles
//! against the design.md-spec'd API surface:
//!
//! - `grex_core::tree::quarantine::prune_quarantine(meta_dir, retain_days) -> Result<PruneReport>`
//! - `grex_core::tree::quarantine::restore_quarantine(meta_dir, ts, basename, force) -> Result<RestoreReport>`
//! - `grex_core::manifest::event::Event::QuarantineGcSwept { ts, entry, age_days }`
//!   (one event PER pruned entry, not a summary — confirmed via the
//!   v1.2.5 in-flight `manifest::event` shape)
//! - `grex_core::manifest::event::Event::QuarantineRestored { ts, src, dest }`
//! - `grex_core::tree::SyncMetaOptions::retain_days: Option<u32>`
//!
//! When W4 lands and these names diverge from the assumptions below,
//! the orchestrator should adjust the `use` lines + struct/enum field
//! patterns rather than rewriting the test logic.
//
// TODO(orchestrator): integrate once W4 lands. Specifically:
//   - confirm the `prune_quarantine` / `restore_quarantine` signatures
//   - confirm `Event::QuarantineGcSwept` + `Event::QuarantineRestored`
//     field names (this file assumes the design.md shapes)
//   - confirm `SyncMetaOptions::retain_days: Option<u32>` field name
//   - if the public `grex_core::sync::run` entry point is preferred
//     over `sync_meta` for the integration surface, swap the call site
//     in scenario 1.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Duration as ChronoDuration, Utc};
use grex_core::manifest::append::read_all;
use grex_core::manifest::event::Event;
use grex_core::tree::quarantine::{
    prune_quarantine, restore_quarantine, QuarantineConfig, RetentionConfig,
};
use tempfile::tempdir;

/// Build the `<meta>/.grex/` skeleton + a `QuarantineConfig` rooted
/// under it. Mirrors the helper used by `tests/quarantine.rs`.
fn setup_meta(tmp: &Path) -> (PathBuf, QuarantineConfig) {
    let meta = tmp.join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let cfg = QuarantineConfig {
        trash_root: meta.join(".grex").join("trash"),
        audit_log: meta.join(".grex").join("events.jsonl"),
    };
    (meta, cfg)
}

/// Format a UTC instant N days in the past in the path-safe ISO8601
/// shape that the v1.2.1 quarantine pipeline writes:
/// `YYYY-MM-DDTHH-MM-SS.sssZ` (colons replaced with hyphens, ms
/// precision). Matches `iso8601_utc_now` in
/// `crates/grex-core/src/tree/quarantine.rs`.
fn ts_n_days_ago(days: i64) -> String {
    (Utc::now() - ChronoDuration::days(days)).format("%Y-%m-%dT%H-%M-%S%.3fZ").to_string()
}

/// Materialise a single trash entry at `<trash_root>/<ts>/<basename>/`
/// containing one stub file. Mirrors the on-disk layout the real
/// `snapshot_then_rm` produces for a single-file dest. Returns the
/// snapshot path.
fn populate_trash_entry(trash_root: &Path, ts: &str, basename: &str) -> PathBuf {
    let snapshot = trash_root.join(ts).join(basename);
    fs::create_dir_all(&snapshot).unwrap();
    fs::write(snapshot.join("payload.txt"), b"quarantined-bytes").unwrap();
    snapshot
}

/// Scenario 1 — `sync_retains_recent_quarantine`.
///
/// Three trash entries seeded with timestamps 1 / 29 / 35 days old.
/// With `retain_days = 30`, the cutoff sits between 29 and 35:
///
/// - 1d old   → fresh, survives
/// - 29d old  → borderline (under cutoff), survives
/// - 35d old  → stale (older than cutoff), deleted
///
/// Asserts: fresh + borderline still on disk; stale gone; the audit
/// log records exactly one `QuarantineGcSwept` event (the variant is
/// emitted PER pruned entry per the in-flight W4 event shape, so 1
/// stale ⇒ 1 event).
#[test]
fn sync_retains_recent_quarantine() {
    let tmp = tempdir().unwrap();
    let (meta, cfg) = setup_meta(tmp.path());

    let ts_fresh = ts_n_days_ago(1);
    let ts_borderline = ts_n_days_ago(29);
    let ts_stale = ts_n_days_ago(35);

    let snapshot_fresh = populate_trash_entry(&cfg.trash_root, &ts_fresh, "fresh");
    let snapshot_borderline = populate_trash_entry(&cfg.trash_root, &ts_borderline, "borderline");
    let snapshot_stale = populate_trash_entry(&cfg.trash_root, &ts_stale, "stale");

    // Sanity: all three exist before the sweep.
    assert!(snapshot_fresh.exists());
    assert!(snapshot_borderline.exists());
    assert!(snapshot_stale.exists());

    // Run the GC sweep. Per design.md, `prune_quarantine(meta_dir, 30)`
    // deletes any `<ts>/` subdir whose parsed timestamp is older than
    // `now - 30 days`.
    let report = prune_quarantine(&meta, RetentionConfig { retain_days: 30 }, Some(&cfg.audit_log))
        .expect("prune_quarantine succeeds");

    // Survivors remain on disk; stale is gone. Use the parent `<ts>/`
    // dir for the existence check because the sweep deletes the whole
    // timestamp bucket, not just the basename leaf.
    assert!(cfg.trash_root.join(&ts_fresh).exists(), "fresh entry must survive");
    assert!(cfg.trash_root.join(&ts_borderline).exists(), "borderline entry must survive");
    assert!(!cfg.trash_root.join(&ts_stale).exists(), "stale entry must be deleted");

    // Report bookkeeping: 1 pruned, 2 retained.
    assert_eq!(report.pruned.len(), 1, "exactly one entry pruned: {report:?}");
    assert_eq!(report.retained.len(), 2, "exactly two entries retained: {report:?}");

    // Audit log: exactly one `QuarantineGCSwept` event for this sweep.
    let events = read_all(&cfg.audit_log).unwrap_or_default();
    let gc_events: Vec<_> =
        events.iter().filter(|e| matches!(e, Event::QuarantineGcSwept { .. })).collect();
    assert_eq!(gc_events.len(), 1, "exactly one QuarantineGCSwept event recorded: {events:?}",);
}

/// Scenario 2 — `sync_retention_zero_keeps_all`.
///
/// Same three-entry fixture as scenario 1. With `retain_days = 0`,
/// per design.md "Default = unset; v1.2.1 indefinite-retention
/// behavior preserved" — a zero retention window means "no GC fires".
/// All three entries survive regardless of age.
#[test]
fn sync_retention_zero_keeps_all() {
    let tmp = tempdir().unwrap();
    let (meta, cfg) = setup_meta(tmp.path());

    let ts_fresh = ts_n_days_ago(1);
    let ts_borderline = ts_n_days_ago(29);
    let ts_stale = ts_n_days_ago(35);

    let _ = populate_trash_entry(&cfg.trash_root, &ts_fresh, "fresh");
    let _ = populate_trash_entry(&cfg.trash_root, &ts_borderline, "borderline");
    let _ = populate_trash_entry(&cfg.trash_root, &ts_stale, "stale");

    let report = prune_quarantine(&meta, RetentionConfig { retain_days: 0 }, Some(&cfg.audit_log))
        .expect("prune with retain=0 is a no-op success");

    assert!(cfg.trash_root.join(&ts_fresh).exists(), "fresh survives under retain=0");
    assert!(cfg.trash_root.join(&ts_borderline).exists(), "borderline survives under retain=0");
    assert!(cfg.trash_root.join(&ts_stale).exists(), "stale survives under retain=0");
    assert_eq!(report.pruned.len(), 0, "no entries pruned under retain=0: {report:?}");
}

/// Scenario 3 — `quarantine_restore_round_trip`.
///
/// Seed a single trash entry; call `restore_quarantine(meta, ts,
/// Some(basename), force = false)`; assert the entry moved back to
/// `<meta>/<basename>/` byte-identical and a `QuarantineRestored`
/// audit event was appended.
#[test]
fn quarantine_restore_round_trip() {
    let tmp = tempdir().unwrap();
    let (meta, cfg) = setup_meta(tmp.path());

    let ts = ts_n_days_ago(2);
    let basename = "restored";
    let snapshot = populate_trash_entry(&cfg.trash_root, &ts, basename);
    assert!(snapshot.exists(), "fixture: snapshot must exist before restore");

    let dest = meta.join(basename);
    assert!(!dest.exists(), "precondition: dest must NOT exist before restore");

    let _report = restore_quarantine(&meta, &ts, Some(basename), false, Some(&cfg.audit_log))
        .expect("restore_quarantine succeeds for unambiguous, non-existing dest");

    // Post-condition: dest reconstructed at the meta root with the
    // original payload; trash entry moved (no longer present at the
    // snapshot path).
    assert!(dest.exists(), "dest must materialise after restore");
    assert_eq!(
        fs::read(dest.join("payload.txt")).expect("payload readable in restored dest"),
        b"quarantined-bytes",
    );
    assert!(!snapshot.exists(), "trash snapshot must be moved out (rename semantics)");

    // Audit log: a QuarantineRestored event was appended.
    let events = read_all(&cfg.audit_log).expect("audit log readable post-restore");
    let restored_events: Vec<_> =
        events.iter().filter(|e| matches!(e, Event::QuarantineRestored { .. })).collect();
    assert_eq!(
        restored_events.len(),
        1,
        "exactly one QuarantineRestored event recorded: {events:?}",
    );
}
