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
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "lowercase")]
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
    /// Record a successful sync of the pack to a specific git SHA.
    Sync {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Resolved commit SHA.
        sha: String,
    },
}

impl Event {
    /// Return the pack id the event applies to.
    pub fn id(&self) -> &PackId {
        match self {
            Event::Add { id, .. }
            | Event::Update { id, .. }
            | Event::Rm { id, .. }
            | Event::Sync { id, .. } => id,
        }
    }

    /// Return the event timestamp.
    pub fn ts(&self) -> DateTime<Utc> {
        match self {
            Event::Add { ts, .. }
            | Event::Update { ts, .. }
            | Event::Rm { ts, .. }
            | Event::Sync { ts, .. } => *ts,
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
}
