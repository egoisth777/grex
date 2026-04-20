//! End-to-end tests for `grex sync` — M3 Stage B slice 6.
//!
//! Build synthetic 3-level pack trees on disk (root → a, b → c) using
//! locally-created bare repos (same pattern as `grex-core/tests/tree_walk.rs`
//! and `grex-core/tests/git_backend.rs`), then invoke `grex_core::sync::run`
//! directly. Driving the orchestrator rather than spawning the binary keeps
//! assertions on the typed [`SyncReport`] while still covering all surfaces
//! touched by the CLI verb.

#![allow(clippy::too_many_lines)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use grex_core::git::gix_backend::file_url_from_path;
use grex_core::manifest;
use grex_core::sync::{run, SyncError, SyncOptions};
use grex_core::{ExecResult, StepKind};
use tempfile::TempDir;

fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
    });
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Seed a working tree with `.grex/pack.yaml` + optional files, commit, then
/// clone it as `<name>.git` bare inside `tmp`. Returns the bare-repo path.
fn bare_with_manifest(tmp: &Path, name: &str, yaml: &str, extra_files: &[(&str, &str)]) -> PathBuf {
    init_git_identity();
    let work = tmp.join(format!("seed-{name}-work"));
    fs::create_dir_all(work.join(".grex")).unwrap();
    for (rel, contents) in extra_files {
        let p = work.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, contents).unwrap();
    }
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    fs::write(work.join(".grex/pack.yaml"), yaml).unwrap();
    // Stage every tracked file.
    run_git(&work, &["add", "-A"]);
    run_git(&work, &["commit", "-q", "-m", "seed"]);

    let bare = tmp.join(format!("{name}.git"));
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

fn write_root(dir: &Path, yaml: &str) {
    fs::create_dir_all(dir.join(".grex")).unwrap();
    fs::write(dir.join(".grex/pack.yaml"), yaml).unwrap();
}

struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    workspace: PathBuf,
    // Target paths the `a` pack's mkdir / symlink actions resolve against.
    a_target_dir: PathBuf,
    a_symlink_src: PathBuf,
    a_symlink_dst: PathBuf,
    c_target_dir: PathBuf,
}

/// Build the 3-level synthetic tree:
///
/// ```text
/// root (meta) → a (declarative: mkdir + symlink)
///             → b (meta)      → c (declarative: mkdir)
/// ```
fn build_fixture() -> Fixture {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();

    let sink = tmp_path.join("sink");
    fs::create_dir_all(&sink).unwrap();
    let a_target_dir = sink.join("a-made");
    let a_symlink_src = sink.join("src-for-a");
    fs::write(&a_symlink_src, b"src").unwrap();
    let a_symlink_dst = sink.join("dst-for-a");
    let c_target_dir = sink.join("c-made");

    // Build child bare repos.
    let c_yaml = format!(
        "schema_version: \"1\"\nname: c\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
        c_target_dir.to_string_lossy().replace('\\', "/"),
    );
    let c_bare = bare_with_manifest(&tmp_path, "c", &c_yaml, &[]);
    let c_url = file_url_from_path(&c_bare);

    let b_yaml = format!(
        "schema_version: \"1\"\nname: b\ntype: meta\nchildren:\n  - url: {c_url}\n    path: c\n",
    );
    let b_bare = bare_with_manifest(&tmp_path, "b", &b_yaml, &[]);
    let b_url = file_url_from_path(&b_bare);

    let a_yaml = format!(
        concat!(
            "schema_version: \"1\"\nname: a\ntype: declarative\n",
            "actions:\n",
            "  - mkdir:\n      path: {mkdir_path}\n",
            "  - symlink:\n      src: {sym_src}\n      dst: {sym_dst}\n      kind: file\n",
        ),
        mkdir_path = a_target_dir.to_string_lossy().replace('\\', "/"),
        sym_src = a_symlink_src.to_string_lossy().replace('\\', "/"),
        sym_dst = a_symlink_dst.to_string_lossy().replace('\\', "/"),
    );
    let a_bare = bare_with_manifest(&tmp_path, "a", &a_yaml, &[("files/keep.txt", "keep")]);
    let a_url = file_url_from_path(&a_bare);

    // Root pack is authored on-disk (no remote), as in `walker_integrates_with_real_git_backend`.
    let root_dir = tmp_path.join("root");
    let root_yaml = format!(
        concat!(
            "schema_version: \"1\"\nname: root\ntype: meta\n",
            "children:\n",
            "  - url: {a_url}\n    path: a\n",
            "  - url: {b_url}\n    path: b\n",
        ),
        a_url = a_url,
        b_url = b_url,
    );
    write_root(&root_dir, &root_yaml);

    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    Fixture {
        _tmp: tmp,
        root: root_dir,
        workspace,
        a_target_dir,
        a_symlink_src,
        a_symlink_dst,
        c_target_dir,
    }
}

fn options(dry_run: bool, workspace: PathBuf) -> SyncOptions {
    SyncOptions { dry_run, validate: true, workspace: Some(workspace) }
}

#[test]
fn e2e_dry_run_3_level_tree() {
    let f = build_fixture();
    let report = run(&f.root, &options(true, f.workspace.clone())).expect("dry run succeeds");
    assert_eq!(report.graph.nodes().len(), 4, "expect root + a + b + c");
    let child_edges = report
        .graph
        .edges()
        .iter()
        .filter(|e| matches!(e.kind, grex_core::EdgeKind::Child))
        .count();
    assert_eq!(child_edges, 3, "3 Child edges: root→a, root→b, b→c");
    assert!(report.halted.is_none());

    // One step for each of: a.mkdir, a.symlink, c.mkdir. b and root have no
    // actions. All three should be WouldPerformChange.
    assert_eq!(report.steps.len(), 3);
    for s in &report.steps {
        assert_eq!(s.exec_step.result, ExecResult::WouldPerformChange, "{s:?}");
    }

    // Disk must be untouched.
    assert!(!f.a_target_dir.exists(), "dry-run mkdir must not create dir");
    assert!(!f.a_symlink_dst.exists(), "dry-run symlink must not create link");
    assert!(!f.c_target_dir.exists(), "dry-run mkdir must not create dir");
}

#[test]
fn e2e_wet_run_3_level_tree() {
    let f = build_fixture();
    let report = run(&f.root, &options(false, f.workspace.clone())).expect("wet run succeeds");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert_eq!(report.steps.len(), 3);

    // Disk assertions — post-order means c.mkdir + a.mkdir + a.symlink all ran.
    assert!(f.a_target_dir.is_dir(), "a mkdir should have produced dir");
    assert!(f.c_target_dir.is_dir(), "c mkdir should have produced dir");
    assert!(
        fs::symlink_metadata(&f.a_symlink_dst).map(|m| m.file_type().is_symlink()).unwrap_or(false),
        "symlink destination should be a symlink"
    );

    // Manifest jsonl assertions: one Sync event per executed step.
    let log = f.root.join(".grex/grex.jsonl");
    let events = manifest::read_all(&log).expect("log readable");
    assert!(events.len() >= 3, "expected >=3 events, got {}", events.len());
    let sym_src_ok = f.a_symlink_src.exists();
    assert!(sym_src_ok, "fixture sym src should exist");
}

#[test]
fn e2e_cycle_aborts() {
    // Build a pack whose children reference a graph cycle. We do this at
    // the mock-loader level of granularity is unavailable from the CLI path,
    // so instead drive a cycle via `depends_on` pointing nowhere — that
    // surfaces as a validation error (depends_on unsatisfied). A true cycle
    // would surface from the walker with TreeError::CycleDetected, which
    // maps to exit code 3 (Tree), tested separately.
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    // Construct: root child references itself via a bare repo whose manifest
    // re-references the same bare repo URL. That's a cycle at walker level.
    init_git_identity();
    let cyc_work = tmp_path.join("cyc-work");
    fs::create_dir_all(cyc_work.join(".grex")).unwrap();
    run_git(&cyc_work, &["init", "-q", "-b", "main"]);
    run_git(&cyc_work, &["config", "user.email", "g@g"]);
    run_git(&cyc_work, &["config", "user.name", "g"]);
    // Placeholder yaml, patched below after we know the bare url.
    fs::write(cyc_work.join(".grex/pack.yaml"), "schema_version: \"1\"\nname: cyc\ntype: meta\n")
        .unwrap();
    run_git(&cyc_work, &["add", "-A"]);
    run_git(&cyc_work, &["commit", "-q", "-m", "seed"]);
    let cyc_bare = tmp_path.join("cyc.git");
    run_git(
        tmp_path,
        &["clone", "-q", "--bare", cyc_work.to_str().unwrap(), cyc_bare.to_str().unwrap()],
    );
    let cyc_url = file_url_from_path(&cyc_bare);
    // Rewrite work copy manifest to list itself as a child, then amend.
    let self_yaml = format!(
        "schema_version: \"1\"\nname: cyc\ntype: meta\nchildren:\n  - url: {cyc_url}\n    path: cyc\n",
    );
    fs::write(cyc_work.join(".grex/pack.yaml"), &self_yaml).unwrap();
    run_git(&cyc_work, &["add", "-A"]);
    run_git(&cyc_work, &["commit", "-q", "-m", "cycle"]);
    run_git(&cyc_work, &["push", "-q", cyc_bare.to_str().unwrap(), "main"]);

    // Root references cyc.
    let root_dir = tmp_path.join("root");
    let root_yaml = format!(
        "schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n  - url: {cyc_url}\n    path: cyc\n",
    );
    write_root(&root_dir, &root_yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let err = run(&root_dir, &options(false, workspace)).unwrap_err();
    match err {
        SyncError::Tree(_) => {}
        other => panic!("expected TreeError cycle, got {other:?}"),
    }
}

#[test]
fn e2e_depends_on_unsatisfied() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root_dir = tmp_path.join("root");
    let root_yaml =
        "schema_version: \"1\"\nname: root\ntype: meta\ndepends_on:\n  - zzz-missing\n".to_string();
    write_root(&root_dir, &root_yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let err = run(&root_dir, &options(false, workspace)).unwrap_err();
    match err {
        SyncError::Validation { errors } => {
            assert!(
                errors.iter().any(|e| format!("{e}").contains("zzz-missing")),
                "errors must mention unresolved dep: {errors:?}"
            );
        }
        other => panic!("expected Validation, got {other:?}"),
    }
}

#[test]
fn e2e_validation_skip_bypasses_checks() {
    // Same shape as `e2e_depends_on_unsatisfied` but with `validate: false`.
    // The walk still succeeds (no children to traverse), no validator trips,
    // no actions to execute — sync completes OK, proving the flag bypasses
    // the validator layer.
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root_dir = tmp_path.join("root");
    let root_yaml =
        "schema_version: \"1\"\nname: root\ntype: meta\ndepends_on:\n  - zzz-missing\n".to_string();
    write_root(&root_dir, &root_yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let opts = SyncOptions { dry_run: true, validate: false, workspace: Some(workspace) };
    let report = run(&root_dir, &opts).expect("--no-validate must bypass");
    assert!(report.halted.is_none());
    // No actions on root, no children: zero steps.
    assert_eq!(report.steps.len(), 0);
    assert_eq!(report.graph.nodes().len(), 1);
    // Silence unused-field warnings on the StepKind import on empty runs.
    let _ = std::mem::size_of::<StepKind>();
}
