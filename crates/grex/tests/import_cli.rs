//! feat-m7-4a — `grex import --from-repos-json` end-to-end.

mod common;

use common::grex;
use predicates::str::contains;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn fixture(dir: &TempDir, body: &str) -> PathBuf {
    let p = dir.path().join("REPOS.json");
    fs::write(&p, body).unwrap();
    p
}

fn manifest_path(dir: &TempDir) -> PathBuf {
    dir.path().join("grex.jsonl")
}

const SAMPLE: &str = r#"[
    {"url": "https://github.com/egoisth777/cfg.git", "path": "cfg"},
    {"url": "git@github.com:egoisth777/code.git", "path": "code"},
    {"url": "", "path": "scripts"}
]"#;

#[test]
fn import_from_repos_json_end_to_end_writes_three_rows() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, SAMPLE);
    let manifest = manifest_path(&dir);

    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("imported=3"));

    let raw = fs::read_to_string(&manifest).expect("manifest written");
    assert_eq!(raw.lines().count(), 3, "one JSONL row per entry");
    assert!(raw.contains(r#""path":"cfg""#));
    assert!(raw.contains(r#""path":"code""#));
    assert!(raw.contains(r#""path":"scripts""#));
    assert!(raw.contains(r#""type":"scripted""#));
    assert!(raw.contains(r#""type":"declarative""#));
}

#[test]
fn import_dry_run_prints_plan_and_leaves_manifest_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, SAMPLE);
    let manifest = manifest_path(&dir);

    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
            "--dry-run",
        ])
        .assert()
        .success()
        .stdout(contains("DRY-RUN: would add"))
        .stdout(contains("imported=3"));

    assert!(!manifest.exists(), "dry-run must not create the manifest");
}

#[test]
fn import_global_dry_run_flag_also_short_circuits() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, SAMPLE);
    let manifest = manifest_path(&dir);

    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .arg("--dry-run")
        .assert()
        .success();

    assert!(!manifest.exists());
}

#[test]
fn import_second_run_is_idempotent_via_skip() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, SAMPLE);
    let manifest = manifest_path(&dir);

    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success();
    let first = fs::read_to_string(&manifest).unwrap();

    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("imported=0"))
        .stdout(contains("skipped=3"));

    let second = fs::read_to_string(&manifest).unwrap();
    assert_eq!(first, second, "second run must be byte-equal");
}

#[test]
fn import_json_output_emits_structured_plan() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, SAMPLE);
    let manifest = manifest_path(&dir);

    let out = grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
            "--dry-run",
            "--json",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let stdout = String::from_utf8(out).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is JSON");
    assert_eq!(value["imported"].as_array().unwrap().len(), 3);
    assert_eq!(value["skipped"].as_array().unwrap().len(), 0);
    assert_eq!(value["imported"][0]["kind"], "scripted");
    assert_eq!(value["imported"][2]["kind"], "declarative");
    assert_eq!(value["imported"][0]["would_dispatch"], true);
}

#[test]
fn import_missing_input_file_fails_with_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest_path(&dir);
    grex()
        .args([
            "import",
            "--from-repos-json",
            "does-not-exist.json",
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

#[test]
fn import_malformed_json_fails_with_nonzero_exit() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, "not valid json");
    let manifest = manifest_path(&dir);
    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .failure();
}

#[test]
fn import_without_from_flag_errors() {
    grex().args(["import"]).assert().failure();
}

#[test]
fn import_collision_path_is_reported_on_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = manifest_path(&dir);
    let seed = "{\"op\":\"add\",\"ts\":\"2026-04-22T00:00:00Z\",\"id\":\"cfg\",\"url\":\"pre\",\"path\":\"cfg\",\"type\":\"declarative\",\"schema_version\":\"1\"}\n";
    fs::write(&manifest, seed).unwrap();

    let input = fixture(
        &dir,
        r#"[
            {"url": "https://x/cfg.git", "path": "cfg"},
            {"url": "", "path": "only-new"}
        ]"#,
    );

    let assertion = grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("imported=1"))
        .stdout(contains("skipped=1"));

    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        stderr.contains("path-collision") && stderr.contains("cfg"),
        "expected collision warning on stderr, got: {stderr}"
    );
}

#[test]
fn import_empty_array_succeeds_with_no_writes() {
    let dir = tempfile::tempdir().unwrap();
    let input = fixture(&dir, "[]");
    let manifest = manifest_path(&dir);
    grex()
        .args([
            "import",
            "--from-repos-json",
            input.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success()
        .stdout(contains("imported=0"));
    assert!(!manifest.exists());
}
