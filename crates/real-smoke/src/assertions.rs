//! Filesystem and string-shape assertions used by the regression tests.
//!
//! Every helper returns `anyhow::Result<_>` so failures bubble up through
//! `?` inside `#[test]` bodies; the regression crate (W2c) decides whether
//! to `unwrap()` (hard fail) or downgrade to a warning.
//!
//! Naming convention: `assert_*` for boolean-shape checks (return `Result<()>`),
//! `parse_*` / `read_*` for value-extracting helpers.

use anyhow::{anyhow, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::grex_cli::CliResult;

// -- stdout / stderr shape ---------------------------------------------------

/// Parses `result.stdout` as a single JSON document. Used for `grex --json`
/// output. Returns the parsed [`serde_json::Value`] on success; on parse
/// failure, the error includes the first 256 characters of stdout for
/// debugging.
pub fn assert_stdout_is_valid_json(result: &CliResult) -> Result<Value> {
    serde_json::from_str(&result.stdout).with_context(|| {
        let preview: String = result.stdout.chars().take(256).collect();
        format!("stdout was not valid JSON; first 256 chars: {preview:?}")
    })
}

/// Asserts that `result.stdout` contains no `WARN ` / `warning:` markers.
///
/// Rationale: warnings belong on stderr; leaking them to stdout breaks
/// machine-readable contracts (e.g. `grex ls --json | jq`).
pub fn assert_no_warn_on_stdout(result: &CliResult) -> Result<()> {
    let lower = result.stdout.to_ascii_lowercase();
    if lower.contains("warn ") || lower.contains("warning:") {
        return Err(anyhow!(
            "stdout contained a warning marker (must go to stderr instead); \
             stdout preview: {:?}",
            result.stdout.chars().take(256).collect::<String>()
        ));
    }
    Ok(())
}

// -- workspace filesystem invariants -----------------------------------------

/// Asserts that the legacy lockfile location (`<workspace>/.grex-lock`) does
/// **not** exist. The current contract puts all lock state under
/// `<workspace>/.grex/` (see `inst/lockfile.md`), so a stray
/// `.grex-lock` at the workspace root is a regression.
pub fn assert_lockfile_under_grex_dir(workspace: &Path) -> Result<()> {
    let stray = workspace.join(".grex-lock");
    if stray.exists() {
        return Err(anyhow!(
            "found legacy lockfile at {} — must live under .grex/ instead",
            stray.display()
        ));
    }
    Ok(())
}

/// Asserts that `<workspace>/.gitignore` matches the given SHA-256 (lower-case
/// hex). Used to prove that grex commands don't mutate the user's
/// `.gitignore` mid-flight.
pub fn assert_gitignore_unchanged(workspace: &Path, expected_sha256: &str) -> Result<()> {
    let path = workspace.join(".gitignore");
    let actual = sha256_of_file(&path).with_context(|| format!("hashing {}", path.display()))?;
    if !actual.eq_ignore_ascii_case(expected_sha256) {
        return Err(anyhow!(
            ".gitignore at {} changed: expected sha256={}, got sha256={}",
            path.display(),
            expected_sha256,
            actual
        ));
    }
    Ok(())
}

/// Reads `path` and returns its SHA-256 as lower-case hex. Helper for callers
/// that want to record a fresh expected digest before mutating the workspace.
pub fn sha256_of_file(path: &Path) -> Result<String> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

// -- event log assertions ----------------------------------------------------

/// Asserts that the JSONL event log at `event_log` contains at least one
/// record where `op == <op>` and the named `<field>` is present (non-null).
///
/// The event log is JSONL: one JSON object per line. Lines that fail to
/// parse are skipped (loose contract) so partial flushes during a test don't
/// cause spurious failures.
pub fn assert_eventlog_record_carries_field(event_log: &Path, op: &str, field: &str) -> Result<()> {
    let raw = std::fs::read_to_string(event_log)
        .with_context(|| format!("reading event log {}", event_log.display()))?;

    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let matches_op = value.get("op").and_then(Value::as_str).is_some_and(|s| s == op);
        if !matches_op {
            continue;
        }
        if let Some(v) = value.get(field) {
            if !v.is_null() {
                return Ok(());
            }
        }
    }

    Err(anyhow!(
        "event log {} contained no record with op={op:?} carrying non-null field {field:?}",
        event_log.display()
    ))
}
