//! CLI coverage for `grex add` manifest registration.

mod common;

use common::grex;
use predicates::str::contains;
use std::fs;

#[test]
fn add_url_only_writes_manifest_and_infers_path() {
    let dir = tempfile::tempdir().unwrap();

    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/org/repo.git"])
        .assert()
        .success()
        .stdout(contains("added"));

    let raw = fs::read_to_string(dir.path().join(".grex/events.jsonl")).expect("manifest written");
    assert_eq!(raw.lines().count(), 1);
    assert!(raw.contains(r#""id":"repo""#));
    assert!(raw.contains(r#""url":"https://example.com/org/repo.git""#));
    assert!(raw.contains(r#""path":"repo""#));
    assert!(raw.contains(r#""type":"scripted""#));
}

#[test]
fn add_explicit_path_is_preserved() {
    let dir = tempfile::tempdir().unwrap();

    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/org/repo.git", "custom-path"])
        .assert()
        .success()
        .stdout(contains("custom-path"));

    let raw = fs::read_to_string(dir.path().join(".grex/events.jsonl")).expect("manifest written");
    assert!(raw.contains(r#""id":"custom-path""#));
    assert!(raw.contains(r#""path":"custom-path""#));
}

#[test]
fn add_global_dry_run_does_not_write_manifest() {
    let dir = tempfile::tempdir().unwrap();

    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/org/repo.git", "--dry-run"])
        .assert()
        .success()
        .stdout(contains("DRY-RUN: would add"));

    assert!(!dir.path().join(".grex/events.jsonl").exists());
}

// ---------- v1.4.0 B15 — path-collision guard ----------

#[test]
fn add_path_collision_exits_one_and_does_not_append() {
    let dir = tempfile::tempdir().unwrap();

    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/a/first.git", "shared"])
        .assert()
        .success();

    let before = fs::read_to_string(dir.path().join(".grex/events.jsonl")).unwrap();

    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/b/second.git", "shared"])
        .assert()
        .failure()
        .code(1)
        .stderr(contains("already registered"));

    let after = fs::read_to_string(dir.path().join(".grex/events.jsonl")).unwrap();
    assert_eq!(before, after, "second add must NOT append on path collision");
}

#[test]
fn add_path_collision_json_envelope_carries_existing_url() {
    let dir = tempfile::tempdir().unwrap();

    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/a/first.git", "shared"])
        .assert()
        .success();

    let out = grex()
        .current_dir(dir.path())
        .args(["--json", "add", "https://example.com/b/second.git", "shared"])
        .assert()
        .failure()
        .code(1)
        .get_output()
        .clone();

    let stdout = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&stdout).expect("collision --json envelope must be valid JSON");
    assert_eq!(v["verb"].as_str(), Some("add"));
    assert_eq!(v.pointer("/error/kind").and_then(|x| x.as_str()), Some("path_collision"));
    assert_eq!(
        v.pointer("/error/existing_url").and_then(|x| x.as_str()),
        Some("https://example.com/a/first.git")
    );
}
