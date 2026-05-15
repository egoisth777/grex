//! CLI coverage for `grex status` (v1.4.0).

mod common;

use common::grex;
use predicates::prelude::PredicateBooleanExt;
use predicates::str::contains;

fn seed_pack(dir: &std::path::Path) {
    let grex_dir = dir.join(".grex");
    std::fs::create_dir_all(&grex_dir).unwrap();
    std::fs::write(
        grex_dir.join("pack.yaml"),
        "schema_version: \"1\"\nname: status-test\ntype: meta\nactions: []\nchildren: []\n",
    )
    .unwrap();
}

#[test]
fn status_outside_pack_root_exits_two_with_usage() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .arg("status")
        .assert()
        .failure()
        .code(2)
        .stderr(contains("pack_root"));
}

#[test]
fn status_clean_pack_emits_a_state_line() {
    // The meta-pack-type plugin appends a synthetic step for the
    // children-sync phase even when `children:` is empty, so a fresh
    // meta pack reports `would-update 1 <name>` rather than `clean`.
    // We assert that status prints a single state line carrying the
    // pack name + a recognised state token.
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    grex()
        .args(["status"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(contains("status-test").and(contains("clean").or(contains("would-update"))));
}

#[test]
fn status_json_envelope_has_packs_array_and_clean_field() {
    let dir = tempfile::tempdir().unwrap();
    seed_pack(dir.path());
    let out =
        grex().args(["--json", "status"]).arg(dir.path()).assert().success().get_output().clone();
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).expect("valid JSON");
    assert_eq!(v["verb"].as_str(), Some("status"));
    assert!(v["clean"].is_boolean(), "clean field must be boolean");
    assert!(v["packs"].is_array(), "packs field must be an array");
}
