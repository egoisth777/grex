//! v1.3.2 W1 — `LockEntry.synthetic` retirement.
//!
//! `pack-spec.md §v1.2.0` retired sync-time auto-synthesis. v1.3.2
//! stops emitting the lockfile field that tracked it: the writer's
//! `skip_serializing_if` predicate always omits `synthetic` from
//! serialized output, while `#[serde(default)]` keeps v1.1.x lockfiles
//! that still carry `synthetic: true` forward-readable.
//!
//! This integration test pins the three observable contracts:
//!
//! - **(a) No-emit.** A fresh on-disk lockfile written from an in-memory
//!   map of entries — including entries whose in-memory `synthetic` is
//!   `true` — contains no `synthetic` key on any line.
//! - **(b) Forward-read.** A hand-crafted v1.1.x lockfile line carrying
//!   `"synthetic": true` deserialises cleanly with `synthetic = true`
//!   on the resulting `LockEntry`. Existing lockfiles on operator
//!   disks remain readable.
//! - **(c) Round-trip drops.** Serialising the just-read v1.1.x entry
//!   omits the `synthetic` key (writer skips on output), and the
//!   subsequent re-read restores `synthetic = false` via
//!   `#[serde(default)]`. The field is write-suppressed end-to-end.

use chrono::{TimeZone, Utc};
use grex_core::lockfile::{read_lockfile, write_lockfile, LockEntry};
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

/// Hand-rolled v1.1.x JSONL line with `"synthetic": true` for the
/// forward-read regression. Keeps the format stable so any future
/// schema drift in the live writer cannot mask a regression here.
const LEGACY_SYNTHETIC_LINE: &str = r#"{"id":"plain-git","path":"plain-git","sha":"deadbeef","branch":"main","installed_at":"2026-04-27T10:00:00Z","actions_hash":"","schema_version":"1","synthetic":true}"#;

/// (a) No-emit: writing a fresh lockfile containing entries with
/// `synthetic = true` in memory MUST NOT serialise the `synthetic` key
/// for any entry.
#[test]
fn writer_emits_no_synthetic_key_for_any_entry() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    let ts = Utc.with_ymd_and_hms(2026, 5, 3, 10, 0, 0).unwrap();

    let mut map: HashMap<String, LockEntry> = HashMap::new();
    for i in 0..7 {
        let id = format!("pack-{i}");
        let mut entry =
            LockEntry::new(id.clone(), format!("sha-{i}"), "main", ts, format!("h-{i}"), "1");
        // Mirror the v1.3.0/v1.3.1 writer bug: 6 of 7 entries had
        // `synthetic = true` in memory before reaching the writer.
        entry.synthetic = i != 0;
        map.insert(id, entry);
    }
    write_lockfile(&p, &map).unwrap();

    let raw = fs::read_to_string(&p).unwrap();
    assert!(
        !raw.contains("synthetic"),
        "fresh lockfile must NOT carry the `synthetic` key on any line; got:\n{raw}",
    );

    // Sanity: every other required field is still present so we know
    // the assertion is not vacuously passing on an empty file.
    assert!(raw.contains(r#""id":"pack-0""#));
    assert!(raw.contains(r#""branch":"main""#));
    assert!(raw.contains(r#""schema_version":"1""#));
}

/// (b) Forward-read: a v1.1.x line carrying `"synthetic": true`
/// deserialises with `synthetic = true` via `#[serde(default)]`.
#[test]
fn reader_tolerates_legacy_synthetic_true() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    fs::write(&p, format!("{LEGACY_SYNTHETIC_LINE}\n")).unwrap();

    let back = read_lockfile(&p).unwrap();
    let entry = back.get("plain-git").expect("legacy entry must parse");
    assert!(entry.synthetic, "v1.1.x `synthetic: true` must round-trip into the in-memory entry");
    assert_eq!(entry.id, "plain-git");
    assert_eq!(entry.branch, "main");
    assert_eq!(entry.sha, "deadbeef");
}

/// (c) Round-trip drop: read a v1.1.x synthetic line, write it back
/// out, and assert the rewritten file no longer carries `synthetic`.
/// Subsequent re-read deserialises with `synthetic = false`.
#[test]
fn round_trip_drops_synthetic_on_rewrite() {
    let dir = tempdir().unwrap();
    let src = dir.path().join("legacy.jsonl");
    let dst = dir.path().join("rewritten.jsonl");
    fs::write(&src, format!("{LEGACY_SYNTHETIC_LINE}\n")).unwrap();

    // Read v1.1.x → in-memory map (synthetic == true).
    let map = read_lockfile(&src).unwrap();
    assert!(map["plain-git"].synthetic, "fixture invariant: legacy entry must have synthetic=true");

    // Write back via v1.3.2 writer.
    write_lockfile(&dst, &map).unwrap();

    let raw = fs::read_to_string(&dst).unwrap();
    assert!(!raw.contains("synthetic"), "rewritten lockfile must drop `synthetic`; got:\n{raw}",);

    // Re-read the rewritten file: `serde(default)` restores false.
    let back = read_lockfile(&dst).unwrap();
    assert!(
        !back["plain-git"].synthetic,
        "rewritten + re-read entry must have synthetic=false (writer skipped, reader defaulted)",
    );
    // Other fields still survive the round-trip unchanged.
    assert_eq!(back["plain-git"].sha, "deadbeef");
    assert_eq!(back["plain-git"].branch, "main");
}
