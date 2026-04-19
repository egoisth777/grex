// Smoke-level minimal coverage. Full per-verb coverage lives in tests/verb_parsing.rs.

use assert_cmd::prelude::*;
use predicates::prelude::*;
use std::process::Command;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary")
}

#[test]
fn version_flag_prints_version() {
    bin().arg("--version").assert().success().stdout(predicate::str::contains("grex"));
}

#[test]
fn help_lists_all_verbs() {
    let out = bin().arg("--help").assert().success();
    let stdout = String::from_utf8(out.get_output().stdout.clone()).unwrap();
    // Small stable subset; exhaustive per-verb coverage lives in
    // tests/verb_parsing.rs and tests/help_output.rs.
    for v in ["init", "add", "sync", "doctor"] {
        assert!(stdout.contains(v), "help missing verb: {}", v);
    }
}

#[test]
fn init_stub_exits_zero() {
    bin().arg("init").assert().success().stdout(predicate::str::contains("unimplemented"));
}

#[test]
fn add_stub_requires_url() {
    bin()
        .arg("add")
        .assert()
        .failure()
        .stderr(predicate::str::contains("Usage:").or(predicate::str::contains("required")));
}
