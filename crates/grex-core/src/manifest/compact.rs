//! Manifest compaction: collapse the log down to a minimal set of `Add`
//! events (plus follow-up updates/syncs) that reproduces the current folded
//! state.
//!
//! Compaction is implemented as an **atomic rewrite**: we serialize the
//! compacted log to `grex.jsonl.tmp` then rename into place via
//! [`crate::fs::atomic_write`]. A crash mid-compaction leaves the original
//! file untouched.

use super::error::ManifestError;
use super::event::{Event, PackId, PackState, SCHEMA_VERSION};
use super::fold::fold;
use crate::fs::atomic_write;
use std::collections::HashMap;
use std::path::Path;

/// Rewrite `path` as a compacted log derived from the folded state.
///
/// The output is equivalent to the input log under [`fold`]: replaying the
/// compacted log yields the same `HashMap<PackId, PackState>`.
///
/// # Output shape
///
/// For each live pack, compaction emits:
/// 1. an `Add` event reproducing the original add fields,
/// 2. an `Update ref` event if `ref_spec` is set,
/// 3. a `Sync` event if `last_sync_sha` is set (stamped with `updated_at`).
///
/// Ordering among packs is by `PackId` for determinism.
///
/// # Errors
///
/// Returns [`ManifestError::Io`] or [`ManifestError::Serialize`] on failure.
/// On failure the original file is left in place.
pub fn compact(path: &Path) -> Result<(), ManifestError> {
    let events = super::append::read_all(path)?;
    let state = fold(events);
    let out = render(&state)?;
    atomic_write(path, out.as_bytes())?;
    Ok(())
}

/// Render the folded state back into JSONL bytes.
fn render(state: &HashMap<PackId, PackState>) -> Result<String, ManifestError> {
    let mut ids: Vec<&PackId> = state.keys().collect();
    ids.sort();
    let mut buf = String::new();
    for id in ids {
        let p = &state[id];
        let add = Event::Add {
            ts: p.added_at,
            id: p.id.clone(),
            url: p.url.clone(),
            path: p.path.clone(),
            pack_type: p.pack_type.clone(),
            schema_version: SCHEMA_VERSION.to_owned(),
        };
        buf.push_str(&serde_json::to_string(&add).map_err(ManifestError::Serialize)?);
        buf.push('\n');

        if let Some(r) = &p.ref_spec {
            let ev = Event::Update {
                ts: p.updated_at,
                id: p.id.clone(),
                field: "ref".into(),
                value: serde_json::Value::String(r.clone()),
            };
            buf.push_str(&serde_json::to_string(&ev).map_err(ManifestError::Serialize)?);
            buf.push('\n');
        }
        if let Some(sha) = &p.last_sync_sha {
            let ev = Event::Sync { ts: p.updated_at, id: p.id.clone(), sha: sha.clone() };
            buf.push_str(&serde_json::to_string(&ev).map_err(ManifestError::Serialize)?);
            buf.push('\n');
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::super::append::{append_event, read_all};
    use super::*;
    use chrono::{Duration, TimeZone, Utc};
    use tempfile::tempdir;

    fn t(n: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap() + Duration::seconds(n)
    }

    #[test]
    fn compact_preserves_folded_state() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        let events = vec![
            Event::Add {
                ts: t(0),
                id: "a".into(),
                url: "u".into(),
                path: "a".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
            Event::Update {
                ts: t(1),
                id: "a".into(),
                field: "ref".into(),
                value: serde_json::json!("v1"),
            },
            Event::Sync { ts: t(2), id: "a".into(), sha: "deadbeef".into() },
            Event::Add {
                ts: t(3),
                id: "b".into(),
                url: "ub".into(),
                path: "b".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
            Event::Rm { ts: t(4), id: "b".into() },
        ];
        for e in &events {
            append_event(&p, e).unwrap();
        }
        let before = fold(read_all(&p).unwrap());
        compact(&p).unwrap();
        let after = fold(read_all(&p).unwrap());
        assert_eq!(before, after);
        // b was removed — compacted log shouldn't resurrect it.
        assert!(!after.contains_key("b"));
        assert!(after.contains_key("a"));
    }

    #[test]
    fn compact_is_idempotent() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        append_event(
            &p,
            &Event::Add {
                ts: t(0),
                id: "a".into(),
                url: "u".into(),
                path: "a".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        compact(&p).unwrap();
        let first = std::fs::read(&p).unwrap();
        compact(&p).unwrap();
        let second = std::fs::read(&p).unwrap();
        assert_eq!(first, second);
    }
}
