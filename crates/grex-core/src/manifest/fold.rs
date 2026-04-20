//! Fold an event stream into the current pack state.
//!
//! Folding is **deterministic** and **total**: the same sequence of events
//! always produces the same state, and every event type is handled (unknown
//! update fields are a no-op by design).

use super::event::{Event, PackId, PackState};
use std::collections::HashMap;

/// Fold an iterator of [`Event`]s into a map of live pack states.
///
/// Events are applied in iteration order. `rm` removes the pack entirely;
/// a subsequent `add` with the same id creates a fresh entry.
pub fn fold<I>(events: I) -> HashMap<PackId, PackState>
where
    I: IntoIterator<Item = Event>,
{
    let mut state: HashMap<PackId, PackState> = HashMap::new();
    for event in events {
        apply(&mut state, event);
    }
    state
}

/// Apply one event to the in-progress state map.
fn apply(state: &mut HashMap<PackId, PackState>, event: Event) {
    match event {
        Event::Add { ts, id, url, path, pack_type, schema_version: _ } => {
            state.insert(
                id.clone(),
                PackState {
                    id,
                    url,
                    path,
                    pack_type,
                    ref_spec: None,
                    last_sync_sha: None,
                    added_at: ts,
                    updated_at: ts,
                },
            );
        }
        Event::Update { ts, id, field, value } => apply_update(state, &id, ts, &field, &value),
        Event::Rm { id, .. } => {
            state.remove(&id);
        }
        Event::Sync { ts, id, sha } => {
            if let Some(p) = state.get_mut(&id) {
                p.last_sync_sha = Some(sha);
                p.updated_at = ts;
            }
        }
        // Action-audit variants are ignored by the fold: they carry
        // crash-recovery context but do not mutate pack state. See
        // [`Event::ActionStarted`].
        Event::ActionStarted { .. }
        | Event::ActionCompleted { .. }
        | Event::ActionHalted { .. } => {}
    }
}

/// Apply an `Update` event: mutate the named field in place, bump
/// `updated_at`. Unknown fields are logged and ignored (forward-compat).
fn apply_update(
    state: &mut HashMap<PackId, PackState>,
    id: &PackId,
    ts: chrono::DateTime<chrono::Utc>,
    field: &str,
    value: &serde_json::Value,
) {
    let Some(p) = state.get_mut(id) else { return };
    match field {
        "url" => {
            if let Some(s) = value.as_str() {
                p.url = s.to_owned();
            }
        }
        "ref" => {
            p.ref_spec = value.as_str().map(str::to_owned);
        }
        "path" => {
            if let Some(s) = value.as_str() {
                p.path = s.to_owned();
            }
        }
        other => {
            tracing::warn!(field = other, "unknown update field, ignoring");
        }
    }
    p.updated_at = ts;
}

#[cfg(test)]
mod tests {
    use super::super::event::SCHEMA_VERSION;
    use super::*;
    use chrono::{Duration, TimeZone, Utc};

    fn t(n: i64) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap() + Duration::seconds(n)
    }

    fn add(id: &str, ts_offset: i64) -> Event {
        Event::Add {
            ts: t(ts_offset),
            id: id.into(),
            url: format!("url://{id}"),
            path: id.into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        }
    }

    #[test]
    fn add_then_rm_leaves_empty_state() {
        let events = vec![add("a", 0), Event::Rm { ts: t(1), id: "a".into() }];
        assert!(fold(events).is_empty());
    }

    #[test]
    fn update_applies_field() {
        let events = vec![
            add("a", 0),
            Event::Update {
                ts: t(1),
                id: "a".into(),
                field: "ref".into(),
                value: serde_json::json!("v1"),
            },
        ];
        let st = fold(events);
        assert_eq!(st["a"].ref_spec.as_deref(), Some("v1"));
        assert_eq!(st["a"].updated_at, t(1));
    }

    #[test]
    fn update_path_and_url() {
        let events = vec![
            add("a", 0),
            Event::Update {
                ts: t(1),
                id: "a".into(),
                field: "url".into(),
                value: serde_json::json!("git://new"),
            },
            Event::Update {
                ts: t(2),
                id: "a".into(),
                field: "path".into(),
                value: serde_json::json!("new-path"),
            },
        ];
        let st = fold(events);
        assert_eq!(st["a"].url, "git://new");
        assert_eq!(st["a"].path, "new-path");
    }

    #[test]
    fn sync_records_sha() {
        let events =
            vec![add("a", 0), Event::Sync { ts: t(1), id: "a".into(), sha: "deadbeef".into() }];
        let st = fold(events);
        assert_eq!(st["a"].last_sync_sha.as_deref(), Some("deadbeef"));
    }

    #[test]
    fn rm_of_unknown_id_is_noop() {
        let events = vec![Event::Rm { ts: t(0), id: "nope".into() }];
        assert!(fold(events).is_empty());
    }

    #[test]
    fn update_of_unknown_id_is_noop() {
        let events = vec![Event::Update {
            ts: t(0),
            id: "nope".into(),
            field: "ref".into(),
            value: serde_json::json!("x"),
        }];
        assert!(fold(events).is_empty());
    }

    #[test]
    fn unknown_update_field_ignored() {
        let events = vec![
            add("a", 0),
            Event::Update {
                ts: t(1),
                id: "a".into(),
                field: "bogus".into(),
                value: serde_json::json!("x"),
            },
        ];
        let st = fold(events);
        // Known fields remain default; updated_at still advances.
        assert_eq!(st["a"].ref_spec, None);
        assert_eq!(st["a"].updated_at, t(1));
    }

    #[test]
    fn events_are_deterministic() {
        let events = vec![
            add("a", 0),
            add("b", 1),
            Event::Update {
                ts: t(2),
                id: "a".into(),
                field: "ref".into(),
                value: serde_json::json!("v1"),
            },
            Event::Rm { ts: t(3), id: "b".into() },
        ];
        let s1 = fold(events.clone());
        let s2 = fold(events);
        assert_eq!(s1, s2);
    }

    #[test]
    fn add_rm_add_resets_added_at() {
        let events = vec![add("a", 0), Event::Rm { ts: t(1), id: "a".into() }, add("a", 2)];
        let st = fold(events);
        assert_eq!(st["a"].added_at, t(2));
    }
}
