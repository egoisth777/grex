//! Integration: lockfile write → read → equality.

use chrono::{TimeZone, Timelike, Utc};
use grex_core::lockfile::{read_lockfile, write_lockfile, LockEntry, LockfileError};
use std::collections::HashMap;
use std::fs;
use tempfile::tempdir;

#[test]
fn lockfile_roundtrip_many() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap();

    let mut map = HashMap::new();
    for i in 0..50 {
        let id = format!("pack-{i}");
        map.insert(
            id.clone(),
            LockEntry::new(id, format!("sha-{i}"), "main", ts, format!("h-{i}"), "1"),
        );
    }
    write_lockfile(&p, &map).unwrap();
    let back = read_lockfile(&p).unwrap();
    assert_eq!(back, map);
}

#[test]
fn empty_lockfile_roundtrip() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    let map: HashMap<String, LockEntry> = HashMap::new();

    write_lockfile(&p, &map).unwrap();
    let back = read_lockfile(&p).unwrap();
    assert!(back.is_empty());
    assert_eq!(back, map);
}

#[test]
fn timestamp_precision_preserved() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    let ts =
        Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap().with_nanosecond(123_456_789).unwrap();

    let mut map = HashMap::new();
    map.insert("pack-ns".into(), LockEntry::new("pack-ns", "abc", "main", ts, "h", "1"));

    write_lockfile(&p, &map).unwrap();
    let back = read_lockfile(&p).unwrap();
    assert_eq!(back.get("pack-ns").unwrap().installed_at, ts);
    assert_eq!(back.get("pack-ns").unwrap().installed_at.nanosecond(), 123_456_789);
}

#[test]
fn unicode_pack_ids_roundtrip() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap();

    let ids = ["パック-1", "grex-αβγ", "🎯-pack"];
    let mut map = HashMap::new();
    for id in ids {
        map.insert(id.to_string(), LockEntry::new(id, "sha", "main", ts, "h", "1"));
    }

    write_lockfile(&p, &map).unwrap();
    let back = read_lockfile(&p).unwrap();
    assert_eq!(back, map);
    for id in ids {
        assert_eq!(back.get(id).unwrap().id, id);
    }
}

#[test]
fn malformed_lockfile_returns_err() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    fs::write(&p, b"}not json{").unwrap();

    let result = read_lockfile(&p);
    assert!(matches!(result, Err(LockfileError::Corruption { .. })));
}

/// v1.1.1 — `LockEntry::synthetic = true` survives a JSONL round-trip.
#[test]
fn synthetic_field_roundtrips() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    let ts = Utc.with_ymd_and_hms(2026, 4, 27, 10, 0, 0).unwrap();

    let mut entry = LockEntry::new("plain-git-child", "deadbeef", "main", ts, "h", "1");
    entry.synthetic = true;
    let mut map = HashMap::new();
    map.insert(entry.id.clone(), entry.clone());
    write_lockfile(&p, &map).unwrap();
    let back = read_lockfile(&p).unwrap();
    assert_eq!(back.get("plain-git-child"), Some(&entry));
    assert!(back.get("plain-git-child").unwrap().synthetic);
}

/// v1.1.1 forward-compat — a v1.1.0-shaped JSONL line (no `synthetic`
/// field) deserialises with `synthetic = false` thanks to
/// `#[serde(default)]`.
#[test]
fn missing_synthetic_field_defaults_to_false() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    // Hand-craft a v1.1.0 line — note the absence of `synthetic`.
    let line = r#"{"id":"legacy","sha":"abc","branch":"main","installed_at":"2026-04-19T10:00:00Z","actions_hash":"h","schema_version":"1"}"#;
    fs::write(&p, format!("{line}\n")).unwrap();

    let back = read_lockfile(&p).unwrap();
    let entry = back.get("legacy").expect("legacy entry must parse");
    assert!(!entry.synthetic, "missing field must deserialise to false");
    assert_eq!(entry.id, "legacy");
    assert_eq!(entry.sha, "abc");
}
