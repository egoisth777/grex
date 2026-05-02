//! v1.3.0 — JSON envelope dual-emit (`workspace` + `pack`) coverage.
//!
//! Asserts the operator-facing JSON wire contract from
//! `openspec/changes/feat-v1.3.0-cli-rename-freeze/proposal.md` §Acceptance
//! item 4: `grex ls --json` and `grex doctor --json` envelopes MUST carry
//! BOTH `workspace` and `pack` keys with identical values, with
//! `workspace` appearing FIRST in the serialised byte stream (preserves
//! diff-friendly stability for existing consumers; `pack` is the
//! additive forward-compat key).
//!
//! Order is asserted by walking the raw stdout text rather than the
//! parsed `serde_json::Value` (the `Map` reorders keys
//! alphabetically). This catches a regression where the construction
//! site ever reorders the literal `serde_json::json!({...})` body.

use assert_cmd::prelude::*;
use serde_json::Value;
use std::process::Command;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("grex").expect("grex binary built")
}

/// Locate `key` (as a JSON-encoded `"key"` token) in `body` and return
/// its byte offset. Used to assert relative ordering of two top-level
/// keys without depending on a full streaming parser.
fn key_offset(body: &str, key: &str) -> Option<usize> {
    let needle = format!("\"{key}\"");
    body.find(&needle)
}

#[test]
fn ls_envelope_contains_both_workspace_and_pack() {
    // `grex ls --json` against a tempdir succeeds (read-only walk over
    // an absent manifest yields a structured error envelope, but the
    // dual-emit case requires a real manifest at the root). Seed a
    // minimal `.grex/pack.yaml` so the success path runs and emits the
    // envelope.
    let tmp = TempDir::new().expect("tempdir");
    std::fs::create_dir_all(tmp.path().join(".grex")).unwrap();
    std::fs::write(
        tmp.path().join(".grex/pack.yaml"),
        "schema_version: \"1\"\nname: tmp\ntype: meta\n",
    )
    .unwrap();

    let out = bin()
        .current_dir(tmp.path())
        .args(["--json", "ls"])
        .assert()
        .success()
        .get_output()
        .clone();
    let stdout = String::from_utf8(out.stdout).expect("stdout utf-8");

    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("ls --json must produce valid JSON: {e}\n---\n{stdout}\n---"));

    let workspace = v
        .get("workspace")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("ls envelope MUST contain `workspace` key; got:\n{stdout}"));
    let pack = v.get("pack").and_then(Value::as_str).unwrap_or_else(|| {
        panic!("ls envelope MUST contain `pack` key (v1.3.0 dual-emit); got:\n{stdout}")
    });
    assert_eq!(
        workspace, pack,
        "v1.3.0 contract: `workspace` and `pack` MUST carry identical values; got workspace={workspace:?}, pack={pack:?}"
    );

    // Order assertion: byte-stable JSON output with `workspace` BEFORE
    // `pack`. Walks the raw text (not the parsed Map, which reorders).
    let ws_offset = key_offset(&stdout, "workspace").expect("workspace key in raw output");
    let pack_offset = key_offset(&stdout, "pack").expect("pack key in raw output");
    assert!(
        ws_offset < pack_offset,
        "v1.3.0 contract: `workspace` MUST appear before `pack` in JSON output (diff-friendly stability); ws={ws_offset}, pack={pack_offset}\n{stdout}"
    );
}

#[test]
fn doctor_envelope_contains_both_workspace_and_pack() {
    // `grex doctor --json` is content-tolerant: it walks the cwd's
    // workspace and emits a findings array. The dual-emit contract
    // applies to the top-level envelope, so a tempdir cwd suffices —
    // doctor returns a (possibly empty) report with the workspace +
    // pack envelope keys.
    let tmp = TempDir::new().expect("tempdir");
    let out = bin()
        .current_dir(tmp.path())
        .args(["doctor", "--json"])
        .output()
        .expect("spawn grex doctor --json");
    let stdout = String::from_utf8(out.stdout).expect("stdout utf-8");

    let v: Value = serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!("doctor --json must produce valid JSON: {e}\n---\n{stdout}\n---")
    });

    let workspace = v.get("workspace").and_then(Value::as_str).unwrap_or_else(|| {
        panic!("doctor envelope MUST contain `workspace` key (v1.3.0 dual-emit); got:\n{stdout}")
    });
    let pack = v.get("pack").and_then(Value::as_str).unwrap_or_else(|| {
        panic!("doctor envelope MUST contain `pack` key (v1.3.0 dual-emit); got:\n{stdout}")
    });
    assert_eq!(
        workspace, pack,
        "v1.3.0 contract: doctor `workspace` and `pack` MUST carry identical values; got workspace={workspace:?}, pack={pack:?}"
    );

    // Order assertion: `workspace` first, `pack` second in raw output.
    let ws_offset = key_offset(&stdout, "workspace").expect("workspace key in raw output");
    let pack_offset = key_offset(&stdout, "pack").expect("pack key in raw output");
    assert!(
        ws_offset < pack_offset,
        "v1.3.0 contract: doctor `workspace` MUST appear before `pack` in JSON output; ws={ws_offset}, pack={pack_offset}\n{stdout}"
    );
}
