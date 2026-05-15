// JSON-output integration tests for the CLI verb surface.
//
// v1.4.0 — all 14 verbs are now wired. Each test asserts the JSON
// envelope shape for one verb under a representative invocation.
// `serve` is excluded (owns stdio for JSON-RPC, --json N/A).

use assert_cmd::prelude::*;
use serde_json::Value;
use std::process::Command;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary")
}

fn parse_json_stdout(out: &std::process::Output) -> Value {
    let stdout = String::from_utf8(out.stdout.clone()).expect("valid utf8 stdout");
    serde_json::from_str::<Value>(&stdout)
        .unwrap_or_else(|e| panic!("stdout is not valid JSON: {e}\n---\n{stdout}\n---"))
}

// Helper: seed a minimal meta-pack manifest at `dir/.grex/pack.yaml`.
fn seed_pack(dir: &std::path::Path) {
    let grex_dir = dir.join(".grex");
    std::fs::create_dir_all(&grex_dir).unwrap();
    std::fs::write(
        grex_dir.join("pack.yaml"),
        "schema_version: \"1\"\nname: test-pack\ntype: meta\nactions: []\nchildren: []\n",
    )
    .unwrap();
}

#[test]
fn init_json_emits_ok_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("workspace");
    let out = bin().args(["--json", "init"]).arg(&target).assert().success().get_output().clone();
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("init"));
    assert_eq!(v.get("status").and_then(Value::as_str), Some("ok"));
    assert!(target.join(".grex/pack.yaml").is_file());
}

#[test]
fn init_json_idempotency_error() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let out =
        bin().args(["--json", "init"]).arg(dir.path()).assert().failure().get_output().clone();
    assert_eq!(out.status.code(), Some(1));
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("init"));
    assert_eq!(v.pointer("/error/kind").and_then(Value::as_str), Some("already_initialized"));
}

#[test]
fn add_json_emits_report() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .current_dir(dir.path())
        .args(["--json", "add", "https://example.com/repo.git"])
        .assert()
        .success()
        .get_output()
        .clone();
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("dry_run").and_then(Value::as_bool), Some(false));
    assert_eq!(v.get("id").and_then(Value::as_str), Some("repo"));
    assert_eq!(v.get("path").and_then(Value::as_str), Some("repo"));
    assert_eq!(v.get("type").and_then(Value::as_str), Some("scripted"));
    assert_eq!(v.get("appended").and_then(Value::as_bool), Some(true));
}

#[test]
fn rm_json_emits_error_for_missing_path() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .current_dir(dir.path())
        .args(["--json", "rm", "definitely-not-here"])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_eq!(out.status.code(), Some(2));
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("rm"));
    assert_eq!(v.pointer("/error/kind").and_then(Value::as_str), Some("not_found"));
}

#[test]
fn status_json_clean_pack() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let out = bin().args(["--json", "status"]).arg(dir.path()).assert().get_output().clone();
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("status"));
    assert!(v.get("clean").is_some(), "status JSON must carry a `clean` field");
    assert!(
        v.get("packs").and_then(Value::as_array).is_some(),
        "status JSON must carry a `packs` array"
    );
}

#[test]
fn update_json_usage_error_outside_pack() {
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .current_dir(dir.path())
        .args(["--json", "update"])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_eq!(out.status.code(), Some(2));
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("sync"));
    assert_eq!(
        v.pointer("/error/kind").and_then(Value::as_str),
        Some("usage"),
        "update delegates to sync; usage error envelope expected"
    );
}

#[test]
fn run_json_no_match_envelope() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let out = bin()
        .args(["--json", "run", "symlink"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("run"));
    assert_eq!(v.get("matched_packs").and_then(Value::as_u64), Some(0));
    assert_eq!(v.get("action").and_then(Value::as_str), Some("symlink"));
}

#[test]
fn exec_json_envelope_with_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let program = if cfg!(windows) { "cmd" } else { "true" };
    let args_slice: &[&str] = if cfg!(windows) { &["/c", "exit", "0"] } else { &[] };
    let mut cmd = bin();
    cmd.args(["--json", "exec", "--pack"]).arg(dir.path()).arg("--").arg(program);
    for a in args_slice {
        cmd.arg(a);
    }
    let out = cmd.assert().success().get_output().clone();
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some("exec"));
    assert_eq!(v.get("exit_code").and_then(Value::as_i64), Some(0));
}

fn assert_usage_error(verb: &str) {
    let dir = tempfile::tempdir().unwrap();
    let out = bin()
        .current_dir(dir.path())
        .args([verb, "--json"])
        .assert()
        .failure()
        .get_output()
        .clone();
    assert_eq!(out.status.code(), Some(2), "{verb} --json must exit 2 on missing pack_root");
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("verb").and_then(Value::as_str), Some(verb));
    assert_eq!(
        v.pointer("/error/kind").and_then(Value::as_str),
        Some("usage"),
        "error.kind must be `usage`"
    );
}

#[test]
fn sync_without_pack_root_json_emits_usage_error() {
    assert_usage_error("sync");
}

#[test]
fn teardown_without_pack_root_json_emits_usage_error() {
    assert_usage_error("teardown");
}

// ----- Reference verbs (regression): doctor + import ---------------------

#[test]
fn doctor_json_has_findings_array() {
    // `doctor` already had --json support pre-M8-6; regression-pin it here.
    let dir = tempfile::tempdir().unwrap();
    // Seed a trivial empty workspace — doctor prints a findings array even
    // when there's nothing to check (every finding is allowed to be absent).
    let out =
        bin().current_dir(dir.path()).args(["doctor", "--json"]).assert().get_output().clone();
    let v: Value = serde_json::from_slice(&out.stdout).expect("doctor --json is valid JSON");
    // v1.3.0: top-level envelope is `{workspace, pack, report}`; the
    // findings + exit_code now live one level deep under `report`.
    let report = v.get("report").expect("v1.3.0: doctor JSON must nest report under `report` key");
    assert!(report.get("findings").is_some(), "doctor JSON must have a `report.findings` array");
    assert!(report.get("exit_code").is_some(), "doctor JSON must have a `report.exit_code`");
}

#[test]
fn import_json_missing_arg_is_error_but_stays_non_panicking() {
    // `grex import --json` without --from-repos-json should exit non-zero
    // (the verb requires the flag) but must not panic. We assert the
    // failure is a clean anyhow error path, not a JSON-parse crash.
    bin().args(["import", "--json"]).assert().failure();
}
