//! Subprocess driver for the `grex` CLI.
//!
//! Black-box only — does **not** link against `grex-core` / `grex-cli`. The
//! caller passes `args` and a `cwd`; we shell out via [`std::process::Command`]
//! and capture stdout / stderr separately so the assertions module can prove
//! invariants like "no warnings on stdout" without ambiguity.
//!
//! Path resolution: by default we look for the binary at
//! `target/<profile>/grex(.exe)?` relative to the workspace root, falling back
//! to the `GREX_BIN` environment variable if set. CI workflows can override
//! the default by exporting `GREX_BIN=/abs/path/to/grex` after `cargo build`.
//!
//! Timing: every run is wall-clock-timed via [`std::time::Instant`]; the
//! resulting `elapsed` field is surfaced on [`CliResult`] so tests can assert
//! runtime ceilings (e.g. cancellation tests need to bound the kill latency).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// Captured outcome of a single `grex` subprocess invocation.
#[derive(Debug, Clone)]
pub struct CliResult {
    /// Process exit code. `None` is mapped to `-1` (signal/aborted on Unix).
    pub exit_code: i32,
    /// Captured stdout, decoded as UTF-8 (lossy).
    pub stdout: String,
    /// Captured stderr, decoded as UTF-8 (lossy).
    pub stderr: String,
    /// Wall-clock time from spawn to wait-completion.
    pub elapsed: Duration,
}

impl CliResult {
    /// Returns `true` iff the process exited with code 0.
    pub fn is_success(&self) -> bool {
        self.exit_code == 0
    }
}

/// Runs the `grex` binary with `args` inside `cwd` and captures the result.
///
/// Path resolution:
/// 1. `GREX_BIN` env var, if set, is used verbatim.
/// 2. Otherwise the function looks for `target/debug/grex(.exe)?` then
///    `target/release/grex(.exe)?` relative to the cargo workspace root
///    (derived from `CARGO_MANIFEST_DIR`).
///
/// Errors only on spawn failure (binary missing, IO error). A non-zero exit
/// from `grex` itself is **not** an error here — it's encoded in
/// [`CliResult::exit_code`] for the caller to assert against.
pub fn run(args: &[&str], cwd: &Path) -> Result<CliResult> {
    let bin = locate_grex_bin().context("locating grex binary")?;
    let start = Instant::now();
    let output = Command::new(&bin)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("spawning {bin:?} in {cwd:?}"))?;
    let elapsed = start.elapsed();

    Ok(CliResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        elapsed,
    })
}

/// Resolves the path of the `grex` binary that this harness should drive.
fn locate_grex_bin() -> Result<PathBuf> {
    if let Ok(explicit) = std::env::var("GREX_BIN") {
        return Ok(PathBuf::from(explicit));
    }

    // CARGO_MANIFEST_DIR points at .../crates/real-smoke. Walk up two levels
    // to land on the workspace root, then probe target/{debug,release}.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .context("CARGO_MANIFEST_DIR not set; build via `cargo` or set GREX_BIN")?;
    let workspace_root = PathBuf::from(&manifest_dir)
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .with_context(|| format!("walking up from CARGO_MANIFEST_DIR={manifest_dir}"))?;

    let exe = if cfg!(windows) { "grex.exe" } else { "grex" };
    for profile in ["debug", "release"] {
        let candidate = workspace_root.join("target").join(profile).join(exe);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    anyhow::bail!(
        "grex binary not found under {}/target/{{debug,release}}/{exe}; \
         build it first or set GREX_BIN.",
        workspace_root.display()
    )
}
