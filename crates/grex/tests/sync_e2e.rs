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

/// Build a `Vec<String>` of `--only` raw patterns for test assertions.
/// `grex-core` now compiles the globset internally so tests drive the
/// same entry point the CLI uses — a `Vec<String>`.
fn only_patterns(patterns: &[&str]) -> Vec<String> {
    patterns.iter().map(|p| (*p).to_string()).collect()
}

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
    SyncOptions::new().with_dry_run(dry_run).with_validate(true).with_workspace(Some(workspace))
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

    let opts =
        SyncOptions::new().with_dry_run(true).with_validate(false).with_workspace(Some(workspace));
    let report = run(&root_dir, &opts).expect("--no-validate must bypass");
    assert!(report.halted.is_none());
    // No actions on root, no children: zero steps.
    assert_eq!(report.steps.len(), 0);
    assert_eq!(report.graph.nodes().len(), 1);
    // Silence unused-field warnings on the StepKind import on empty runs.
    let _ = std::mem::size_of::<StepKind>();
}

// ---------------------------------------------------------------------------
// M4-D D2 + post-review fix bundle — `--only <GLOB>` end-to-end filter
//
// Post-F1+F2 semantics: the matcher evaluates against each pack's
// **workspace-relative** path normalized to forward-slash form. Bare
// names (`c`, `a`) no longer match — F2 drops the name-OR-path fallback
// per spec §M4 req 6. Root packs (not under `workspace`) fall back to
// their absolute forward-slash path.
//
// These tests drive the same 3-level fixture used by the wet/dry-run
// e2e tests and assert on disk-side effects to prove the filter short-
// circuits execution, not just step rendering.
// ---------------------------------------------------------------------------

#[test]
fn e2e_only_filter_by_pack_name_runs_just_one_pack() {
    let f = build_fixture();
    // Walker flattens children under the workspace root, so pack `c`'s
    // workspace-relative path is bare `c`. The glob matches that path.
    // (In a nested-layout world the path would be `b/c` — the matcher
    // handles both forms transparently via forward-slash normalization.)
    let opts = SyncOptions::new()
        .with_workspace(Some(f.workspace.clone()))
        .with_only_patterns(Some(only_patterns(&["c"])));
    let report = run(&f.root, &opts).expect("only-filter sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);

    // Only c should have produced its target dir; a's artifacts stay absent.
    assert!(f.c_target_dir.is_dir(), "c must have executed");
    assert!(!f.a_target_dir.exists(), "a must have been filtered out");
    assert!(!f.a_symlink_dst.exists(), "a symlink must have been filtered out");
}

#[test]
fn e2e_only_absolute_path_glob_does_not_match() {
    // F1+F2 regression: prior to the M4-D post-review fix bundle, the
    // matcher evaluated against `pack_path.display().to_string()` — an
    // absolute path. A glob built from that absolute path would match.
    // Post-fix the matcher evaluates against **workspace-relative**
    // paths only, so an absolute-path glob MUST NOT match anything.
    let f = build_fixture();
    let abs_a = f.workspace.join("a").display().to_string();
    // Replace `\\` so the glob parses on Windows; the point is the glob
    // text is the (now-unused) absolute representation, not the
    // workspace-relative `a`.
    let abs_a = abs_a.replace('\\', "/");
    let opts = SyncOptions::new()
        .with_workspace(Some(f.workspace.clone()))
        .with_only_patterns(Some(vec![abs_a]));
    let report = run(&f.root, &opts).expect("only-filter sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert_eq!(
        report.steps.len(),
        0,
        "absolute-path glob must not match any workspace-relative path: {:?}",
        report.steps
    );
    assert!(!f.a_target_dir.exists(), "a must NOT have executed via absolute glob");
    assert!(!f.c_target_dir.exists(), "c must NOT have executed");
}

#[test]
fn e2e_only_filter_multiple_patterns_or_combine() {
    let f = build_fixture();
    // Two workspace-relative patterns OR-combined: both packs execute.
    let opts = SyncOptions::new()
        .with_workspace(Some(f.workspace.clone()))
        .with_only_patterns(Some(only_patterns(&["a", "c"])));
    let report = run(&f.root, &opts).expect("multi-pattern only sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert!(f.a_target_dir.is_dir(), "a must have executed");
    assert!(f.c_target_dir.is_dir(), "c must have executed");
}

#[test]
fn e2e_only_filter_non_matching_skips_everything() {
    let f = build_fixture();
    // Pattern matches nothing in the fixture. No action should execute
    // and no lockfile should be written (write only happens after at
    // least one pack runs through the upsert path — but more importantly,
    // even if the file is written it must carry zero entries).
    let opts = SyncOptions::new()
        .with_workspace(Some(f.workspace.clone()))
        .with_only_patterns(Some(only_patterns(&["zzz-no-match"])));
    let report = run(&f.root, &opts).expect("non-matching only sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert_eq!(report.steps.len(), 0, "zero steps when nothing matches");
    assert!(!f.a_target_dir.exists(), "a must not have executed");
    assert!(!f.c_target_dir.exists(), "c must not have executed");
    assert!(!f.a_symlink_dst.exists(), "symlink must not have been created");

    // Lockfile: either absent, or present with zero `"id"` entries.
    let lockfile = f.root.join(".grex/grex.lock.jsonl");
    if lockfile.exists() {
        let body = fs::read_to_string(&lockfile).unwrap();
        assert!(!body.contains("\"id\":"), "no pack entries when nothing matched: {body:?}");
    }
}

#[test]
fn e2e_only_filter_matches_workspace_relative_path() {
    // F1: matcher evaluates against workspace-relative path normalized
    // to forward-slash. This is the uniform representation that is
    // identical on Windows and POSIX — no more platform-skewed
    // absolute-path globs that fail on one host and succeed on the
    // other.
    let f = build_fixture();
    let opts = SyncOptions::new()
        .with_workspace(Some(f.workspace.clone()))
        .with_only_patterns(Some(only_patterns(&["a"])));
    let report = run(&f.root, &opts).expect("path-matched only sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    assert!(f.a_target_dir.is_dir(), "a must have executed via path match");
    assert!(!f.c_target_dir.exists(), "c must have been filtered out");
}

#[test]
fn e2e_only_filter_preserves_prior_lock_entries_for_filtered_packs() {
    // F3: when `--only` excludes a pack, its prior lockfile entry must
    // be carried forward. Otherwise the next unfiltered sync sees no
    // prior hash for the filtered pack and re-executes from scratch.
    let f = build_fixture();
    let base = SyncOptions::new().with_workspace(Some(f.workspace.clone()));

    // Run A: full sync, both `a` and `c` land in the lockfile.
    run(&f.root, &base).expect("sync A ok");
    let lockfile_path = f.root.join(".grex/grex.lock.jsonl");
    let body_a = fs::read_to_string(&lockfile_path).expect("lockfile exists");
    assert!(body_a.contains("\"id\":\"a\""), "pack a in lockfile after A: {body_a}");
    assert!(body_a.contains("\"id\":\"c\""), "pack c in lockfile after A: {body_a}");

    // Run B: filter to `a` only. `c` must be carried forward from the
    // prior lock, not dropped.
    let only_b = base.clone().with_only_patterns(Some(only_patterns(&["a"])));
    run(&f.root, &only_b).expect("sync B ok");
    let body_b = fs::read_to_string(&lockfile_path).expect("lockfile exists");
    assert!(body_b.contains("\"id\":\"c\""), "pack c preserved across filtered sync: {body_b}");

    // Run C: full sync again. `c`'s inputs are unchanged (actions +
    // commit SHA identical to run A), so skip-on-hash must fire and `c`
    // must emit a `PackSkipped` step — proving its prior entry carried
    // through run B intact.
    let r_c = run(&f.root, &base).expect("sync C ok");
    let c_skipped = r_c
        .steps
        .iter()
        .any(|s| s.pack == "c" && matches!(s.exec_step.details, StepKind::PackSkipped { .. }));
    assert!(
        c_skipped,
        "pack c must short-circuit on run C (prior lock entry preserved through filtered run B): {:?}",
        r_c.steps
    );
}

#[test]
fn e2e_force_plus_dry_run_plans_but_does_not_write_lockfile() {
    // Testing-reviewer P1: `--force` combined with `--dry-run` must
    // still produce the planned-step transcript, but must NOT write a
    // lockfile (dry runs never persist state). Regression guard
    // against the invariant slip where `force=true` short-circuits the
    // dry-run carve-out.
    let f = build_fixture();
    // Warm-up so there is a prior lockfile to NOT-overwrite.
    let base = SyncOptions::new().with_workspace(Some(f.workspace.clone()));
    run(&f.root, &base).expect("warm-up ok");
    let lockfile_path = f.root.join(".grex/grex.lock.jsonl");
    let warm_body = fs::read_to_string(&lockfile_path).expect("warm lockfile");

    let dry_force = SyncOptions::new()
        .with_dry_run(true)
        .with_workspace(Some(f.workspace.clone()))
        .with_force(true);
    let report = run(&f.root, &dry_force).expect("dry+force sync ok");
    assert!(report.halted.is_none(), "halted: {:?}", report.halted);
    // Steps must be plan-only (either WouldPerformChange for unsatisfied
    // actions, or AlreadySatisfied for actions whose effect already
    // exists on disk from the warm-up). PerformedChange would prove the
    // dry-run carve-out was bypassed — assert it does not appear.
    // `--force` must also suppress PackSkipped (no hash short-circuit).
    for s in &report.steps {
        assert!(
            matches!(
                s.exec_step.result,
                ExecResult::WouldPerformChange | ExecResult::AlreadySatisfied | ExecResult::NoOp
            ),
            "dry-run must not emit PerformedChange: {s:?}"
        );
    }
    // Lockfile unchanged — dry-run must never persist.
    let post_body = fs::read_to_string(&lockfile_path).expect("lockfile still present");
    assert_eq!(warm_body, post_body, "dry-run + force must not rewrite lockfile");
}

#[test]
fn e2e_upsert_lock_entry_sha_refreshes_on_commit_sha_change() {
    // Testing-reviewer P1: assert the persisted `sha` field reflects the
    // freshly-probed commit SHA — not just that `PackSkipped` is
    // absent. Drives a second commit into pack `a`'s workspace clone
    // and confirms the new SHA is in the lockfile.
    let f = build_fixture();
    let base = SyncOptions::new().with_workspace(Some(f.workspace.clone()));
    run(&f.root, &base).expect("run 1 ok");
    let lockfile_path = f.root.join(".grex/grex.lock.jsonl");
    let sha_before = extract_pack_sha(&lockfile_path, "a");

    // Bump pack a's workspace HEAD with an empty commit.
    let a_ws = f.workspace.join("a");
    run_git(&a_ws, &["config", "user.email", "grex-test@example.com"]);
    run_git(&a_ws, &["config", "user.name", "grex-test"]);
    run_git(&a_ws, &["commit", "--allow-empty", "-q", "-m", "head-bump"]);

    run(&f.root, &base).expect("run 2 ok");
    let sha_after = extract_pack_sha(&lockfile_path, "a");
    assert_ne!(
        sha_before, sha_after,
        "commit-SHA change must be persisted to lockfile (before={sha_before:?}, after={sha_after:?})"
    );
    assert!(
        sha_after.as_ref().is_some_and(|s| !s.is_empty()),
        "post-bump sha must be non-empty: {sha_after:?}"
    );
}

/// Find the last-line-wins `"sha":"..."` for `pack_name` in the
/// jsonl lockfile. Returns `None` if the pack isn't recorded.
fn extract_pack_sha(lockfile: &Path, pack_name: &str) -> Option<String> {
    let body = fs::read_to_string(lockfile).ok()?;
    let mut last: Option<String> = None;
    let id_tag = format!("\"id\":\"{pack_name}\"");
    for line in body.lines() {
        if !line.contains(&id_tag) {
            continue;
        }
        let Some(rest) = line.split("\"sha\":\"").nth(1) else { continue };
        let Some(end) = rest.find('"') else { continue };
        last = Some(rest[..end].to_string());
    }
    last
}

// ---------------------------------------------------------------------------
// M4-D D3 — commit_sha change invalidates `actions_hash` skip
//
// Spec §M4 req 4a: the resolved commit SHA is mixed into
// `compute_actions_hash`, so a fetched HEAD that changes — even with the
// action list byte-identical — MUST re-execute the pack. This test drives
// the flow by swapping the child's working-tree HEAD between two syncs
// and asserting the second run does NOT emit a `PackSkipped` step.
// ---------------------------------------------------------------------------

#[test]
fn e2e_commit_sha_change_invalidates_skip() {
    let f = build_fixture();

    // Run 1: establishes a lockfile entry for pack `a` keyed on (actions,
    // commit_sha_1).
    let opts = SyncOptions::new().with_workspace(Some(f.workspace.clone()));
    let r1 = run(&f.root, &opts).expect("first sync ok");
    assert!(r1.halted.is_none(), "halted: {:?}", r1.halted);

    // Locate pack a's checked-out workspace repo and advance its HEAD with
    // an empty commit. `GixBackend::head_sha` will now return a fresh SHA,
    // forcing a different `actions_hash` on the next sync.
    let a_ws = f.workspace.join("a");
    assert!(a_ws.join(".git").exists(), "workspace clone of a present");
    run_git(&a_ws, &["config", "user.email", "grex-test@example.com"]);
    run_git(&a_ws, &["config", "user.name", "grex-test"]);
    run_git(&a_ws, &["commit", "--allow-empty", "-q", "-m", "head-bump"]);

    // Run 2: same pack manifest (unchanged actions), new commit SHA.
    let r2 = run(&f.root, &opts).expect("second sync ok");
    assert!(r2.halted.is_none(), "halted: {:?}", r2.halted);
    // `a` must NOT be skipped — commit SHA drift invalidates the hash.
    let a_skipped = r2
        .steps
        .iter()
        .any(|s| s.pack == "a" && matches!(s.exec_step.details, StepKind::PackSkipped { .. }));
    assert!(!a_skipped, "pack `a` with new commit SHA must re-execute, not skip: {:?}", r2.steps);
    // Regression guard: `a`'s actions appear as real executor steps on r2.
    let a_executed = r2
        .steps
        .iter()
        .any(|s| s.pack == "a" && !matches!(s.exec_step.details, StepKind::PackSkipped { .. }));
    assert!(a_executed, "pack `a` must have executed actions on run 2: {:?}", r2.steps);
}

// ---------------------------------------------------------------------------
// M4-D D4 — `--force` bypasses the skip-on-hash short-circuit
//
// With `force = false`, a second run of an unchanged pack yields at least
// one `StepKind::PackSkipped` step. With `force = true`, every pack re-
// executes regardless of lockfile match and zero `PackSkipped` steps
// appear.
// ---------------------------------------------------------------------------

#[test]
fn e2e_force_bypasses_skip_on_hash() {
    let f = build_fixture();

    // Warm-up sync: populates lockfile so the follow-up runs have a hash
    // to match against.
    let base = SyncOptions::new().with_workspace(Some(f.workspace.clone()));
    run(&f.root, &base).expect("warm-up sync ok");

    // force=false: at least one pack should be PackSkipped.
    let r_skip = run(&f.root, &base).expect("second sync ok");
    let skipped_count = r_skip
        .steps
        .iter()
        .filter(|s| matches!(s.exec_step.details, StepKind::PackSkipped { .. }))
        .count();
    assert!(
        skipped_count >= 1,
        "force=false + unchanged inputs must short-circuit at least one pack (got {}): {:?}",
        skipped_count,
        r_skip.steps
    );

    // force=true: NO pack should be PackSkipped.
    let force_opts = base.clone().with_force(true);
    let r_force = run(&f.root, &force_opts).expect("forced sync ok");
    let forced_skips = r_force
        .steps
        .iter()
        .filter(|s| matches!(s.exec_step.details, StepKind::PackSkipped { .. }))
        .count();
    assert_eq!(
        forced_skips, 0,
        "--force must bypass skip-on-hash; got {} skipped steps: {:?}",
        forced_skips, r_force.steps
    );
}
