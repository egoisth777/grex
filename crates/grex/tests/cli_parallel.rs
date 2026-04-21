//! feat-m6-1 — CLI parse tests for `grex sync --parallel N`.
//!
//! These are integration-style tests that use `assert_cmd` to spawn the real
//! `grex` binary. They assert only on clap's exit-code / stderr semantics —
//! no `.grex/` side effects beyond the usual `sync` stub, which exits 0 when
//! no `pack_root` is given.
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

/// `grex sync --parallel 4` parses and exits cleanly. Without a `pack_root`
/// argument `sync` prints the M1 scaffold stub and exits 0, which is enough
/// to confirm the flag reached clap.
#[test]
fn parallel_flag_parses() {
    Command::cargo_bin("grex")
        .expect("binary built")
        .args(["sync", "--parallel", "4"])
        .assert()
        .success();
}

#[test]
fn parallel_zero_accepted_unbounded() {
    // `--parallel 0` is the documented "unbounded" sentinel per spec.
    // Must NOT be rejected by clap.
    Command::cargo_bin("grex")
        .expect("binary built")
        .args(["sync", "--parallel", "0"])
        .assert()
        .success();
}

#[test]
fn parallel_one_preserves_serial_flag() {
    // `--parallel 1` is the serial fast-path. Still parses; still exits 0.
    Command::cargo_bin("grex")
        .expect("binary built")
        .args(["sync", "--parallel", "1"])
        .assert()
        .success();
}

#[test]
fn parallel_env_var_used_when_flag_absent() {
    // `GREX_PARALLEL=2` is the env-fallback. We cannot observe the
    // resolved value from the stub-exit path, so we only assert the env
    // var is NOT rejected by clap (it must route via the `env` attribute,
    // not a positional).
    Command::cargo_bin("grex")
        .expect("binary built")
        .env("GREX_PARALLEL", "2")
        .args(["sync"])
        .assert()
        .success();
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
