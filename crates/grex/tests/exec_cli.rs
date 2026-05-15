//! CLI coverage for `grex exec` (v1.4.0).

mod common;

use common::grex;
use std::fs;

fn seed_pack(dir: &std::path::Path) {
    let grex_dir = dir.join(".grex");
    fs::create_dir_all(&grex_dir).unwrap();
    fs::write(
        grex_dir.join("pack.yaml"),
        "schema_version: \"1\"\nname: exec-test\ntype: meta\nactions: []\nchildren: []\n",
    )
    .unwrap();
}

#[test]
fn exec_without_args_fails_at_parse_time() {
    let dir = tempfile::tempdir().unwrap();
    grex().current_dir(dir.path()).arg("exec").assert().failure();
}

#[test]
fn exec_outside_pack_root_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    let program = if cfg!(windows) { "cmd" } else { "true" };
    grex().current_dir(dir.path()).args(["exec", "--", program]).assert().failure().code(2);
}

#[test]
fn exec_in_pack_root_propagates_child_exit_zero() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let mut cmd = grex();
    cmd.current_dir(dir.path()).args(["exec", "--"]);
    if cfg!(windows) {
        cmd.args(["cmd", "/c", "exit", "0"]);
    } else {
        cmd.arg("true");
    }
    cmd.assert().success();
}

#[test]
fn exec_json_envelope_captures_stdout_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let mut cmd = grex();
    cmd.args(["--json", "exec", "--pack"]).arg(dir.path()).arg("--");
    if cfg!(windows) {
        cmd.args(["cmd", "/c", "echo", "hello"]);
    } else {
        cmd.args(["echo", "hello"]);
    }
    let out = cmd.assert().success().get_output().clone();
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("exec --json must emit valid JSON");
    assert_eq!(v["verb"].as_str(), Some("exec"));
    assert_eq!(v["exit_code"].as_i64(), Some(0));
    assert!(v.get("stdout").is_some());
}
