//! CLI coverage for `grex init` (v1.4.0).

mod common;

use common::grex;
use predicates::str::contains;
use std::fs;

#[test]
fn init_writes_minimal_manifest_skeleton() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ws");
    grex().args(["init"]).arg(&target).assert().success();
    let manifest = target.join(".grex/pack.yaml");
    assert!(manifest.is_file(), "pack.yaml must be written");
    let raw = fs::read_to_string(&manifest).unwrap();
    assert!(raw.contains("schema_version: \"1\""));
    assert!(raw.contains("type: meta"));
    assert!(raw.contains("name:"));
}

#[test]
fn init_idempotency_exits_one_and_does_not_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    grex().args(["init"]).arg(dir.path()).assert().success();
    let manifest_path = dir.path().join(".grex/pack.yaml");
    let before = fs::read_to_string(&manifest_path).unwrap();
    grex()
        .args(["init"])
        .arg(dir.path())
        .assert()
        .failure()
        .code(1)
        .stderr(contains("already initialized"));
    let after = fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(before, after, "second init must not mutate the manifest");
}

#[test]
fn init_cwd_default_when_no_path_given() {
    let dir = tempfile::tempdir().unwrap();
    grex().current_dir(dir.path()).arg("init").assert().success();
    assert!(dir.path().join(".grex/pack.yaml").is_file());
}

#[test]
fn init_json_envelope_carries_path_and_manifest_keys() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("ws");
    let out = grex().args(["--json", "init"]).arg(&target).assert().success().get_output().clone();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert_eq!(v["verb"].as_str(), Some("init"));
    assert_eq!(v["status"].as_str(), Some("ok"));
    assert!(v.get("manifest").is_some());
    assert!(v.get("path").is_some());
}
