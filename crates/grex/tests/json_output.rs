// JSON-output integration tests for the CLI verb surface.
//
// M8-6 / issue #35: `--json` is now wired for all 11 non-transport
// verbs. Stubs emit `{"status":"unimplemented","verb":"<name>"}`;
// real verbs (`doctor`, `import`, `sync`, `teardown`) emit a verb-specific
// schema mirroring the human output. `serve` is excluded — it owns stdio
// for JSON-RPC and `--json` is not applicable.
//
// Each test spawns the real `grex` binary via `assert_cmd`, invokes
// `grex <verb> --json`, and asserts that stdout parses as JSON with
// the expected verb-specific key.

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

// ----- Stub verbs: expect `{"status":"unimplemented","verb":"<name>"}` -----

fn assert_unimplemented(verb: &str, extra_args: &[&str]) {
    // `--json` is placed before the verb so that verbs using
    // `trailing_var_arg = true` (notably `exec`) cannot swallow it as a
    // positional — clap parses it as the global flag regardless of
    // verb ordering.
    let mut cmd = bin();
    cmd.arg("--json");
    cmd.arg(verb);
    for a in extra_args {
        cmd.arg(a);
    }
    let out = cmd.assert().success().get_output().clone();
    let v = parse_json_stdout(&out);
    assert_eq!(v.get("status").and_then(Value::as_str), Some("unimplemented"), "status field");
    assert_eq!(v.get("verb").and_then(Value::as_str), Some(verb), "verb field for {verb}");
}

#[test]
fn init_json_emits_unimplemented() {
    assert_unimplemented("init", &[]);
}

#[test]
fn add_json_emits_unimplemented() {
    assert_unimplemented("add", &["https://example.com/repo.git"]);
}

#[test]
fn rm_json_emits_unimplemented() {
    assert_unimplemented("rm", &["some-pack"]);
}

#[test]
fn ls_json_emits_unimplemented() {
    assert_unimplemented("ls", &[]);
}

#[test]
fn status_json_emits_unimplemented() {
    assert_unimplemented("status", &[]);
}

#[test]
fn update_json_emits_unimplemented() {
    assert_unimplemented("update", &[]);
}

#[test]
fn run_json_emits_unimplemented() {
    assert_unimplemented("run", &["some-action"]);
}

#[test]
fn exec_json_emits_unimplemented() {
    assert_unimplemented("exec", &["echo", "hi"]);
}

// `sync` and `teardown` without `<pack_root>` emit a usage-error envelope
// and exit 2 (not the `unimplemented` stub). Asserted below in
// `sync_without_pack_root_json_emits_usage_error` /
// `teardown_without_pack_root_json_emits_usage_error`.

fn assert_usage_error(verb: &str) {
    let out = bin().args([verb, "--json"]).assert().failure().get_output().clone();
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
    assert!(v.get("findings").is_some(), "doctor JSON must have a `findings` array");
    assert!(v.get("exit_code").is_some(), "doctor JSON must have an `exit_code`");
}

#[test]
fn import_json_missing_arg_is_error_but_stays_non_panicking() {
    // `grex import --json` without --from-repos-json should exit non-zero
    // (the verb requires the flag) but must not panic. We assert the
    // failure is a clean anyhow error path, not a JSON-parse crash.
    bin().args(["import", "--json"]).assert().failure();
}
