//! CLI coverage for `grex run` (v1.4.0).

mod common;

use common::grex;

fn seed_pack(dir: &std::path::Path, body: &str) {
    let grex_dir = dir.join(".grex");
    std::fs::create_dir_all(&grex_dir).unwrap();
    std::fs::write(grex_dir.join("pack.yaml"), body).unwrap();
}

#[test]
fn run_outside_pack_root_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["run", "symlink"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn run_no_match_exits_zero_with_informational_message() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(
        dir.path(),
        "schema_version: \"1\"\nname: run-test\ntype: scripted\nactions: []\nchildren: []\n",
    );
    let out = grex()
        .args(["run", "symlink"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert!(
        stdout.contains("no packs declare action"),
        "no-match message expected; got: {stdout}"
    );
}

#[test]
fn run_json_no_match_envelope_has_matched_packs_zero() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(
        dir.path(),
        "schema_version: \"1\"\nname: run-test\ntype: scripted\nactions: []\nchildren: []\n",
    );
    let out = grex()
        .args(["--json", "run", "mkdir"])
        .arg(dir.path())
        .assert()
        .success()
        .get_output()
        .clone();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v["verb"].as_str(), Some("run"));
    assert_eq!(v["matched_packs"].as_u64(), Some(0));
    assert_eq!(v["action"].as_str(), Some("mkdir"));
}
