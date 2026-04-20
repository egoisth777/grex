//! Event types for the manifest log.
//!
//! Events are JSON objects with a `"op"` discriminant. Unknown fields are
//! **intentionally accepted** (no `#[serde(deny_unknown_fields)]`) so that
//! older grex binaries can still read newer logs as long as the operation
//! type is known.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Logical identifier of a pack. Stable across updates, unique per workspace.
pub type PackId = String;

/// Current manifest schema version. Bumped whenever event shapes change
/// incompatibly.
pub const SCHEMA_VERSION: &str = "1";

/// One entry in the manifest log.
///
/// Serialized form uses a lowercase `"op"` tag:
/// ```json
/// {"op":"add","ts":"...","id":"...","url":"...","path":"...","type":"...","schema_version":"1"}
/// ```
///
/// # Action audit variants (PR E)
///
/// [`Event::ActionStarted`] is appended **before** the executor runs an
/// action. [`Event::ActionCompleted`] is appended **after** success;
/// [`Event::ActionHalted`] is appended **after** failure. A dangling
/// `ActionStarted` with no matching completed/halted peer is a crash
/// candidate — see [`crate::sync::scan_recovery`].
///
/// These variants are ignored by [`crate::manifest::fold::fold`] (they do
/// not mutate pack state) so the folded projection is unchanged from the
/// pre-PR-E schema; old readers decoding a log that contains them still
/// parse successfully because the `op` discriminants are known lowercase
/// tags with plain fields (unknown fields are tolerated per module docs).
#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Event {
    /// Register a new pack in the workspace.
    Add {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Upstream source URL (git remote, file path, etc).
        url: String,
        /// Workspace-relative checkout path.
        path: String,
        /// Pack flavor (e.g. `"declarative"`, `"imperative"`).
        #[serde(rename = "type")]
        pack_type: String,
        /// Schema version at time of write.
        schema_version: String,
    },
    /// Update a single field on an existing pack.
    ///
    /// `field` must be one of `"url"`, `"ref"`, `"path"`. Unknown field
    /// names are ignored by [`crate::manifest::fold::fold`] to keep forward
    /// compatibility.
    Update {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Field name being updated.
        field: String,
        /// New value (string or other JSON scalar).
        value: serde_json::Value,
    },
    /// Remove a pack from the workspace.
    Rm {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
    },
    /// Record a completed action step. Emitted on the success path of
    /// [`crate::sync::run`] along with [`Event::ActionCompleted`]; the
    /// `sha` field carries a short human summary of the step outcome
    /// (kept for backward-compat with M2 readers that folded only on this
    /// variant).
    Sync {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Resolved commit SHA or short action summary.
        sha: String,
    },
    /// An executor is **about to run** an action. Written before
    /// `executor.execute` so a crash mid-action leaves a discoverable
    /// trace. A dangling `ActionStarted` with no matching completed/halted
    /// peer signals a crashed run — see [`crate::sync::scan_recovery`].
    ActionStarted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier owning the action.
        pack: PackId,
        /// 0-based index into the pack's top-level `actions` vector.
        action_idx: usize,
        /// Short action kind tag (e.g. `"symlink"`, `"mkdir"`).
        action_name: String,
    },
    /// The executor returned `Ok`. Paired with a preceding
    /// [`Event::ActionStarted`]. `result_summary` is a short
    /// human-readable string (e.g. `"performed_change"`).
    ActionCompleted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier owning the action.
        pack: PackId,
        /// 0-based index into the pack's top-level `actions` vector.
        action_idx: usize,
        /// Short outcome summary tag.
        result_summary: String,
    },
    /// The executor returned `Err`. Paired with a preceding
    /// [`Event::ActionStarted`]. `error_summary` is the error's `Display`
    /// output truncated to a small limit so an audit trail line stays
    /// single-event-sized.
    ActionHalted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier owning the action.
        pack: PackId,
        /// 0-based index into the pack's top-level `actions` vector.
        action_idx: usize,
        /// Short action kind tag.
        action_name: String,
        /// Truncated error message (at most
        /// [`ACTION_ERROR_SUMMARY_MAX`] bytes).
        error_summary: String,
    },
}

/// Max bytes retained in [`Event::ActionHalted::error_summary`].
///
/// Truncation keeps one halt record on one JSONL line without
/// pathological blowup when an executor surfaces a multi-KB error
/// (e.g. captured stderr from an exec failure).
pub const ACTION_ERROR_SUMMARY_MAX: usize = 2048;

impl Event {
    /// Return the pack id the event applies to.
    ///
    /// Action-audit variants return the `pack` field; legacy variants
    /// return their `id`.
    pub fn id(&self) -> &PackId {
        match self {
            Event::Add { id, .. }
            | Event::Update { id, .. }
            | Event::Rm { id, .. }
            | Event::Sync { id, .. } => id,
            Event::ActionStarted { pack, .. }
            | Event::ActionCompleted { pack, .. }
            | Event::ActionHalted { pack, .. } => pack,
        }
    }

    /// Return the event timestamp.
    pub fn ts(&self) -> DateTime<Utc> {
        match self {
            Event::Add { ts, .. }
            | Event::Update { ts, .. }
            | Event::Rm { ts, .. }
            | Event::Sync { ts, .. }
            | Event::ActionStarted { ts, .. }
            | Event::ActionCompleted { ts, .. }
            | Event::ActionHalted { ts, .. } => *ts,
        }
    }
}

/// Current resolved state of a single pack.
///
/// Produced by folding the manifest log. **Not serialized** — this is an
/// in-memory projection; the lockfile has its own on-disk shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackState {
    /// Pack identifier.
    pub id: PackId,
    /// Upstream URL.
    pub url: String,
    /// Workspace-relative checkout path.
    pub path: String,
    /// Pack flavor.
    pub pack_type: String,
    /// Optional ref spec (branch, tag) — `None` until first `update ref`.
    pub ref_spec: Option<String>,
    /// Last synced SHA — `None` until first `sync`.
    pub last_sync_sha: Option<String>,
    /// Timestamp of the originating `add` event.
    pub added_at: DateTime<Utc>,
    /// Timestamp of the most recent event that touched this pack.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap()
    }

    #[test]
    fn add_roundtrip() {
        let e = Event::Add {
            ts: ts(),
            id: "warp-cfg".into(),
            url: "git@example:warp".into(),
            path: "warp-cfg".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        assert!(s.contains(r#""op":"add""#));
        assert!(s.contains(r#""type":"declarative""#));
    }

    #[test]
    fn update_roundtrip() {
        let e = Event::Update {
            ts: ts(),
            id: "warp-cfg".into(),
            field: "ref".into(),
            value: serde_json::json!("v0.2.0"),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn rm_roundtrip() {
        let e = Event::Rm { ts: ts(), id: "warp-cfg".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn sync_roundtrip() {
        let e = Event::Sync { ts: ts(), id: "warp-cfg".into(), sha: "abc123".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn unknown_fields_are_accepted() {
        // Forward-compat: a newer writer may add fields the old reader
        // doesn't know about. Parse must still succeed.
        let raw = r#"{"op":"rm","ts":"2026-04-19T10:00:00Z","id":"x","future_field":true}"#;
        let e: Event = serde_json::from_str(raw).unwrap();
        assert_eq!(e.id(), "x");
    }

    #[test]
    fn id_and_ts_accessors() {
        let e = Event::Sync { ts: ts(), id: "a".into(), sha: "s".into() };
        assert_eq!(e.id(), "a");
        assert_eq!(e.ts(), ts());
    }

    #[test]
    fn action_started_roundtrip() {
        let e = Event::ActionStarted {
            ts: ts(),
            pack: "warp".into(),
            action_idx: 3,
            action_name: "symlink".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"action_started""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn action_completed_roundtrip() {
        let e = Event::ActionCompleted {
            ts: ts(),
            pack: "warp".into(),
            action_idx: 1,
            result_summary: "performed_change".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"action_completed""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn action_halted_roundtrip() {
        let e = Event::ActionHalted {
            ts: ts(),
            pack: "warp".into(),
            action_idx: 2,
            action_name: "exec".into(),
            error_summary: "non-zero exit 3".into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"action_halted""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn legacy_lowercase_tags_still_parse() {
        // Historical writers used `rename_all = "lowercase"`. snake_case
        // and lowercase are identical for the single-word legacy tags, so
        // old logs must still decode.
        let raw = r#"{"op":"add","ts":"2026-04-19T10:00:00Z","id":"a","url":"u","path":"a","type":"declarative","schema_version":"1"}"#;
        let _: Event = serde_json::from_str(raw).unwrap();
        let raw = r#"{"op":"sync","ts":"2026-04-19T10:00:00Z","id":"a","sha":"deadbeef"}"#;
        let _: Event = serde_json::from_str(raw).unwrap();
    }
}
