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

/// Verbs whose stub still exits 0 with "unimplemented" stdout when invoked.
///
/// `serve` is excluded as of feat-m7-1 stage 8: it is now a real long-running
/// stdio MCP loop (no "unimplemented" message, exits non-zero on closed
/// stdin without a handshake). Its dedicated coverage lives in
/// `crates/grex/tests/serve_smoke.rs`. `doctor` is excluded as of feat-m7-4b:
/// it now executes real checks against the current working directory and
/// exits with a severity-derived code, so its dedicated coverage lives in
/// `crates/grex/tests/doctor_cli.rs`. `sync` is excluded as of
/// feat-m8-release: the bare-invocation fall-through now emits a
/// `usage` error envelope and exits 2 (see `man/reference/cli-json.md`
/// §"Missing `<pack_root>`"); its dedicated coverage lives in
/// `crates/grex/tests/json_output.rs::sync_without_pack_root_json_emits_usage_error`
/// and the E2E suite. Use this slice for parametric tests that actually
/// *run* the verb; use `VERBS` for tests that only inspect help text or
/// the verb-name surface.
pub const STUB_VERBS: &[&str] = &["init", "add", "rm", "ls", "status", "update", "run", "exec"];

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
