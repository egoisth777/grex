//! feat-m6-1 — CLI parse tests for `grex sync --parallel N`.
//!
//! These are integration-style tests that use `assert_cmd` to spawn the real
//! `grex` binary. They assert only on clap's parse outcome — i.e. that the
//! flag is accepted by clap, not that `sync` succeeds. `sync` without a
//! `<pack_root>` positional now surfaces a usage-error envelope and exits 2
//! (feat-m8-release blocker fix), so these tests explicitly assert the
//! stderr shape to distinguish "clap accepted the flag" from "clap rejected
//! it". The former is the invariant under test.
//!
//! The spec distinguishes four parallelism shapes that must all round-trip
//! through clap cleanly:
//!
//! | flag                     | env                 | effective parallel |
//! |--------------------------|---------------------|--------------------|
//! | `--parallel 4`           | (ignored)           | `4`                |
//! | absent                   | `GREX_PARALLEL=2`   | `2`                |
//! | `--parallel 1`           | —                   | `1` (serial)       |
//! | `--parallel 0`           | —                   | unbounded          |
//!
//! A `--parallel 0` MUST NOT be rejected by clap — unbounded is a documented
//! knob. This contrasts with the existing global `--parallel` which rejects
//! 0; `grex sync --parallel` is a sync-scoped flag with its own range.

use assert_cmd::Command;

/// Helper: clap accepted the flag if stderr carries the verb-level usage
/// error (produced by our own fall-through), not the `error:` prefix clap
/// emits on parse failure. Both exit 2 under the new contract, so we
/// differentiate on stderr content.
fn assert_clap_accepted(args: &[&str], env: &[(&str, &str)]) {
    let mut cmd = Command::cargo_bin("grex").expect("binary built");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.args(args).assert().get_output().clone();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("grex sync:") || stderr.contains("<pack_root>"),
        "clap should have accepted the flag; got stderr: {stderr}"
    );
    assert!(
        !stderr.starts_with("error:"),
        "clap parse-error leak (should have been our fall-through): {stderr}"
    );
}

/// `grex sync --parallel 4` parses cleanly (clap-accept, then the
/// missing-pack-root fall-through — not a clap parse error).
#[test]
fn parallel_flag_parses() {
    assert_clap_accepted(&["sync", "--parallel", "4"], &[]);
}

#[test]
fn parallel_zero_accepted_unbounded() {
    // `--parallel 0` is the documented "unbounded" sentinel per spec.
    // Must NOT be rejected by clap.
    assert_clap_accepted(&["sync", "--parallel", "0"], &[]);
}

#[test]
fn parallel_one_preserves_serial_flag() {
    // `--parallel 1` is the serial fast-path. Must parse at clap.
    assert_clap_accepted(&["sync", "--parallel", "1"], &[]);
}

#[test]
fn parallel_env_var_used_when_flag_absent() {
    // `GREX_PARALLEL=2` is the env-fallback. We cannot observe the
    // resolved value from the usage-error path, so we only assert the
    // env var is NOT rejected by clap (it must route via the `env`
    // attribute, not a positional).
    assert_clap_accepted(&["sync"], &[("GREX_PARALLEL", "2")]);
}

#[test]
fn parallel_rejects_negative() {
    // Negative values must fail at clap parse time — we accept `usize`.
    Command::cargo_bin("grex")
        .expect("binary built")
        .args(["sync", "--parallel", "-1"])
        .assert()
        .failure();
}

#[test]
fn parallel_rejects_non_numeric() {
    Command::cargo_bin("grex")
        .expect("binary built")
        .args(["sync", "--parallel", "abc"])
        .assert()
        .failure();
}
