//! One test per verb: valid invocation, missing-required-arg failure,
//! and unknown-arg failure.

mod common;

use common::{grex, required_args_for, STUB_VERBS, VERBS};
use predicates::prelude::*;

/// Each stub verb, invoked with its minimal required args, exits 0 and
/// prints the M1 stub marker. `serve` is excluded — see `STUB_VERBS` doc.
#[test]
fn every_verb_stub_runs_and_prints_unimplemented() {
    for verb in STUB_VERBS {
        let mut cmd = grex();
        cmd.arg(verb);
        cmd.args(required_args_for(verb));
        cmd.assert().success().stdout(predicate::str::contains("unimplemented"));
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

/// `init` — no args, succeeds.
#[test]
fn init_stub() {
    grex().arg("init").assert().success().stdout(predicate::str::contains("unimplemented"));
}

/// `ls`, `status`, `doctor` — no args, all succeed.
#[test]
fn zero_arg_verbs_succeed() {
    for verb in ["ls", "status", "doctor"] {
        grex().arg(verb).assert().success().stdout(predicate::str::contains("unimplemented"));
    }
}
