//! v1.3.1 (B8) — events schema v2 hard-cut regression suite.
//!
//! Pins the on-disk JSONL shape for the four schema-v2 variants:
//!
//! 1. `Event::ActionStarted` round-trip — JSON has `"id"` (not
//!    `"pack"`), `"schema_version":"2"`, op tag `"action_started"`.
//! 2. `Event::ActionCompleted` round-trip — same shape, op tag
//!    `"action_completed"`.
//! 3. `Event::ActionHalted` round-trip — same shape, op tag
//!    `"action_halted"`.
//! 4. `Event::DryRunWouldClone` round-trip — JSON has `"id"`, `"url"`,
//!    `"ref"` (Rust keyword exposed via `#[serde(rename = "ref")]`),
//!    `"schema_version":"2"`, op tag `"dry_run_would_clone"`.
//!
//! Bonus case: writer-supplied `id` value is preserved verbatim
//! through round-trip (the `id == folder_name` invariant is enforced
//! at the writer site, NOT at the serde-shape level — verifying only
//! that the field is faithfully reproduced).

use chrono::{TimeZone, Utc};
use grex_core::manifest::{Event, SCHEMA_VERSION};

fn ts() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 2, 10, 0, 0).unwrap()
}

#[test]
fn schema_version_const_is_two() {
    // Hard-cut sentinel: regressions that revert SCHEMA_VERSION back
    // to "1" would silently break every reader that pinned to v2.
    assert_eq!(SCHEMA_VERSION, "2");
}

#[test]
fn action_started_uses_id_not_pack_and_carries_schema_version() {
    let e = Event::ActionStarted {
        ts: ts(),
        id: "warp-cfgs".into(),
        action_idx: 0,
        action_name: "mkdir".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    let json = serde_json::to_string(&e).expect("serialize");
    assert!(json.contains(r#""op":"action_started""#), "op tag missing in {json}");
    assert!(json.contains(r#""id":"warp-cfgs""#), "id field missing in {json}");
    assert!(
        !json.contains(r#""pack":"warp-cfgs""#),
        "v2 must not emit legacy `pack` field, got {json}"
    );
    assert!(
        json.contains(r#""schema_version":"2""#),
        "schema_version field missing or wrong in {json}"
    );
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, e);
}

#[test]
fn action_completed_uses_id_not_pack_and_carries_schema_version() {
    let e = Event::ActionCompleted {
        ts: ts(),
        id: "warp-cfgs".into(),
        action_idx: 1,
        result_summary: "performed_change".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    let json = serde_json::to_string(&e).expect("serialize");
    assert!(json.contains(r#""op":"action_completed""#));
    assert!(json.contains(r#""id":"warp-cfgs""#));
    assert!(!json.contains(r#""pack":"warp-cfgs""#));
    assert!(json.contains(r#""schema_version":"2""#));
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, e);
}

#[test]
fn action_halted_uses_id_not_pack_and_carries_schema_version() {
    let e = Event::ActionHalted {
        ts: ts(),
        id: "warp-cfgs".into(),
        action_idx: 2,
        action_name: "exec".into(),
        error_summary: "non-zero exit 3".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    let json = serde_json::to_string(&e).expect("serialize");
    assert!(json.contains(r#""op":"action_halted""#));
    assert!(json.contains(r#""id":"warp-cfgs""#));
    assert!(!json.contains(r#""pack":"warp-cfgs""#));
    assert!(json.contains(r#""schema_version":"2""#));
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, e);
}

#[test]
fn dry_run_would_clone_round_trips_with_ref_keyword() {
    let e = Event::DryRunWouldClone {
        ts: ts(),
        id: "warp-cfgs".into(),
        url: "https://example.com/warp-cfgs.git".into(),
        ref_: Some("main".into()),
        schema_version: SCHEMA_VERSION.into(),
    };
    let json = serde_json::to_string(&e).expect("serialize");
    assert!(json.contains(r#""op":"dry_run_would_clone""#));
    assert!(json.contains(r#""id":"warp-cfgs""#));
    assert!(
        json.contains(r#""ref":"main""#),
        "Rust keyword `ref` must serialize as `ref` (via #[serde(rename)]) in {json}"
    );
    assert!(json.contains(r#""url":"https://example.com/warp-cfgs.git""#));
    assert!(json.contains(r#""schema_version":"2""#));
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back, e);
}

#[test]
fn id_value_preserved_verbatim_through_round_trip() {
    // Writer-supplied id is reproduced byte-for-byte. The runtime
    // invariant `id = folder_name = repo_name` is enforced at the
    // writer site (sync.rs / walker.rs), NOT at the serde-shape
    // level — this test covers only the serde fidelity contract.
    let writer_id = "some-arbitrary-folder-name-1234";
    let e = Event::ActionStarted {
        ts: ts(),
        id: writer_id.into(),
        action_idx: 7,
        action_name: "noop".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    let json = serde_json::to_string(&e).expect("serialize");
    assert!(json.contains(&format!(r#""id":"{writer_id}""#)));
    let back: Event = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.id().as_str(), writer_id);
}
