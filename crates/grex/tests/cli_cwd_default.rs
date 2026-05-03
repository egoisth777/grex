//! v1.3.1 B2 — `<pack_root>` cwd default coverage.
//!
//! Asserts the operator-facing surface contract from
//! `openspec/changes/feat-v1.3.1/proposal.md` §B2:
//!
//! Resolution order for `pack_root`:
//!   1. Explicit `--pack <path>` (or legacy `--workspace`) and the
//!      positional `<pack_root>` — use as-is.
//!   2. No flag AND `cwd/.grex/pack.yaml` is a file — default
//!      `pack_root = cwd` (mirrors `git status` defaulting to cwd
//!      when `.git/` is present).
//!   3. Else: emit existing "<pack_root> required" usage error.
//!
//! Tests drive the real `grex` binary via `assert_cmd::Command`. The
//! spawned process's cwd is set via `.current_dir(...)`, so the parent
//! test runner's cwd is not mutated and tests can run in parallel.

use assert_cmd::prelude::*;
use std::fs;
use std::process::Command;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary built")
}

/// Stable substring of the verb-level usage error emitted on stderr
/// when the cwd lacks a pack-marker AND the operator omitted the
/// positional. Loose enough to survive copy-polish; tight enough to
/// detect when the cwd-default fast-path stops firing.
const PACK_ROOT_REQUIRED: &str = "<pack_root> required";

/// Seed `<dir>/.grex/pack.yaml` with a minimal pack manifest. The
/// manifest is just enough for `grex sync --no-validate --dry-run` to
/// reach the dry-run executor without erroring on missing required
/// fields. Used by the cwd-default tests below.
fn seed_pack_marker(dir: &std::path::Path) {
    let grex_dir = dir.join(".grex");
    fs::create_dir_all(&grex_dir).expect("create .grex/");
    let yaml = "name: cwd-default-test\n";
    fs::write(grex_dir.join("pack.yaml"), yaml).expect("write pack.yaml");
}

#[test]
fn sync_defaults_pack_root_to_cwd_when_marker_present() {
    // Case 1 — cwd has `.grex/pack.yaml`, no positional, no flags →
    // sync MUST resolve `pack_root = cwd` and skip the
    // "<pack_root> required" usage-error fall-through. We don't
    // require the verb to succeed end-to-end (the manifest is a
    // minimal stub); we only assert that the cwd-default short-circuit
    // fired by checking that stderr does NOT carry the usage error.
    let tmp = TempDir::new().expect("tempdir");
    seed_pack_marker(tmp.path());
    let out = bin()
        .current_dir(tmp.path())
        .args(["sync", "--dry-run", "--no-validate"])
        .output()
        .expect("spawn grex sync");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(PACK_ROOT_REQUIRED),
        "cwd has `.grex/pack.yaml` so `<pack_root> required` must NOT fire; got stderr:\n---\n{stderr}\n---"
    );
}

#[test]
fn sync_emits_usage_error_when_cwd_lacks_marker() {
    // Case 2 — cwd has no marker, no positional → preserve the legacy
    // "<pack_root> required" usage error and exit 2. Bare-tempdir
    // cwd has no `.grex/`, so the cwd-default fast-path does NOT fire.
    let tmp = TempDir::new().expect("tempdir");
    bin()
        .current_dir(tmp.path())
        .args(["sync"])
        .assert()
        .failure()
        .code(2)
        .stderr(predicates::str::contains(PACK_ROOT_REQUIRED));
}

#[test]
fn sync_explicit_pack_root_wins_over_cwd_default() {
    // Case 3 — cwd HAS the pack marker, but the operator passed
    // `<pack_root>` explicitly → flag wins, `pack_root` is the
    // explicit path (which here is a different tempdir without a
    // marker, so `sync` must error out reading the manifest from
    // THAT path, not from cwd). The verb's own loader/validate layer
    // produces the secondary error; we only assert that the explicit
    // path was honoured by checking that stderr does NOT carry the
    // verb-level "<pack_root> required" line (which would mean the
    // explicit positional was discarded).
    let cwd_with_marker = TempDir::new().expect("cwd tempdir");
    seed_pack_marker(cwd_with_marker.path());
    let elsewhere = TempDir::new().expect("elsewhere tempdir");
    let out = bin()
        .current_dir(cwd_with_marker.path())
        .arg("sync")
        .arg(elsewhere.path())
        .arg("--dry-run")
        .arg("--no-validate")
        .output()
        .expect("spawn grex sync <elsewhere>");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(PACK_ROOT_REQUIRED),
        "explicit positional supplied — `<pack_root> required` must NOT fire; got stderr:\n---\n{stderr}\n---"
    );
}

#[test]
fn teardown_defaults_pack_root_to_cwd_when_marker_present() {
    // Same contract as `sync_defaults_pack_root_to_cwd_when_marker_present`
    // but for `grex teardown` — it shares the v1.3.0 freeze surface and
    // emitted the same `<pack_root> required` usage error pre-v1.3.1.
    let tmp = TempDir::new().expect("tempdir");
    seed_pack_marker(tmp.path());
    let out = bin()
        .current_dir(tmp.path())
        .args(["teardown", "--no-validate"])
        .output()
        .expect("spawn grex teardown");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(PACK_ROOT_REQUIRED),
        "cwd has `.grex/pack.yaml` so `grex teardown` must NOT emit `<pack_root> required`; got stderr:\n---\n{stderr}\n---"
    );
}
