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
//!
//! v1.3.1 fix-sweep B7: in addition to the stdout/stderr pin, the
//! second test below seeds a manifest whose semantic-warning path
//! (`emit_semantic_warnings`, in `manifest/append.rs`) renders the
//! event's op tag in the trace line. v1.3.0 used
//! `op = ?std::mem::discriminant(ev)` which rendered as the opaque
//! `Discriminant(N)`. v1.3.1 must render the snake_case tag via
//! `Event::op_name()` — `op=sync` / `op=update` etc. Operators
//! grepping trace output need the human-readable name.

use assert_cmd::prelude::*;
use serde_json::Value;
use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary")
}

/// Strip ANSI escape sequences from a slice so substring asserts work
/// regardless of whether `tracing_subscriber` decided the test sink is
/// TTY-like and emitted color codes around field names / values. Codes
/// take the form `\x1b[<params>m` (CSI ... m) — a small state machine
/// suffices for the bytes the default formatter emits.
fn strip_ansi(input: &[u8]) -> String {
    let mut out = Vec::with_capacity(input.len());
    let mut iter = input.iter().copied().peekable();
    while let Some(b) = iter.next() {
        if b == 0x1b {
            // ESC — consume the optional `[`, parameter bytes, then the
            // final letter (typically `m` for SGR). If we don't find a
            // bracket immediately, just drop the ESC and continue.
            if iter.peek() == Some(&b'[') {
                iter.next();
                while let Some(&p) = iter.peek() {
                    iter.next();
                    if p.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(b);
    }
    String::from_utf8_lossy(&out).into_owned()
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
    let stderr_text = strip_ansi(&out.stderr);
    assert!(
        stderr_text.contains("torn trailing line"),
        "test fixture failed to trigger the manifest-torn-line warn; stderr was:\n{stderr_text}",
    );

    // v1.3.1 fix-sweep B7 (cross-cut on the same fixture): regardless
    // of which warn path fired, a `Discriminant(` token MUST NOT appear
    // in trace output. v1.3.0 emitted that token from the
    // `emit_semantic_warnings` path; v1.3.1 routes through
    // `Event::op_name()` instead. Assert against both stdout and
    // stderr so a regression that re-routes back to stdout is also
    // caught here.
    let stdout_text = strip_ansi(&stdout);
    assert!(
        !stdout_text.contains("Discriminant("),
        "B7 regressed: `Discriminant(` token leaked into stdout — op name should be Display-formatted via Event::op_name().\nstdout:\n{stdout_text}",
    );
    assert!(
        !stderr_text.contains("Discriminant("),
        "B7 regressed: `Discriminant(` token in stderr trace line — op name should be Display-formatted via Event::op_name().\nstderr:\n{stderr_text}",
    );
}

/// v1.3.1 fix-sweep B7: drive the `emit_semantic_warnings` path that
/// previously emitted `op=Discriminant(N)` into the trace line. The
/// fixture has a `Sync` event for a pack id with no prior `Add`, which
/// trips the "manifest event references unknown pack id" warn that
/// carries the `op` field.
///
/// Asserts:
///   1. stdout is empty / parseable JSON (no tracing leak),
///   2. stderr carries the warn,
///   3. stderr (with ANSI codes stripped) renders the op tag as
///      `op=sync` (Display via `Event::op_name()`), NOT
///      `op=Discriminant(`,
///   4. cross-check: regardless of formatter shape, the literal
///      substring `Discriminant(` is absent from stderr.
#[test]
fn semantic_warn_renders_op_name_not_discriminant() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join(".grex/events.jsonl");
    fs::create_dir_all(manifest.parent().unwrap()).unwrap();

    // A Sync event for pack id "ghost" with no prior Add. The
    // semantic-warning sweep in `manifest/append.rs` will fire a
    // `tracing::warn!` carrying `op = %ev.op_name()`.
    let mut payload = String::new();
    payload.push_str(r#"{"op":"sync","ts":"2026-04-23T10:00:00Z","id":"ghost","sha":"deadbeef"}"#);
    payload.push('\n');
    fs::write(&manifest, payload).unwrap();

    let out = bin()
        .current_dir(dir.path())
        .env("RUST_LOG", "grex=debug,grex_core=debug")
        .args(["doctor", "--json"])
        .assert()
        .get_output()
        .clone();

    // 1. stdout is pure JSON (no tracing leak).
    let stdout = out.stdout.clone();
    let parsed: Value = serde_json::from_slice(&stdout).unwrap_or_else(|e| {
        panic!(
            "stdout is not pure JSON — tracing leak suspected.\nerror: {e}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&stdout),
            String::from_utf8_lossy(&out.stderr),
        )
    });
    assert!(parsed.get("report").is_some(), "doctor --json envelope missing `report`");

    // 2. stderr carries the unknown-id warn (proves the warn path fired).
    let stderr_text = strip_ansi(&out.stderr);
    assert!(
        stderr_text.contains("references unknown pack id"),
        "fixture failed to trigger the semantic warn; stderr was:\n{stderr_text}",
    );

    // 3. The op tag is Display-rendered via `Event::op_name()`. After
    // stripping ANSI, the default tracing-subscriber field formatter
    // renders `op = %v` for a `&str` value as `op=sync` (no quotes;
    // quotes only show up for `?v` Debug-formatting). Accept either
    // shape so a future formatter swap doesn't false-fail.
    let has_named_op = stderr_text.contains("op=sync") || stderr_text.contains("op=\"sync\"");
    assert!(
        has_named_op,
        "B7 regressed: stderr does not render `op=sync` — op name should be Display-formatted via Event::op_name().\nstderr:\n{stderr_text}",
    );

    // 4. The opaque `Discriminant(` form must not appear anywhere.
    assert!(
        !stderr_text.contains("Discriminant("),
        "B7 regressed: `Discriminant(` token in stderr — op name should be Display-formatted via Event::op_name().\nstderr:\n{stderr_text}",
    );
    let stdout_text = strip_ansi(&stdout);
    assert!(
        !stdout_text.contains("Discriminant("),
        "B7 regressed: `Discriminant(` token leaked into stdout.\nstdout:\n{stdout_text}",
    );
}
