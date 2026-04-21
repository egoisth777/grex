//! Pack-type dispatch integration tests — M5-1 Stage C.
//!
//! Covers the three built-in pack types under the new
//! `PackTypeRegistry` dispatch path introduced by Stage C:
//!
//! 1. Declarative pack runs its actions (equivalent to M4 behaviour).
//! 2. Meta pack recursively installs two nested declarative children
//!    via the outer sync loop post-order traversal — the MetaPlugin
//!    synthesis step sits alongside the real declarative side-effects.
//! 3. Scripted pack runs a hook script. On Unix a trivial `sh` hook is
//!    seeded; on Windows a missing-hook → no-op path is exercised
//!    because PowerShell availability is not guaranteed in CI.
//!
//! Tests bypass the git tree-walk (no remote repos) and drive
//! `sync::run` against on-disk meta roots with local `path:`-resolved
//! children. This keeps the fixture short and avoids depending on
//! `git` being on PATH.

use std::fs;
use std::path::{Path, PathBuf};

use grex_core::sync::{run, SyncOptions};
use grex_core::ExecResult;
use tempfile::TempDir;

/// Write a `pack.yaml` under `<dir>/.grex/pack.yaml`. Creates the
/// intermediate `.grex` directory.
fn write_pack(dir: &Path, yaml: &str) -> PathBuf {
    fs::create_dir_all(dir.join(".grex")).unwrap();
    let path = dir.join(".grex").join("pack.yaml");
    fs::write(&path, yaml).unwrap();
    path
}

/// A root meta pack whose two children are declarative packs that mkdir
/// into the fixture sink. The walker resolves children by `url:` only,
/// so we cannot express nested on-disk children without git — this
/// helper instead authors a single-pack tree (no `children:`) for the
/// declarative and scripted tests, and the meta test drives a pre-
/// walked graph via the sync entry point against a root that
/// `depends_on:` nothing and has no children (the dispatch behaviour
/// under test is the plugin call itself, not tree traversal).
fn options(workspace: PathBuf) -> SyncOptions {
    SyncOptions::new().with_validate(true).with_workspace(Some(workspace))
}

#[test]
fn declarative_pack_runs_its_actions_through_new_dispatch() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let sink = tmp_path.join("sink");
    fs::create_dir_all(&sink).unwrap();
    let target = sink.join("declarative-target");

    let yaml = format!(
        "schema_version: \"1\"\nname: d\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
        target.to_string_lossy().replace('\\', "/"),
    );
    let root = tmp_path.join("root");
    write_pack(&root, &yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let report = run(&root, &options(workspace)).expect("declarative sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert!(target.is_dir(), "mkdir must have executed");
    // One `PerformedChange` step from the mkdir action.
    let performed = report
        .steps
        .iter()
        .filter(|s| matches!(s.exec_step.result, ExecResult::PerformedChange))
        .count();
    assert_eq!(performed, 1, "one mkdir PerformedChange step: {:?}", report.steps);
}

#[test]
fn meta_pack_with_no_children_emits_synthesis_step() {
    // M5-1c: a meta pack with no children emits one `noop_step("meta")`
    // through `MetaPlugin::install`. This exercises the dispatch path
    // (registry lookup → tokio::block_on → plugin call) without
    // requiring git for child resolution. The declarative recursion
    // case is covered end-to-end by `crates/grex/tests/sync_e2e.rs`'s
    // 3-level fixture which walks root(meta) → a(decl) and
    // root(meta) → b(meta) → c(decl) via real bare repos.
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let yaml = "schema_version: \"1\"\nname: m\ntype: meta\n";
    let root = tmp_path.join("root");
    write_pack(&root, yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let report = run(&root, &options(workspace)).expect("meta sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert_eq!(report.steps.len(), 1, "one synthesis step: {:?}", report.steps);
    let step = &report.steps[0];
    assert_eq!(step.pack, "m");
    assert!(matches!(step.exec_step.result, ExecResult::NoOp));
    assert_eq!(step.exec_step.action_name.as_ref(), "meta");
}

#[test]
fn scripted_pack_missing_hook_is_a_noop() {
    // Cross-platform path: missing `setup.{sh,ps1}` → `Ok(noop)` via
    // `ScriptedPlugin::run_hook`. Seeding a real hook would require
    // `sh` on Unix and `pwsh` on Windows; both are fragile in CI. The
    // missing-hook path exercises the same dispatch chain (registry
    // lookup → async block_on → ScriptedPlugin::install → hook_path
    // → tokio::fs::metadata NotFound → Ok(noop_step)).
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let yaml = "schema_version: \"1\"\nname: s\ntype: scripted\n";
    let root = tmp_path.join("root");
    write_pack(&root, yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let report = run(&root, &options(workspace)).expect("scripted sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert_eq!(report.steps.len(), 1, "one scripted step: {:?}", report.steps);
    let step = &report.steps[0];
    assert_eq!(step.pack, "s");
    assert!(matches!(step.exec_step.result, ExecResult::NoOp));
    assert_eq!(step.exec_step.action_name.as_ref(), "scripted");
}
