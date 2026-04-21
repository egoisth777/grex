//! Parametric-style tests: each of the 12 verbs must accept each universal
//! flag, plus sensible combinations.

mod common;

use common::{grex, required_args_for, run_with_flags, VERBS};
use predicates::prelude::*;

#[test]
fn json_flag_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--json"]);
    }
}

#[test]
fn plain_flag_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--plain"]);
    }
}

#[test]
fn dry_run_flag_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--dry-run"]);
    }
}

// feat-m6 B2: `--parallel` was promoted from a universal flag to a
// sync-scoped flag. Per-verb parallel coverage lives in
// `crates/grex/src/cli/args.rs` unit tests.

#[test]
fn filter_flag_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--filter", "kind=git"]);
    }
}

/// Edge-case filter expressions: empty, whitespace-only, and metachar-heavy.
/// The M1 surface does not yet validate filter expressions, so all should
/// parse successfully.
#[test]
fn filter_edge_cases_accepted() {
    for expr in ["", "   ", "kind=*&name=?"] {
        run_with_flags("init", &["--filter", expr]);
    }
}

#[test]
fn combined_flags_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--json", "--dry-run"]);
    }
}

/// `--json` combo: json + dry-run on every verb.
#[test]
fn all_json_flags_together_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--json", "--dry-run", "--filter", "name=foo"]);
    }
}

/// `--plain` combo: plain + dry-run on every verb (split from the
/// json combo because `--json` and `--plain` are mutually exclusive).
#[test]
fn all_plain_flags_together_accepted_on_every_verb() {
    for verb in VERBS {
        run_with_flags(verb, &["--plain", "--dry-run", "--filter", "name=foo"]);
    }
}

/// `--json` and `--plain` conflict and must be rejected together.
#[test]
fn json_and_plain_conflict() {
    grex().args(["init", "--json", "--plain"]).assert().failure().stderr(
        predicate::str::contains("cannot be used").or(predicate::str::contains("conflict")),
    );
}

/// Global flags work on either side of the verb (`grex --json init` and
/// `grex init --json` should both parse).
#[test]
fn flag_ordering_before_and_after_verb_accepted() {
    // After the verb — standard case.
    for verb in VERBS {
        let mut cmd = grex();
        cmd.arg(verb);
        cmd.args(required_args_for(verb));
        cmd.arg("--json");
        cmd.assert().success();
    }
    // Before the verb — global flag.
    for verb in VERBS {
        let mut cmd = grex();
        cmd.arg("--json");
        cmd.arg(verb);
        cmd.args(required_args_for(verb));
        cmd.assert().success();
    }
}
