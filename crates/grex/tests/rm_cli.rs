//! CLI coverage for `grex rm` (v1.4.0).

mod common;

use common::grex;
use predicates::str::contains;
use std::fs;

fn seed_meta_pack(dir: &std::path::Path, body: &str) {
    let grex_dir = dir.join(".grex");
    fs::create_dir_all(&grex_dir).unwrap();
    fs::write(grex_dir.join("pack.yaml"), body).unwrap();
}

#[test]
fn rm_missing_path_exits_two() {
    let dir = tempfile::tempdir().unwrap();
    grex().current_dir(dir.path()).args(["rm", "definitely-not-a-pack"]).assert().failure().code(2);
}

#[test]
fn rm_empty_meta_pack_removes_directory() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("p");
    seed_meta_pack(
        &pack,
        "schema_version: \"1\"\nname: leaf\ntype: meta\nactions: []\nchildren: []\n",
    );
    grex().arg("rm").arg(&pack).assert().success();
    assert!(!pack.exists(), "directory must be removed");
}

#[test]
fn rm_meta_with_children_refuses_without_force() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("p");
    seed_meta_pack(
        &pack,
        "schema_version: \"1\"\nname: parent\ntype: meta\nactions: []\nchildren:\n  - url: https://example.invalid/child.git\n    path: c\n",
    );
    grex().arg("rm").arg(&pack).assert().failure().code(1).stderr(contains("children"));
    assert!(pack.exists(), "refused rm must not delete the directory");
}

#[test]
fn rm_meta_with_children_succeeds_with_force() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("p");
    seed_meta_pack(
        &pack,
        "schema_version: \"1\"\nname: parent\ntype: meta\nactions: []\nchildren:\n  - url: https://example.invalid/child.git\n    path: c\n",
    );
    grex().arg("rm").arg(&pack).arg("--force").assert().success();
    assert!(!pack.exists(), "directory must be removed under --force");
}

#[test]
fn rm_dry_run_does_not_remove_directory() {
    let dir = tempfile::tempdir().unwrap();
    let pack = dir.path().join("p");
    seed_meta_pack(
        &pack,
        "schema_version: \"1\"\nname: leaf\ntype: meta\nactions: []\nchildren: []\n",
    );
    grex().args(["--dry-run", "rm"]).arg(&pack).assert().success();
    assert!(pack.exists(), "dry-run must not remove the directory");
}
