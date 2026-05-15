//! CLI coverage for `grex update` (v1.4.0).
//!
//! `update` is a thin alias for `sync`. The smoke pin here ensures the
//! alias dispatches into the sync handler rather than into a stub or
//! divergent code path.

mod common;

use common::grex;
use predicates::str::contains;

#[test]
fn update_outside_pack_root_fails_with_sync_usage() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .arg("update")
        .assert()
        .failure()
        .code(2)
        .stderr(contains("grex sync:"));
}

#[test]
fn update_in_seeded_pack_root_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let grex_dir = dir.path().join(".grex");
    std::fs::create_dir_all(&grex_dir).unwrap();
    std::fs::write(
        grex_dir.join("pack.yaml"),
        "schema_version: \"1\"\nname: update-test\ntype: meta\nactions: []\nchildren: []\n",
    )
    .unwrap();
    grex()
        .current_dir(dir.path())
        .arg("update")
        .assert()
        .success();
}
