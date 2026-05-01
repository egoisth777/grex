//! End-to-end CLI tests for `grex doctor --scan-undeclared` (v1.2.1
//! item 4). Spawns the real `grex` binary via `assert_cmd`, prepares a
//! temp workspace with a mix of registered + untracked `.git/`
//! directories, and asserts the report block in stdout.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::fs;
use std::path::Path;
use std::process::Command;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary")
}

/// Make `dir` look like a git repo by writing a minimal `.git/HEAD`.
/// The fixture deliberately omits a remote so `git config --get
/// remote.origin.url` reports `[unknown]` in the doctor output.
fn fake_repo(dir: &Path) {
    fs::create_dir_all(dir.join(".git")).unwrap();
    fs::write(dir.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
}

/// Seed `<workspace>/.grex/pack.yaml` declaring the listed children
/// AND a matching `<workspace>/.grex/events.jsonl` that registers each
/// child via `Event::Add`. The pack.yaml is what `--scan-undeclared`
/// consults; the events.jsonl is what the standard doctor on-disk-drift
/// check consults. We seed both so the doctor's drift check does not
/// flag the registered children as unregistered (which would mask the
/// scan's own exit-code semantics in these tests).
fn write_meta_yaml(workspace: &Path, name: &str, children: &[(&str, &str)]) {
    let grex_dir = workspace.join(".grex");
    fs::create_dir_all(&grex_dir).unwrap();
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\n");
    if !children.is_empty() {
        yaml.push_str("children:\n");
        for (segment, url) in children {
            yaml.push_str(&format!("  - url: {url}\n    path: {segment}\n"));
        }
    }
    fs::write(grex_dir.join("pack.yaml"), yaml).unwrap();

    let events_path = grex_dir.join("events.jsonl");
    let mut log = String::new();
    for (segment, url) in children {
        log.push_str(&format!(
            r#"{{"op":"add","ts":"2026-04-30T10:00:00Z","id":"{segment}","url":"{url}","path":"{segment}","type":"meta","schema_version":"1"}}
"#,
        ));
    }
    fs::write(events_path, log).unwrap();
}

/// AC #1 — A workspace whose only `.git/` dirs belong to declared
/// children produces a "no undeclared" message.
#[test]
fn doctor_scan_undeclared_clean_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write_meta_yaml(dir.path(), "root", &[("alpha", "https://example/alpha.git")]);
    fake_repo(&dir.path().join("alpha"));

    bin()
        .current_dir(dir.path())
        .args(["doctor", "--scan-undeclared"])
        .assert()
        // Doctor's standard checks pass cleanly → exit 0.
        .success()
        .stdout(predicate::str::contains("No undeclared git repos found"));
}

/// AC #2 — A workspace with one undeclared `.git/` reports the relative
/// path. The fixture has no remote configured → `[unknown]` placeholder.
///
/// The doctor's standard `on-disk-drift` check WILL warn about
/// `vendor/` (top-level unregistered dir) and exit 1. We intentionally
/// do not assert on the exit code here: `--scan-undeclared` is
/// report-only by spec and never alters the exit semantics, so the
/// drift warning is orthogonal to what this test is checking.
#[test]
fn doctor_scan_undeclared_finds_untracked_repo() {
    let dir = tempfile::tempdir().unwrap();
    write_meta_yaml(dir.path(), "root", &[("alpha", "https://example/alpha.git")]);
    fake_repo(&dir.path().join("alpha"));
    fake_repo(&dir.path().join("vendor").join("legacy"));

    let out = bin()
        .current_dir(dir.path())
        .args(["doctor", "--scan-undeclared"])
        .output()
        .expect("spawn grex doctor");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("Found 1 undeclared git repo:"),
        "expected single-repo header in stdout: {stdout}",
    );
    assert!(stdout.contains("vendor/legacy"), "expected the untracked path in stdout: {stdout}",);
    assert!(
        stdout.contains("[unknown]"),
        "no remote.origin.url in fixture → expected [unknown] marker: {stdout}",
    );
    assert!(stdout.contains("To register: grex add"), "expected fix-suggestion footer: {stdout}",);
}

/// AC #3 — Nested tree with a mix of registered + untracked. `--depth 1`
/// limits the scan so a depth-2 untracked repo is not surfaced; without
/// the flag it is reported. Same exit-code disclaimer as AC #2.
#[test]
fn doctor_scan_undeclared_depth_bounds_scan() {
    let dir = tempfile::tempdir().unwrap();
    write_meta_yaml(dir.path(), "root", &[("alpha", "https://example/alpha.git")]);
    fake_repo(&dir.path().join("alpha"));
    // Untracked repo at depth=2 (`vendor/legacy/.git/`).
    fake_repo(&dir.path().join("vendor").join("legacy"));

    // depth=1 → scan never descends into vendor/, so legacy stays hidden.
    let out = bin()
        .current_dir(dir.path())
        .args(["doctor", "--scan-undeclared", "--depth", "1"])
        .output()
        .expect("spawn grex doctor depth=1");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("No undeclared git repos found"),
        "depth=1 must hide vendor/legacy: {stdout}",
    );

    // Unbounded scan finds it.
    let out = bin()
        .current_dir(dir.path())
        .args(["doctor", "--scan-undeclared"])
        .output()
        .expect("spawn grex doctor unbounded");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(stdout.contains("vendor/legacy"), "unbounded scan must find vendor/legacy: {stdout}",);
}

/// AC #4 — `--depth` requires `--scan-undeclared`. Passing `--depth`
/// alone must be a clap argument-validation error so users get a clear
/// message rather than silently ignoring the bound.
#[test]
fn doctor_depth_without_scan_flag_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    bin().current_dir(dir.path()).args(["doctor", "--depth", "1"]).assert().failure();
}

/// AC #5 — Without `--scan-undeclared`, the doctor verb runs identically
/// to v1.2.0: no scan block, no "Scanning ..." message in stdout.
/// (The fixture's stray `vendor/` will trigger an `on-disk-drift`
/// warning regardless, so we read output without asserting on the
/// exit code.)
#[test]
fn doctor_default_run_does_not_scan() {
    let dir = tempfile::tempdir().unwrap();
    write_meta_yaml(dir.path(), "root", &[("alpha", "https://example/alpha.git")]);
    fake_repo(&dir.path().join("alpha"));
    fake_repo(&dir.path().join("vendor").join("legacy"));

    let out = bin().current_dir(dir.path()).arg("doctor").output().expect("spawn doctor");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(!stdout.contains("Scanning"), "default doctor run must not invoke the scan: {stdout}",);
    assert!(
        !stdout.contains("vendor/legacy"),
        "default doctor run must not list untracked repos in stdout: {stdout}",
    );
}
