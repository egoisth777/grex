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

    let raw = fs::read_to_string(dir.path().join("grex.jsonl")).expect("manifest written");
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

    let raw = fs::read_to_string(dir.path().join("grex.jsonl")).expect("manifest written");
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

    assert!(!dir.path().join("grex.jsonl").exists());
}
