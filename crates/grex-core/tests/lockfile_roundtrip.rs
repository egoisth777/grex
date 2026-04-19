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
            LockEntry {
                id,
                sha: format!("sha-{i}"),
                branch: "main".into(),
                installed_at: ts,
                actions_hash: format!("h-{i}"),
                schema_version: "1".into(),
            },
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
    map.insert(
        "pack-ns".into(),
        LockEntry {
            id: "pack-ns".into(),
            sha: "abc".into(),
            branch: "main".into(),
            installed_at: ts,
            actions_hash: "h".into(),
            schema_version: "1".into(),
        },
    );

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
        map.insert(
            id.to_string(),
            LockEntry {
                id: id.to_string(),
                sha: "sha".into(),
                branch: "main".into(),
                installed_at: ts,
                actions_hash: "h".into(),
                schema_version: "1".into(),
            },
        );
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
