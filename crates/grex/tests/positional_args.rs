//! Verb-specific positional-arg behaviours.

mod common;

use common::grex;
use predicates::prelude::*;

// ---------- add ----------

#[test]
fn add_with_url_only_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/repo.git"])
        .assert()
        .success()
        .stdout(predicate::str::contains("added"));
}

#[test]
fn add_with_url_and_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["add", "https://example.com/repo.git", "my-path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("my-path"));
}

#[test]
fn add_with_no_url_fails() {
    grex().arg("add").assert().failure();
}

// ---------- rm ----------

// v1.4.0 — `rm` is wired against a real path. Invoking it on a
// non-existent path exits 2 (`not_found`). Coverage of the full
// teardown lifecycle lives in `crates/grex/tests/rm_cli.rs`.
#[test]
fn rm_with_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["rm", "my-pack"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn rm_without_path_fails() {
    grex().arg("rm").assert().failure();
}

// ---------- update ----------
//
// v1.4.0 — `update` delegates to `sync`. Outside a pack root it fails
// with the sync usage envelope. Inside a pack root it runs the same
// pipeline as `sync`. Full coverage lives in `crates/grex/tests/update_cli.rs`.

#[test]
fn update_without_pack_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .arg("update")
        .assert()
        .failure()
        .code(2);
}

#[test]
fn update_with_pack_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["update", "my-pack"])
        .assert()
        .failure();
}

// ---------- run ----------
//
// v1.4.0 — `run` requires a pack root. Without one it exits 2.

#[test]
fn run_with_action_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["run", "symlink"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn run_without_action_fails() {
    grex().arg("run").assert().failure();
}

// ---------- exec ----------
//
// v1.4.0 — `exec` spawns the given program in the pack root. Without a
// pack root it exits 2. With `--required` enforced by clap, zero-arg
// invocations also fail at parse time.

#[test]
fn exec_with_trailing_args_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["exec", "--", "echo", "hi", "there"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn exec_with_single_arg_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["exec", "--", "echo"])
        .assert()
        .failure()
        .code(2);
}

/// v1.4.0 — `cmd` is `required = true`, so `grex exec` with no
/// positionals now fails at clap parse time (rather than silently
/// succeeding with an empty cmd vector as it did under the M1 stub).
#[test]
fn exec_without_args_currently_succeeds() {
    grex().arg("exec").assert().failure();
}

// ---------- boundary values ----------

/// Empty URL on `add` — clap only cares that the positional is present, so
/// `grex add ""` currently parses. Semantic URL validation belongs to M2/M3.
#[test]
fn add_empty_url_currently_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["add", "", "--dry-run"])
        .assert()
        .success()
        .stdout(predicate::str::contains("DRY-RUN: would add"));
}

#[test]
fn rm_unicode_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["rm", "unicode-пакет-🎯"])
        .assert()
        .failure()
        .code(2);
}

#[test]
fn rm_long_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    let long = "a".repeat(512);
    grex()
        .current_dir(dir.path())
        .args(["rm", long.as_str()])
        .assert()
        .failure()
        .code(2);
}

// ---------- windows path handling ----------

#[cfg(windows)]
#[test]
fn import_with_windows_drive_path_parses() {
    // feat-m7-4a: `import` is a real verb. A non-existent drive path must
    // fail at I/O — we only assert the CLI parsed the Windows path shape
    // (not a clap rejection) by checking the exit is a runtime failure,
    // not a usage failure.
    grex()
        .args(["import", "--from-repos-json", r"C:\temp\does-not-exist\REPOS.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("grex import"));
}

#[cfg(windows)]
#[test]
fn rm_with_windows_relative_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["rm", r".\pack"])
        .assert()
        .failure()
        .code(2);
}

#[cfg(windows)]
#[test]
fn rm_with_windows_parent_relative_path_succeeds() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["rm", r"..\pack"])
        .assert()
        .failure()
        .code(2);
}

// ---------- import ----------
//
// feat-m7-4a: `import` is a real verb that requires `--from-repos-json
// <path>` and an existing file. Full coverage lives in
// `crates/grex/tests/import_cli.rs`. These two tests retain the
// positional-surface shape only.

#[test]
fn import_with_no_flag_fails_with_message() {
    let out = grex().arg("import").assert().failure().get_output().stderr.clone();
    let s = String::from_utf8(out).unwrap();
    assert!(s.contains("--from-repos-json"), "expected missing-flag message, got: {s}");
}

#[test]
fn import_with_from_repos_json_relative_path_parses() {
    // Relative path parses through clap; a missing file surfaces at I/O
    // time as a runtime failure, not a usage failure.
    grex()
        .args(["import", "--from-repos-json", "./does-not-exist.json"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("grex import"));
}

// ---------- sync ----------
//
// feat-m8-release blocker fix: `sync` without `<pack_root>` now emits a
// usage-error envelope and exits 2 (not the legacy "unimplemented" stub).
// Parse-surface coverage is asserted via `stderr` containing the verb's
// own usage message (not clap's `error:` prefix), so we can still prove
// clap accepted the flag shape.

#[test]
fn sync_default_emits_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .arg("sync")
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("grex sync:").and(predicate::str::contains("pack_root")));
}

/// With `#[arg(long, default_value_t = true)]`, clap derive does **not**
/// synthesize a `--no-recursive` negation. The only supported ways to set the
/// bool are `--recursive` (sets true, the default) and — if we want false —
/// re-declaring the field with `ArgAction::Set`. That is an M2 concern;
/// here we just verify the current spelling parses cleanly (clap-accepted,
/// then the missing-pack-root fall-through).
#[test]
fn sync_recursive_explicit_true_parses() {
    let dir = tempfile::tempdir().unwrap();
    grex()
        .current_dir(dir.path())
        .args(["sync", "--recursive"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicate::str::contains("grex sync:"));
}

// ---------- serve ----------
//
// `serve` is a real long-running stdio MCP loop as of feat-m7-1 stage 8;
// the prior "unimplemented" stub assertions no longer apply. Argument
// parsing is exercised via `cli::args::tests::serve_mcp_flag_parses` (in
// the binary crate), and full handshake coverage lives in
// `crates/grex/tests/serve_smoke.rs`.
