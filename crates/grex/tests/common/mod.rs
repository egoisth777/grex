//! Shared helpers for integration tests.
//!
//! Every test module declares `mod common;` at the top to pull these in.

#![allow(dead_code)] // Not every test uses every helper.

use assert_cmd::Command;

/// Construct a `Command` pointing at the built `grex` binary.
pub fn grex() -> Command {
    Command::cargo_bin("grex").expect("grex binary available")
}

/// The canonical list of M1 verbs. Keep in lockstep with `cli::args::Verb`.
pub const VERBS: &[&str] = &[
    "init", "add", "rm", "ls", "status", "sync", "update", "doctor", "serve", "import", "run",
    "exec",
];

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

/// Run a verb with the given universal-flag slice and assert success +
/// "unimplemented" stub output. Flags are passed through verbatim so callers
/// can shape them (e.g. `&["--json", "--dry-run"]`).
pub fn run_with_flags(verb: &str, flags: &[&str]) {
    let mut cmd = grex();
    cmd.arg(verb);
    cmd.args(required_args_for(verb));
    cmd.args(flags);
    let assert = cmd.assert().success();
    let stdout =
        String::from_utf8(assert.get_output().stdout.clone()).expect("grex stdout is valid UTF-8");
    assert!(
        stdout.contains("unimplemented"),
        "verb `{verb}` with flags {flags:?} did not print 'unimplemented'; got: {stdout}"
    );
}
