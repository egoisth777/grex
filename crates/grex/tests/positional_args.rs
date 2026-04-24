//! Verb-specific positional-arg behaviours.

mod common;

use common::grex;
use predicates::prelude::*;

// ---------- add ----------

#[test]
fn add_with_url_only_succeeds() {
    grex()
        .args(["add", "https://example.com/repo.git"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn add_with_url_and_path_succeeds() {
    grex()
        .args(["add", "https://example.com/repo.git", "my-path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn add_with_no_url_fails() {
    grex().arg("add").assert().failure();
}

// ---------- rm ----------

#[test]
fn rm_with_path_succeeds() {
    grex()
        .args(["rm", "my-pack"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn rm_without_path_fails() {
    grex().arg("rm").assert().failure();
}

// ---------- update ----------

#[test]
fn update_without_pack_succeeds() {
    grex().arg("update").assert().success().stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn update_with_pack_succeeds() {
    grex()
        .args(["update", "my-pack"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

// ---------- run ----------

#[test]
fn run_with_action_succeeds() {
    grex()
        .args(["run", "symlink"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn run_without_action_fails() {
    grex().arg("run").assert().failure();
}

// ---------- exec ----------

#[test]
fn exec_with_trailing_args_succeeds() {
    grex()
        .args(["exec", "echo", "hi", "there"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn exec_with_single_arg_succeeds() {
    grex()
        .args(["exec", "echo"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

/// `trailing_var_arg = true` on a `Vec<String>` accepts zero args — so
/// `grex exec` currently parses to an empty `cmd` vec and succeeds. A real
/// required-non-empty check will land with the exec runtime in M2/M3.
#[test]
fn exec_without_args_currently_succeeds() {
    grex().arg("exec").assert().success().stdout(predicate::str::contains("unimplemented"));
}

// ---------- boundary values ----------

/// Empty URL on `add` — clap only cares that the positional is present, so
/// `grex add ""` currently parses. Semantic URL validation belongs to M2/M3.
#[test]
fn add_empty_url_currently_succeeds() {
    grex().args(["add", ""]).assert().success().stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn rm_unicode_path_succeeds() {
    grex()
        .args(["rm", "unicode-пакет-🎯"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn rm_long_path_succeeds() {
    let long = "a".repeat(512);
    grex()
        .args(["rm", long.as_str()])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
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
        .failure();
}

#[cfg(windows)]
#[test]
fn rm_with_windows_relative_path_succeeds() {
    grex()
        .args(["rm", r".\pack"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
}

#[cfg(windows)]
#[test]
fn rm_with_windows_parent_relative_path_succeeds() {
    grex()
        .args(["rm", r"..\pack"])
        .assert()
        .success()
        .stdout(predicate::str::contains("unimplemented"));
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
    grex().args(["import", "--from-repos-json", "./does-not-exist.json"]).assert().failure();
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
    grex()
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
    grex()
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
