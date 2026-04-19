//! Help text and version assertions.

mod common;

use common::{grex, VERBS};
use predicates::prelude::*;

#[test]
fn version_flag_prints_semver_and_name() {
    let output = grex().arg("--version").assert().success();
    let stdout =
        String::from_utf8(output.get_output().stdout.clone()).expect("version stdout is UTF-8");
    // Anchored `^grex N.N.N` at the start of the first line. Trailing
    // metadata (e.g. `-alpha.1`, commit hash) is tolerated. We keep the check
    // dependency-free by hand-walking the bytes rather than pulling in regex.
    let first_line = stdout.lines().next().unwrap_or("");
    assert!(
        matches_grex_semver(first_line),
        "version output first line did not match `^grex N.N.N`: {first_line}"
    );
}

/// True iff `s` starts with `grex ` followed by a `\d+\.\d+\.\d+` triple.
fn matches_grex_semver(s: &str) -> bool {
    let Some(rest) = s.strip_prefix("grex ") else {
        return false;
    };
    let mut parts = rest.split('.');
    for _ in 0..3 {
        let Some(p) = parts.next() else {
            return false;
        };
        let digits: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            return false;
        }
    }
    true
}

#[test]
fn top_level_help_lists_every_verb() {
    let output = grex().arg("--help").assert().success();
    let stdout =
        String::from_utf8(output.get_output().stdout.clone()).expect("help stdout is UTF-8");
    for verb in VERBS {
        assert!(stdout.contains(verb), "--help missing verb `{verb}`: {stdout}");
    }
}

#[test]
fn per_verb_help_succeeds_and_mentions_verb() {
    for verb in VERBS {
        let output = grex().args([verb, "--help"]).assert().success();
        let stdout = String::from_utf8(output.get_output().stdout.clone())
            .expect("per-verb help stdout is UTF-8");
        // Word-boundary match for the verb: either bracketed by non-alnum
        // chars, or at line-start/end. We lower-case the haystack for case
        // insensitivity.
        let lower = stdout.to_lowercase();
        let has_verb_word =
            lower.split(|c: char| !c.is_ascii_alphanumeric()).any(|tok| tok == *verb);
        assert!(has_verb_word, "help for `{verb}` did not mention verb as a whole word: {stdout}");
        assert!(
            lower.contains("usage:"),
            "help for `{verb}` did not contain `Usage:` line: {stdout}"
        );
    }
}

#[test]
fn no_args_fails_with_help_hint() {
    grex()
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage").or(predicate::str::contains("help")));
}

#[test]
fn help_output_is_ascii_under_no_color() {
    let output = grex().env("NO_COLOR", "1").arg("--help").assert().success();
    let stdout =
        String::from_utf8(output.get_output().stdout.clone()).expect("help stdout is UTF-8");
    // ANSI escape sequences start with ESC (0x1B). Under NO_COLOR they must
    // not appear.
    assert!(!stdout.contains('\u{001b}'), "help output contained ANSI escape under NO_COLOR=1");
}
