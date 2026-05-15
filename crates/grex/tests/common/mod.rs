//! Shared helpers for integration tests.
//!
//! Every test module declares `mod common;` at the top to pull these in.

#![allow(dead_code)] // Not every test uses every helper.

use assert_cmd::Command;

/// Construct a `Command` pointing at the built `grex` binary.
pub fn grex() -> Command {
    Command::cargo_bin("grex").expect("grex binary available")
}

/// The canonical list of CLI verbs. Keep in lockstep with `cli::args::Verb`.
/// Used by tests that need to enumerate every verb (e.g. help-output checks).
pub const VERBS: &[&str] = &[
    "init", "add", "rm", "ls", "status", "sync", "update", "doctor", "serve", "import", "run",
    "exec",
];

/// Verbs that previously shipped as M1 stubs. v1.4.0 wired all six
/// (`init`, `rm`, `status`, `update`, `run`, `exec`) — the slice is
/// retained empty for downstream test scaffolds that still loop over
/// "stub-shaped" verbs as a no-op; new tests should iterate `VERBS`
/// directly or target individual verbs by name.
pub const STUB_VERBS: &[&str] = &[];

/// Return the minimal required positional args for a verb.
/// Verbs with no required positionals return an empty vec.
pub fn required_args_for(verb: &str) -> Vec<&'static str> {
    match verb {
        "add" => vec!["https://example.com/repo.git"],
        "rm" => vec!["my-pack"],
        "run" => vec!["symlink"],
        "exec" => vec!["echo", "hi"],
        _ => vec![],
    }
}

/// v1.4.0 — every verb is now wired; this helper exercises the global
/// flag parser against `<verb> --help` so the parser surface is still
/// checked without actually running the verb's side effects. The
/// previous "unimplemented" assertion is gone (v1.3.x M1 scaffolds are
/// retired). Each verb's real behavior gets its own dedicated test file.
pub fn run_with_flags(verb: &str, flags: &[&str]) {
    let mut cmd = grex();
    cmd.arg(verb);
    cmd.args(flags);
    cmd.arg("--help");
    cmd.assert().success();
}
