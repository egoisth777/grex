//! Round-2 fix-sweep regression: in non-serve verbs the tracing
//! subscriber MUST pin its writer to stderr. The default
//! `tracing_subscriber::fmt()` writer is `io::stdout`, which would
//! interleave any `tracing::warn!` line with the JSON envelope a
//! `--json` verb is producing on stdout. The serve branch already
//! pins to stderr (`crates/grex-mcp/tests/stdout_discipline.rs`); this
//! test pins the discipline for the rest of the CLI.
//!
//! Strategy: seed a workspace whose `.grex/events.jsonl` ends with a torn
//! (incomplete) trailing line. The M3 manifest reader recovers from
//! this by emitting a `tracing::warn!` ("discarding torn trailing
//! line in manifest") and returning the prefix events. Running
//! `grex doctor --json` against that workspace exercises the warn
//! path during `check_manifest_schema` AND emits a structured JSON
//! envelope to stdout. If the writer were stdout, the warn would
//! land in the same byte stream as the JSON and `serde_json::from_str`
//! would reject the captured stdout.

use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary")
}

#[test]
fn doctor_json_stdout_stays_pure_when_tracing_warn_fires() {
    let dir = tempfile::tempdir().unwrap();

    // Seed `.grex/events.jsonl` with one valid Add event followed by a
    // torn trailing line (no terminating newline, truncated JSON). The
    // M3 reader recovers from this by emitting a `tracing::warn!`.
    let manifest = dir.path().join(".grex/events.jsonl");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    let mut payload = String::new();
    payload.push_str(
        r#"{"op":"add","ts":"2026-04-23T10:00:00Z","id":"a","url":"https://example/a","path":"a","type":"declarative","schema_version":"1"}"#,
    );
    payload.push('\n');
    // Truncated trailing line — opening brace + key, no closing brace.
    payload.push_str(r#"{"op":"add","ts":"2026-04-23T10:00:01Z","i"#);
    fs::write(&manifest, payload).unwrap();

    // Pack dir for `a` so the on-disk-drift check stays Ok.
    fs::create_dir_all(dir.path().join("a")).unwrap();

    // Force the tracing filter wide-open so the warn definitely fires
    // through the subscriber. If the writer were stdout, this is the
    // worst case for stdout pollution.
    let out = bin()
        .current_dir(dir.path())
        .env("RUST_LOG", "grex=debug,grex_core=debug")
        .args(["doctor", "--json"])
        .assert()
        .get_output()
        .clone();

    // Stdout MUST parse cleanly as a single JSON value with no extra
    // non-JSON lines. `from_slice` rejects trailing content too.
    let stdout = out.stdout.clone();
    let parsed: Value = serde_json::from_slice(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not pure JSON — tracing leak suspected.\nerror: {e}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&out.stderr),
        )
    });
    // v1.3.0: top-level envelope is `{workspace, pack, report: {findings, exit_code, ...}}`.
    let report =
        parsed.get("report").expect("v1.3.0: doctor --json must wrap inner shape under `report`");
    assert!(report.get("findings").is_some(), "doctor --json must emit a `report.findings` array");

    // Sanity: the warn DID fire — it must show up on stderr (proving
    // the test actually exercised the tracing path it claims to).
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("torn trailing line"),
        "test fixture failed to trigger the manifest-torn-line warn; stderr was:\n{stderr}",
    );
}
