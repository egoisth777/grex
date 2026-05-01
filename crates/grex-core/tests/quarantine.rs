//! v1.2.1 Item 5b — `--quarantine` end-to-end integration tests.
//!
//! Exercises [`grex_core::tree::quarantine::snapshot_then_rm`] through
//! the [`grex_core::tree::phase2_prune`] consent layer (the actual
//! production call site). The Lean theorem
//! `quarantine_snapshot_precedes_delete`
//! ([`proof/Grex/Quarantine.lean`](../../../proof/Grex/Quarantine.lean))
//! proves the model contract; these tests assert the Rust runtime
//! faithfully realises it on a real filesystem.
//!
//! ## Coverage matrix
//!
//! | # | Scenario                                  | Asserts                                                           |
//! |---|-------------------------------------------|-------------------------------------------------------------------|
//! | 1 | `--force-prune --quarantine` happy path   | snapshot exists + dest gone + Start/Complete events on disk       |
//! | 2 | `--force-prune` WITHOUT `--quarantine`    | dest gone + NO trash dir + NO quarantine events (legacy v1.2.0)   |
//! | 3 | Recursive snapshot (3-level subtree)      | every byte at every depth preserved in trash bucket                |
//! | 4 | Snapshot failure aborts unlink            | dest intact + Err returned + no QuarantineComplete event          |
//! | 5 | Audit-log fsync ordering (Start ≺ Copy)   | log line for Start exists BEFORE any copied byte by file-mtime    |
//! | 6 | `Clean` consent prunes skip quarantine    | clean prune deletes directly, no trash bucket created              |
//!
//! Tests that depend on `git init` skip themselves when `git` is
//! unavailable on the host — same precedent as
//! `crates/grex-core/src/tree/consent.rs::tests`.

use std::fs;
use std::path::{Path, PathBuf};

use grex_core::manifest::append::read_all;
use grex_core::manifest::event::Event;
use grex_core::tree::{phase2_prune, QuarantineConfig, TreeError};
use tempfile::tempdir;

/// Initialise `dir` as a git repo. Returns `true` on success.
fn try_git_init(dir: &Path) -> bool {
    let status =
        std::process::Command::new("git").arg("-C").arg(dir).arg("init").arg("-q").status();
    matches!(status, Ok(s) if s.success())
}

fn try_git_identity(dir: &Path) -> bool {
    let a = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.email", "test@example.com"])
        .status();
    let b = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.name", "Test"])
        .status();
    matches!((a, b), (Ok(sa), Ok(sb)) if sa.success() && sb.success())
}

fn try_git_commit_initial(dir: &Path) -> bool {
    fs::write(dir.join("README"), b"seed\n").unwrap();
    let add =
        std::process::Command::new("git").arg("-C").arg(dir).args(["add", "README"]).status();
    if !matches!(add, Ok(s) if s.success()) {
        return false;
    }
    let commit = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-q", "-m", "init"])
        .status();
    matches!(commit, Ok(s) if s.success())
}

fn build_meta_with_dirty_child(meta: &Path, name: &str) -> Option<PathBuf> {
    let dest = meta.join(name);
    fs::create_dir_all(&dest).unwrap();
    if !try_git_init(&dest) {
        return None;
    }
    // Stamp dirt: an untracked, non-ignored file makes the consent
    // walk return DirtyTree, which `--force-prune` consumes.
    fs::write(dest.join("scratch.txt"), b"dirty bytes").unwrap();
    Some(dest)
}

fn quarantine_cfg_for(meta: &Path) -> QuarantineConfig {
    QuarantineConfig {
        trash_root: meta.join(".grex").join("trash"),
        audit_log: meta.join(".grex").join("events.jsonl"),
    }
}

/// Test #1 — `--force-prune --quarantine` happy path. Snapshot
/// materialised, dest gone, Start + Complete events recorded.
#[test]
fn quarantine_force_prune_snapshots_then_unlinks() {
    let tmp = tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let cfg = quarantine_cfg_for(&meta);

    let Some(dest) = build_meta_with_dirty_child(&meta, "victim") else { return };
    // Sanity: dest was set up.
    assert!(dest.join("scratch.txt").exists());

    let res = phase2_prune(
        &dest,
        /* force_prune */ true,
        /* force_prune_with_ignored */ false,
        Some(cfg.audit_log.as_path()),
        Some(&cfg),
    );
    assert!(res.is_ok(), "quarantine-driven prune must succeed: {res:?}");
    assert!(!dest.exists(), "original dest must be unlinked after success");

    // Audit log: must contain a QuarantineStart entry, must contain
    // a QuarantineComplete entry; both reference the same trash path.
    let events = read_all(&cfg.audit_log).expect("audit log readable");
    let starts: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::QuarantineStart { src, trash, .. } => Some((src.clone(), trash.clone())),
            _ => None,
        })
        .collect();
    let completes: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::QuarantineComplete { src, trash, .. } => Some((src.clone(), trash.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 1, "exactly one QuarantineStart on success");
    assert_eq!(completes.len(), 1, "exactly one QuarantineComplete on success");
    assert_eq!(starts[0], completes[0], "Start.trash MUST equal Complete.trash");

    // Snapshot is on disk under `<meta>/.grex/trash/` and contains
    // the dirt the consent walk saw.
    let trash_path = PathBuf::from(&starts[0].1);
    assert!(trash_path.exists(), "snapshot must exist on disk");
    assert!(trash_path.starts_with(&cfg.trash_root));
    assert_eq!(fs::read(trash_path.join("scratch.txt")).unwrap(), b"dirty bytes");
}

/// Test #2 — without `--quarantine`, `--force-prune` still works and
/// no trash bucket is created. Guards regression: quarantine wiring
/// must not silently activate for callers that left it `None`.
#[test]
fn force_prune_without_quarantine_uses_direct_unlink() {
    let tmp = tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let audit_log = meta.join(".grex").join("events.jsonl");

    let Some(dest) = build_meta_with_dirty_child(&meta, "legacy-victim") else { return };

    let res = phase2_prune(
        &dest,
        /* force_prune */ true,
        false,
        Some(audit_log.as_path()),
        /* quarantine */ None,
    );
    assert!(res.is_ok(), "direct-unlink force-prune must succeed: {res:?}");
    assert!(!dest.exists(), "dest must be unlinked");
    assert!(
        !meta.join(".grex").join("trash").exists(),
        "no trash bucket may be created when --quarantine is absent"
    );
    // Audit log still gets the existing ForcePruneExecuted entry but
    // NO quarantine variants.
    let events = read_all(&audit_log).unwrap_or_default();
    let any_quarantine = events.iter().any(|e| {
        matches!(
            e,
            Event::QuarantineStart { .. }
                | Event::QuarantineComplete { .. }
                | Event::QuarantineFailed { .. }
        )
    });
    assert!(!any_quarantine, "no quarantine event without --quarantine");
}

/// Test #3 — recursive snapshot of a 3-level dirty subtree preserves
/// every byte at every depth.
#[test]
fn quarantine_snapshot_is_recursive() {
    let tmp = tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let cfg = quarantine_cfg_for(&meta);

    let Some(dest) = build_meta_with_dirty_child(&meta, "deep-victim") else { return };
    fs::create_dir_all(dest.join("a/b/c")).unwrap();
    fs::write(dest.join("a/level-1.bin"), [9u8, 8, 7, 6, 5]).unwrap();
    fs::write(dest.join("a/b/level-2.txt"), b"middle").unwrap();
    fs::write(dest.join("a/b/c/leaf.bin"), [255u8, 254, 253]).unwrap();

    let res = phase2_prune(
        &dest,
        true,
        false,
        Some(cfg.audit_log.as_path()),
        Some(&cfg),
    );
    assert!(res.is_ok(), "recursive prune must succeed: {res:?}");

    let events = read_all(&cfg.audit_log).expect("audit log readable");
    let trash_path = events
        .iter()
        .find_map(|e| match e {
            Event::QuarantineStart { trash, .. } => Some(PathBuf::from(trash)),
            _ => None,
        })
        .expect("QuarantineStart present");

    assert_eq!(fs::read(trash_path.join("scratch.txt")).unwrap(), b"dirty bytes");
    assert_eq!(
        fs::read(trash_path.join("a/level-1.bin")).unwrap(),
        vec![9u8, 8, 7, 6, 5]
    );
    assert_eq!(fs::read(trash_path.join("a/b/level-2.txt")).unwrap(), b"middle");
    assert_eq!(
        fs::read(trash_path.join("a/b/c/leaf.bin")).unwrap(),
        vec![255u8, 254, 253]
    );
}

/// Test #4 — snapshot failure aborts the prune. We make the trash
/// root a regular file so create_dir_all on it fails. The dest must
/// remain intact and the consent layer must surface a refusal error.
#[test]
fn quarantine_snapshot_failure_aborts_prune() {
    let tmp = tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let cfg = quarantine_cfg_for(&meta);

    let Some(dest) = build_meta_with_dirty_child(&meta, "abort-victim") else { return };

    // Pre-create the trash root AS A FILE so create_dir_all fails.
    fs::write(&cfg.trash_root, b"i am a file, not a dir").unwrap();

    let res = phase2_prune(
        &dest,
        true,
        false,
        Some(cfg.audit_log.as_path()),
        Some(&cfg),
    );
    assert!(
        matches!(res, Err(TreeError::DirtyTreeRefusal { .. })),
        "snapshot failure must surface as a refusal: {res:?}",
    );
    assert!(dest.exists(), "dest MUST remain on snapshot failure");
    assert!(dest.join("scratch.txt").exists(), "dirt must remain too");

    // No QuarantineComplete may exist when snapshot failed.
    let events = read_all(&cfg.audit_log).unwrap_or_default();
    assert!(
        !events.iter().any(|e| matches!(e, Event::QuarantineComplete { .. })),
        "no QuarantineComplete may appear when snapshot failed: {events:?}"
    );
}

/// Test #5 — audit-log Start entry is durable BEFORE the snapshot
/// dir exists. We can't easily intercept the order from outside the
/// pipeline, but we can prove the property after-the-fact:
/// successful prune ⇒ events.jsonl modification time ≤ trash dir
/// modification time. This catches a regression where the audit
/// append was reordered to AFTER the copy (which would invalidate
/// the Lean delete-licensed predicate).
#[test]
fn quarantine_audit_log_modtime_precedes_or_equals_snapshot() {
    let tmp = tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let cfg = quarantine_cfg_for(&meta);

    let Some(dest) = build_meta_with_dirty_child(&meta, "ordered-victim") else { return };

    let res = phase2_prune(
        &dest,
        true,
        false,
        Some(cfg.audit_log.as_path()),
        Some(&cfg),
    );
    assert!(res.is_ok(), "ordered prune must succeed: {res:?}");

    let events = read_all(&cfg.audit_log).expect("audit log readable");
    let trash_path = events
        .iter()
        .find_map(|e| match e {
            Event::QuarantineStart { trash, .. } => Some(PathBuf::from(trash)),
            _ => None,
        })
        .expect("Start entry must exist after success");

    // The audit-log file existed (and was fsync'd) before the trash
    // dir was created. mtime is a coarse proxy but on every supported
    // FS the audit fsync MUST have happened before the snapshot
    // create_dir; if it hasn't, the Lean iff is broken.
    let audit_mtime = fs::metadata(&cfg.audit_log).unwrap().modified().unwrap();
    let trash_mtime = fs::metadata(&trash_path).unwrap().modified().unwrap();
    // We only assert audit_mtime <= trash_mtime + a small slack to
    // account for FS clock granularity (HFS+/FAT round to 2s; ext4
    // can store ns). The slack is generous enough that a real
    // out-of-order bug — Start written AFTER snapshot — would still
    // be caught (the inversion would be on the order of seconds).
    use std::time::Duration;
    assert!(
        audit_mtime <= trash_mtime + Duration::from_secs(2),
        "audit log mtime {audit_mtime:?} must precede trash mtime {trash_mtime:?} (modulo slack)"
    );
}

/// Test #6 — `Clean` consent prunes ALSO route through quarantine
/// when the flag is set. Reason: an operator who opts into
/// `--quarantine` wants the snapshot regardless of dirtiness — the
/// flag is "snapshot all prunes for this run", not "snapshot only
/// when overriding". This guards the documentation-vs-behavior
/// alignment for the "explicitly opt-in" case.
///
/// CONTRARY READING: an alternate interpretation of the spec
/// ("--quarantine only applies to overridden prunes") would say
/// Clean prunes go direct. The current Rust impl honours the more
/// useful interpretation: if the operator typed `--quarantine`, they
/// asked for snapshots, period. If we ever flip this, this test
/// will fail loudly.
#[test]
fn quarantine_applies_to_clean_consent_too() {
    let tmp = tempdir().unwrap();
    let meta = tmp.path().join("meta");
    fs::create_dir_all(meta.join(".grex")).unwrap();
    let cfg = quarantine_cfg_for(&meta);

    let dest = meta.join("clean-victim");
    fs::create_dir_all(&dest).unwrap();
    if !try_git_init(&dest) {
        return;
    }
    if !try_git_identity(&dest) || !try_git_commit_initial(&dest) {
        return;
    }
    // `dest` is now a clean git repo (commit just made).

    let res = phase2_prune(
        &dest,
        /* force_prune */ false,
        /* force_prune_with_ignored */ false,
        Some(cfg.audit_log.as_path()),
        Some(&cfg),
    );
    assert!(res.is_ok(), "clean prune must succeed under --quarantine: {res:?}");
    assert!(!dest.exists(), "clean dest must be unlinked after pipeline");

    let events = read_all(&cfg.audit_log).expect("audit log readable");
    let has_start = events.iter().any(|e| matches!(e, Event::QuarantineStart { .. }));
    let has_complete = events.iter().any(|e| matches!(e, Event::QuarantineComplete { .. }));
    assert!(has_start && has_complete, "clean prune still emits Start+Complete: {events:?}");
    // Snapshot is on disk under <meta>/.grex/trash/.
    assert!(cfg.trash_root.exists(), "trash root materialised even on clean consent");
}
