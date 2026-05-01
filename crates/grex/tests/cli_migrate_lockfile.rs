//! End-to-end CLI tests for `grex migrate-lockfile` (v1.2.1 item 2).
//!
//! Spawns the real `grex` binary via `assert_cmd`, prepares a temp
//! workspace with a v1.1.x-shape lockfile, and asserts exit codes +
//! stdout + on-disk bytes for each scenario from
//! `openspec/feat-v1.2.1/spec.md` §"CLI `grex migrate-lockfile`
//! dispatcher" → "Acceptance".

mod common;

use common::grex;
use std::fs;
use tempfile::TempDir;

/// v1.1.1-shape lockfile: two entries with NO `path` field. Matches the
/// fixture used by the library migrator's own unit tests
/// (`crates/grex-core/src/lockfile/migrate_v1_1_1.rs::tests`).
const V1_1_1_LOCKFILE: &str = "\
{\"id\":\"alpha\",\"sha\":\"abc\",\"branch\":\"main\",\"installed_at\":\"2026-04-27T10:00:00Z\",\"actions_hash\":\"h\",\"schema_version\":\"1\"}
{\"id\":\"beta\",\"sha\":\"def\",\"branch\":\"main\",\"installed_at\":\"2026-04-27T10:00:00Z\",\"actions_hash\":\"h\",\"schema_version\":\"1\"}
";

/// Seed `<workspace>/.grex/grex.lock.jsonl` with the v1.1.1-shape
/// fixture; return the lockfile path.
fn seed_v1_1_1_lockfile(workspace: &std::path::Path) -> std::path::PathBuf {
    let lock_dir = workspace.join(".grex");
    fs::create_dir_all(&lock_dir).unwrap();
    let lock_path = lock_dir.join("grex.lock.jsonl");
    fs::write(&lock_path, V1_1_1_LOCKFILE).unwrap();
    lock_path
}

/// AC: `grex migrate-lockfile --dry-run` on a v1.1.x fixture exits 0,
/// reports the would-be migration, and leaves the lockfile bytes
/// untouched.
#[test]
fn dry_run_on_legacy_lockfile_reports_and_does_not_write() {
    let tmp = TempDir::new().unwrap();
    let lock_path = seed_v1_1_1_lockfile(tmp.path());
    let before = fs::read(&lock_path).unwrap();

    let assertion = grex()
        .args(["migrate-lockfile", "--dry-run", "--workspace"])
        .arg(tmp.path())
        .assert()
        .success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("would migrate"),
        "dry-run output must announce a would-migrate; got: {stdout}",
    );

    let after = fs::read(&lock_path).unwrap();
    assert_eq!(before, after, "--dry-run must not write the lockfile");
}

/// AC: `grex migrate-lockfile` on a v1.1.x fixture rewrites the
/// lockfile to v1.2.0 schema (`"path":` field present) and exits 0.
#[test]
fn wet_run_on_legacy_lockfile_rewrites_to_v1_2_0() {
    let tmp = TempDir::new().unwrap();
    let lock_path = seed_v1_1_1_lockfile(tmp.path());

    let assertion =
        grex().args(["migrate-lockfile", "--workspace"]).arg(tmp.path()).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("migrated 2 entries"),
        "wet-run output must report 2 migrated entries; got: {stdout}",
    );

    // Post-migration the bytes must carry the explicit `"path":` field
    // for both entries (the v1.2.0 schema discriminator).
    let raw = fs::read_to_string(&lock_path).unwrap();
    assert!(raw.contains("\"path\":\"alpha\""), "post-migration bytes missing alpha path: {raw}");
    assert!(raw.contains("\"path\":\"beta\""), "post-migration bytes missing beta path: {raw}");
}

/// AC: `grex migrate-lockfile` on an already-v1.2.0 lockfile is a
/// no-op (exit 0, lockfile bytes unchanged). Idempotency: re-running
/// is byte-stable.
#[test]
fn rerun_on_v1_2_0_lockfile_is_a_noop() {
    let tmp = TempDir::new().unwrap();
    let lock_path = seed_v1_1_1_lockfile(tmp.path());

    // First run migrates.
    grex().args(["migrate-lockfile", "--workspace"]).arg(tmp.path()).assert().success();
    let after_first = fs::read(&lock_path).unwrap();

    // Second run must be byte-identical (no rewrite).
    let assertion =
        grex().args(["migrate-lockfile", "--workspace"]).arg(tmp.path()).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("already on v1.2.0"),
        "second run on migrated lockfile must announce no-op; got: {stdout}",
    );

    let after_second = fs::read(&lock_path).unwrap();
    assert_eq!(after_first, after_second, "second run must not rewrite the lockfile bytes");
}

/// AC: a workspace with no lockfile exits 0 and reports the empty
/// state (not an error). Mirrors the library migrator's
/// `MigrationReport::no_lockfile` branch.
#[test]
fn no_lockfile_in_workspace_is_reported_and_exits_zero() {
    let tmp = TempDir::new().unwrap();

    let assertion =
        grex().args(["migrate-lockfile", "--workspace"]).arg(tmp.path()).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("no lockfile"),
        "no-lockfile path must report empty state; got: {stdout}",
    );
}

/// AC: `--help` lists both `--workspace` and `--dry-run` flags, per
/// spec §"Acceptance" line 1.
#[test]
fn help_text_lists_workspace_and_dry_run() {
    let assertion = grex().args(["migrate-lockfile", "--help"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(stdout.contains("--workspace"), "help must mention --workspace; got: {stdout}");
    assert!(stdout.contains("--dry-run"), "help must mention --dry-run; got: {stdout}");
}
