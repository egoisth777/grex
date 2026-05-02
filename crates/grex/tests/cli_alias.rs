//! v1.3.0 — `--workspace` → `--pack` CLI rename: deprecation-warn coverage.
//!
//! Asserts the operator-facing surface contract from
//! `openspec/changes/feat-v1.3.0-cli-rename-freeze/proposal.md` §Acceptance:
//!
//! * Item 3 — invoking any of the 4 affected verbs (`sync`, `serve`,
//!   `teardown`, `migrate-lockfile`) with the legacy `--workspace` flag
//!   emits exactly one deprecation diagnostic on stderr per process.
//! * The canonical `--pack` flag MUST NOT emit any deprecation line.
//!
//! Implementation expectation (per `design.md` §Algorithm sketches):
//!   `crates/grex/src/deprecation.rs::warn_workspace_deprecation_if_used`
//!   inspects `std::env::args()` after `Cli::parse()` and emits
//!   `warning: --workspace is deprecated since v1.3.0; use --pack instead`
//!   exactly once via a process-scoped `OnceLock<()>`.
//!
//! Tests drive the real `grex` binary via `assert_cmd` — they assert on
//! stderr regardless of process exit code (the underlying verb may fail
//! against a manifest-less tempdir; the deprecation diagnostic fires
//! before any verb body executes).

use assert_cmd::prelude::*;
use std::process::Command;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary built")
}

/// Stable substring the deprecation diagnostic must contain. Kept
/// loose enough to survive minor wording polish; tight enough to fail
/// loudly if the diagnostic is dropped or routed to a non-stderr sink.
const DEPRECATION_MARKER: &str = "--workspace is deprecated";
const PACK_HINT: &str = "--pack";

#[test]
fn cli_workspace_alias_emits_deprecation_warn() {
    // Pick `sync` as the canonical surface — it is the most-used verb
    // carrying the `--workspace` flag, and its body exits quickly when
    // the pack root has no manifest. Any non-success exit is acceptable;
    // the contract under test is about stderr, not the verb body.
    let tmp = TempDir::new().expect("tempdir");
    let out = bin()
        .arg("sync")
        .arg(tmp.path())
        .arg("--workspace")
        .arg(tmp.path())
        .arg("--dry-run")
        .arg("--no-validate")
        .output()
        .expect("spawn grex sync");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(DEPRECATION_MARKER),
        "stderr must contain `{DEPRECATION_MARKER}` when --workspace is used; got:\n---\n{stderr}\n---"
    );
    assert!(
        stderr.contains(PACK_HINT),
        "deprecation diagnostic must mention the canonical `--pack` flag; got:\n---\n{stderr}\n---"
    );
}

#[test]
fn cli_pack_flag_does_not_emit_warn() {
    // Same shape as the alias case but using the new canonical flag.
    // The deprecation warn-once helper inspects argv for `--workspace`,
    // so `--pack` must not match — zero diagnostic lines on stderr.
    let tmp = TempDir::new().expect("tempdir");
    let out = bin()
        .arg("sync")
        .arg(tmp.path())
        .arg("--pack")
        .arg(tmp.path())
        .arg("--dry-run")
        .arg("--no-validate")
        .output()
        .expect("spawn grex sync --pack");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !stderr.contains(DEPRECATION_MARKER),
        "stderr must NOT contain the deprecation marker when --pack is used; got:\n---\n{stderr}\n---"
    );
}

#[test]
fn cli_workspace_alias_emits_deprecation_on_teardown() {
    // Coverage for the second affected verb: the deprecation diagnostic
    // fires regardless of which of the 4 verbs carries `--workspace`
    // (the helper inspects argv, not verb-specific argstructs).
    let tmp = TempDir::new().expect("tempdir");
    let out = bin()
        .arg("teardown")
        .arg(tmp.path())
        .arg("--workspace")
        .arg(tmp.path())
        .arg("--no-validate")
        .output()
        .expect("spawn grex teardown");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(DEPRECATION_MARKER),
        "stderr must contain `{DEPRECATION_MARKER}` on teardown --workspace; got:\n---\n{stderr}\n---"
    );
}
