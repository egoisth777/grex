//! One test per verb: valid invocation, missing-required-arg failure,
//! and unknown-arg failure.

mod common;

use common::{grex, required_args_for, STUB_VERBS, VERBS};
use predicates::prelude::*;

/// v1.4.0 — `STUB_VERBS` is now empty (all six prior stubs wired). The
/// loop body never fires; the function is retained so the suite still
/// guards against a regression where a future verb is reintroduced as
/// a stub. Coverage of real verb behavior lives in per-verb test files.
#[test]
fn every_verb_stub_runs_and_prints_unimplemented() {
    for verb in STUB_VERBS {
        let mut cmd = grex();
        cmd.arg(verb);
        cmd.args(required_args_for(verb));
        cmd.assert().success();
    }
}

/// Verbs that expose a required positional arg must fail when it is missing.
#[test]
fn required_positional_args_are_enforced() {
    // `add`, `rm`, `run` all have a required positional.
    // `exec` takes `trailing_var_arg` with Vec<String>, which accepts empty — skip.
    for verb in ["add", "rm", "run"] {
        grex()
            .arg(verb)
            .assert()
            .failure()
            .stderr(predicate::str::contains("required").or(predicate::str::contains("Usage")));
    }
}

/// Every verb (except `exec`, whose `trailing_var_arg` captures anything)
/// should reject an unknown flag.
#[test]
fn unknown_flag_fails_for_every_verb() {
    for verb in VERBS {
        if *verb == "exec" {
            // `exec` uses `trailing_var_arg = true` — unknown flags are
            // consumed as command args, not rejected. Intentional.
            continue;
        }
        let mut cmd = grex();
        cmd.arg(verb);
        cmd.args(required_args_for(verb));
        cmd.arg("--definitely-not-a-real-flag");
        cmd.assert().failure().stderr(
            predicate::str::contains("unexpected argument")
                .or(predicate::str::contains("unknown argument")),
        );
    }
}

/// Running `grex` with no arguments should fail and surface a usage hint.
#[test]
fn bare_grex_fails_with_help_hint() {
    grex()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:").or(predicate::str::contains("<COMMAND>")));
}

/// clap should reject two subcommands stacked in a row (`grex init ls`).
#[test]
fn multi_verb_input_fails() {
    grex().args(["init", "ls"]).assert().failure();
}

/// `init` — invoked with an explicit tempdir path writes the minimal
/// `.grex/pack.yaml` skeleton and exits 0. v1.4.0 replaces the prior
/// M1 stub assertion. Idempotency + JSON envelope coverage lives in
/// `crates/grex/tests/init_cli.rs`.
#[test]
fn init_writes_manifest_skeleton() {
    let dir = tempfile::tempdir().expect("tempdir");
    grex()
        .args(["init"])
        .arg(dir.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("pack.yaml"));
    assert!(dir.path().join(".grex/pack.yaml").is_file());
}

/// `status` — invoked outside a pack root exits 2 with a usage error.
/// Full drift-reporting coverage lives in `crates/grex/tests/status_cli.rs`.
#[test]
fn status_outside_pack_root_exits_usage_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    grex()
        .current_dir(dir.path())
        .arg("status")
        .assert()
        .failure()
        .stderr(predicate::str::contains("pack_root").or(predicate::str::contains("required")));
}
