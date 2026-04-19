//! Property tests for flag/verb parsing. Uses `proptest` to fuzz numeric and
//! string inputs within the universal-flag surface.

mod common;

use common::grex;
use proptest::prelude::*;

const VERBS: &[&str] = &[
    "init", "add", "rm", "ls", "status", "sync", "update", "doctor", "serve", "import", "run",
    "exec",
];

fn required_args(verb: &str) -> Vec<&'static str> {
    match verb {
        "add" => vec!["https://example.com/repo.git"],
        "rm" => vec!["my-pack"],
        "run" => vec!["symlink"],
        "exec" => vec!["echo", "hi"],
        _ => vec![],
    }
}

proptest! {
    // Property runs can be slow when each case spawns a binary. Keep cases
    // modest so total test runtime stays well under 10s.
    #![proptest_config(ProptestConfig {
        cases: 64,
        .. ProptestConfig::default()
    })]

    /// Any `--parallel N` with N in 1..=1024 parses successfully on `init`.
    #[test]
    fn parallel_accepts_reasonable_values(n in 1u32..=1024) {
        grex()
            .args(["init", "--parallel"])
            .arg(n.to_string())
            .assert()
            .success();
    }

    /// Any `--parallel N` outside 1..=1024 must be rejected. Covers both
    /// the low side (0) and the high side (1025..=u32::MAX). Asserts that
    /// stderr is non-empty.
    #[test]
    fn parallel_rejects_out_of_range(
        n in prop_oneof![
            Just(0u32),
            1025u32..=u32::MAX,
        ]
    ) {
        let output = grex()
            .args(["init", "--parallel"])
            .arg(n.to_string())
            .assert()
            .failure();
        let stderr = String::from_utf8(output.get_output().stderr.clone())
            .expect("stderr is UTF-8");
        prop_assert!(!stderr.is_empty(), "stderr should be non-empty on validation failure");
    }

    /// Any `--filter` value using the alphanumeric + `=,` alphabet parses.
    #[test]
    fn filter_accepts_typical_expressions(
        expr in proptest::string::string_regex("[a-zA-Z0-9=,]{1,32}").unwrap()
    ) {
        grex()
            .args(["init", "--filter"])
            .arg(expr)
            .assert()
            .success();
    }

    /// M1 does no filter-expression validation — empty and whitespace-only
    /// filter strings are permissively accepted today. Codify this so the
    /// M2 validator change is a visible regression.
    #[test]
    fn filter_accepts_empty_and_whitespace(
        expr in proptest::string::string_regex(r"[ \t]{0,16}").unwrap()
    ) {
        grex()
            .args(["init", "--filter"])
            .arg(expr)
            .assert()
            .success();
    }

    /// Random verb-shaped strings that are *not* in the 12 must fail with
    /// non-empty stderr. Use `prop_filter` so the strategy itself excludes
    /// real verbs (rather than `prop_assume!` silently discarding cases).
    #[test]
    fn bogus_verb_names_fail(
        bogus in proptest::string::string_regex("[a-z]{3,16}")
            .unwrap()
            .prop_filter("must not be a real verb", |s| !VERBS.contains(&s.as_str()))
    ) {
        let output = grex().arg(bogus).assert().failure();
        let stderr = String::from_utf8(output.get_output().stderr.clone())
            .expect("stderr is UTF-8");
        prop_assert!(!stderr.is_empty(), "stderr should be non-empty on unknown-verb failure");
    }
}

/// A non-property sanity test: every verb accepts its required args plus
/// `--parallel 1` — catches bitrot in `required_args` when verbs shift.
#[test]
fn each_verb_accepts_parallel_one() {
    for verb in VERBS {
        let mut cmd = grex();
        cmd.arg(verb);
        cmd.args(required_args(verb));
        cmd.args(["--parallel", "1"]);
        cmd.assert().success();
    }
}
