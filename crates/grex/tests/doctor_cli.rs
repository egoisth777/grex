// End-to-end CLI tests for `grex doctor`.
//
// Spawns the real `grex` binary via `assert_cmd`, prepares a temp
// workspace, and asserts exit codes + stdout contents for each
// scenario required by the M7-4b spec.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary")
}

/// Recursive path+bytes snapshot of a directory, keyed by path
/// relative to `root`. Used to prove `--fix` writes nothing when the
/// doctor refuses to heal (schema error, drift error). Skips `.git/`
/// and `target/` — never relevant here and keeps output deterministic.
fn fs_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            if name == ".git" || name == "target" {
                continue;
            }
            let ft = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if ft.is_dir() {
                walk(&path, root, out);
            } else if ft.is_file() {
                let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                let bytes = fs::read(&path).unwrap_or_default();
                out.insert(rel, bytes);
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Minimal valid grex.jsonl with one declarative pack `pack_id` rooted
/// at `<workspace>/<pack_id>`.
fn seed_manifest(workspace: &Path, pack_id: &str) {
    let manifest = workspace.join("grex.jsonl");
    let line = format!(
        r#"{{"op":"add","ts":"2026-04-22T10:00:00Z","id":"{id}","url":"https://example/{id}","path":"{id}","type":"declarative","schema_version":"1"}}
"#,
        id = pack_id
    );
    fs::write(&manifest, line).unwrap();
    fs::create_dir_all(workspace.join(pack_id)).unwrap();
}

/// Write a valid empty managed block for `pack_id`.
fn seed_clean_gitignore(workspace: &Path, pack_id: &str) {
    let gi = workspace.join(pack_id).join(".gitignore");
    let body = format!("# >>> grex:{id} >>>\n# <<< grex:{id} <<<\n", id = pack_id);
    fs::write(gi, body).unwrap();
}

/// Write a drifted managed block (unexpected pattern line).
fn seed_drifted_gitignore(workspace: &Path, pack_id: &str) {
    let gi = workspace.join(pack_id).join(".gitignore");
    let body = format!("# >>> grex:{id} >>>\ndrifted-pattern\n# <<< grex:{id} <<<\n", id = pack_id);
    fs::write(gi, body).unwrap();
}

#[test]
fn doctor_clean_workspace_exits_zero() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    seed_clean_gitignore(dir.path(), "a");

    bin()
        .current_dir(dir.path())
        .arg("doctor")
        .assert()
        .success()
        .stdout(predicate::str::contains("manifest-schema"));
}

#[test]
fn doctor_gitignore_drift_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    seed_drifted_gitignore(dir.path(), "a");

    bin().current_dir(dir.path()).arg("doctor").assert().code(1);
}

#[test]
fn doctor_missing_pack_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    fs::remove_dir_all(dir.path().join("a")).unwrap();

    bin().current_dir(dir.path()).arg("doctor").assert().code(2);
}

#[test]
fn doctor_fix_heals_gitignore_drift() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    seed_drifted_gitignore(dir.path(), "a");

    // --fix → exit 0 (drift healed post-fix).
    bin().current_dir(dir.path()).args(["doctor", "--fix"]).assert().success();

    // Re-run without --fix → still exit 0 (idempotent).
    bin().current_dir(dir.path()).arg("doctor").assert().success();
}

#[test]
fn doctor_fix_does_not_touch_missing_pack_dir() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    fs::remove_dir_all(dir.path().join("a")).unwrap();

    // SAFETY CRITICAL: --fix must NOT write anywhere in the workspace
    // on drift error — a recursive path+bytes snapshot proves no stray
    // write landed in `grex.jsonl`, a `.gitignore`, or the missing
    // pack dir.
    let before = fs_snapshot(dir.path());

    bin().current_dir(dir.path()).args(["doctor", "--fix"]).assert().code(2);

    let after = fs_snapshot(dir.path());
    assert_eq!(before, after, "--fix must not write anywhere on drift error");
    assert!(!dir.path().join("a").exists(), "missing pack dir must stay missing");
}

#[test]
fn doctor_fix_does_not_touch_manifest_on_corruption() {
    let dir = tempfile::tempdir().unwrap();
    // Corrupt line 1 (not last — line 2 is valid).
    let manifest = dir.path().join("grex.jsonl");
    fs::write(
        &manifest,
        "garbage-line\n{\"op\":\"add\",\"ts\":\"2026-04-22T10:00:00Z\",\"id\":\"x\",\"url\":\"u\",\"path\":\"x\",\"type\":\"declarative\",\"schema_version\":\"1\"}\n",
    )
    .unwrap();
    fs::create_dir_all(dir.path().join("x")).unwrap();
    let before_manifest = fs::read(&manifest).unwrap();
    let before = fs_snapshot(dir.path());

    bin().current_dir(dir.path()).args(["doctor", "--fix"]).assert().code(2);

    let after_manifest = fs::read(&manifest).unwrap();
    assert_eq!(
        before_manifest, after_manifest,
        "manifest bytes must be unchanged by --fix on schema error"
    );
    // SAFETY: extend the byte-equal contract to the whole workspace —
    // no stray write in `.gitignore`, pack dirs, or anywhere else.
    let after = fs_snapshot(dir.path());
    assert_eq!(before, after, "--fix must not write anywhere on schema error");
}

#[test]
fn doctor_lint_config_skipped_by_default() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    seed_clean_gitignore(dir.path(), "a");
    // Seed a broken config.yaml that must be ignored by default.
    fs::create_dir_all(dir.path().join("openspec")).unwrap();
    fs::write(dir.path().join("openspec").join("config.yaml"), ": : : [bad").unwrap();

    let before = fs_snapshot(dir.path());
    let out = bin().current_dir(dir.path()).arg("doctor").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    assert!(!stdout.contains("config-lint"), "default run must not mention config-lint: {stdout}");
    // SAFETY: default (read-only) run must not touch the fixture.
    let after = fs_snapshot(dir.path());
    assert_eq!(before, after, "default doctor run must be read-only");
}

#[test]
fn doctor_lint_config_flag_runs_config_check() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    seed_clean_gitignore(dir.path(), "a");
    fs::create_dir_all(dir.path().join("openspec")).unwrap();
    fs::write(dir.path().join("openspec").join("config.yaml"), ": : : [bad").unwrap();

    bin()
        .current_dir(dir.path())
        .args(["doctor", "--lint-config"])
        .assert()
        .code(1)
        .stdout(predicate::str::contains("config-lint"));
}

#[test]
fn doctor_json_emits_report_shape() {
    let dir = tempfile::tempdir().unwrap();
    seed_manifest(dir.path(), "a");
    seed_clean_gitignore(dir.path(), "a");

    let out = bin().current_dir(dir.path()).args(["doctor", "--json"]).assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid JSON");
    assert!(parsed.get("findings").is_some(), "json must have findings array");
}
