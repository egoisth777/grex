//! Property tests for fold + compaction invariants.

use chrono::{Duration, TimeZone, Utc};
use grex_core::manifest::{
    append_event, compact, fold, read_all, Event, PackId, PackState, SCHEMA_VERSION,
};
use proptest::prelude::*;
use std::collections::HashMap;
use tempfile::tempdir;

fn ts(n: i64) -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap() + Duration::seconds(n)
}

/// Weighted id strategy: ~50% collision-prone (a/b/c), ~50% random lowercase.
fn arb_id() -> impl Strategy<Value = String> {
    prop_oneof![
        1 => Just("a".to_string()),
        1 => Just("b".to_string()),
        1 => Just("c".to_string()),
        3 => "[a-z]{1,8}".prop_map(String::from),
    ]
}

/// Wider timestamp offset range, including negatives so equal and
/// out-of-order timestamps can appear inside an event stream.
fn arb_ts_offset() -> impl Strategy<Value = i64> {
    -500i64..5000
}

fn arb_event() -> impl Strategy<Value = Event> {
    let add = (arb_id(), arb_ts_offset()).prop_map(|(id, n)| Event::Add {
        ts: ts(n),
        id: id.clone(),
        url: format!("u://{id}"),
        path: id,
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    });
    let upd = (arb_id(), arb_ts_offset(), "[a-z]{1,5}").prop_map(|(id, n, v)| Event::Update {
        ts: ts(n),
        id,
        field: "ref".into(),
        value: serde_json::Value::String(v),
    });
    let rm = (arb_id(), arb_ts_offset()).prop_map(|(id, n)| Event::Rm { ts: ts(n), id });
    let sync = (arb_id(), arb_ts_offset(), "[a-f0-9]{6}").prop_map(|(id, n, sha)| Event::Sync {
        ts: ts(n),
        id,
        sha,
    });
    prop_oneof![add, upd, rm, sync]
}

/// Manual one-event-at-a-time accumulator that should match batched fold.
fn streamed_fold(events: &[Event]) -> HashMap<PackId, PackState> {
    let mut acc: HashMap<PackId, PackState> = HashMap::new();
    for ev in events {
        // Refold the union of prior accumulator-implied events + next event
        // by building a single-event batch chained onto the existing map.
        // We test equivalence via per-event fold composition: acc' = fold(acc_events + [ev]).
        // Since `fold` only takes events (not states), compose by round-tripping
        // through a replay vector.
        let replay = replay_events_from_state(&acc);
        let mut next = replay;
        next.push(ev.clone());
        acc = fold(next);
    }
    acc
}

/// Reconstruct a minimal event sequence that, when folded, yields the given
/// state. Used only to drive the streaming test — not a public API.
fn replay_events_from_state(state: &HashMap<PackId, PackState>) -> Vec<Event> {
    let mut out = Vec::with_capacity(state.len() * 3);
    // Stable order keeps the replay deterministic.
    let mut ids: Vec<&PackId> = state.keys().collect();
    ids.sort();
    for id in ids {
        let p = &state[id];
        out.push(Event::Add {
            ts: p.added_at,
            id: p.id.clone(),
            url: p.url.clone(),
            path: p.path.clone(),
            pack_type: p.pack_type.clone(),
            schema_version: SCHEMA_VERSION.into(),
        });
        if let Some(r) = &p.ref_spec {
            out.push(Event::Update {
                ts: p.updated_at,
                id: p.id.clone(),
                field: "ref".into(),
                value: serde_json::Value::String(r.clone()),
            });
        }
        if let Some(sha) = &p.last_sync_sha {
            out.push(Event::Sync { ts: p.updated_at, id: p.id.clone(), sha: sha.clone() });
        }
    }
    out
}

proptest! {
    /// Streaming fold (one event at a time, via replay) must equal batched fold.
    /// Catches ordering bugs, stateful leakage, or non-associative apply logic
    /// that a tautological `fold(x) == fold(x)` cannot detect.
    #[test]
    fn streamed_fold_matches_batch_fold(events in prop::collection::vec(arb_event(), 0..40)) {
        let batch = fold(events.clone());
        let streamed = streamed_fold(&events);
        prop_assert_eq!(batch, streamed);
    }

    /// Fold -> persist -> read-back -> fold must equal direct fold.
    /// Exercises serde round-trip and append/read pathways.
    #[test]
    fn fold_persist_then_read_matches_fold_direct(
        events in prop::collection::vec(arb_event(), 0..40),
    ) {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        for ev in &events {
            append_event(&p, ev).unwrap();
        }
        let direct = fold(events);
        let round_tripped = fold(read_all(&p).unwrap());
        prop_assert_eq!(direct, round_tripped);
    }

    #[test]
    fn compaction_preserves_fold(events in prop::collection::vec(arb_event(), 0..40)) {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        for ev in &events {
            append_event(&p, ev).unwrap();
        }
        let before = fold(read_all(&p).unwrap());
        compact(&p).unwrap();
        let after = fold(read_all(&p).unwrap());
        prop_assert_eq!(before, after);
    }

    #[test]
    fn rm_then_update_is_noop(n in 0i64..1000) {
        let events = vec![
            Event::Add {
                ts: ts(n),
                id: "a".into(),
                url: "u".into(),
                path: "a".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
            Event::Rm { ts: ts(n+1), id: "a".into() },
            Event::Update {
                ts: ts(n+2),
                id: "a".into(),
                field: "ref".into(),
                value: serde_json::json!("v"),
            },
        ];
        prop_assert!(fold(events).is_empty());
    }

    /// Applying the same update twice must equal applying it once.
    #[test]
    fn update_is_idempotent(
        id in arb_id(),
        n in 0i64..1000,
        v in "[a-z]{1,8}",
    ) {
        let add = Event::Add {
            ts: ts(n),
            id: id.clone(),
            url: "u".into(),
            path: id.clone(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let upd = Event::Update {
            ts: ts(n + 1),
            id: id.clone(),
            field: "ref".into(),
            value: serde_json::Value::String(v),
        };
        let once = fold(vec![add.clone(), upd.clone()]);
        let twice = fold(vec![add, upd.clone(), upd]);
        prop_assert_eq!(once, twice);
    }

    /// `rm` is absorbing: any later `update`/`sync` on that id is a no-op
    /// until a subsequent `add` resurrects it.
    #[test]
    fn rm_is_absorbing(
        id in arb_id(),
        n in 0i64..1000,
        v in "[a-z]{1,8}",
        sha in "[a-f0-9]{6}",
    ) {
        let add = Event::Add {
            ts: ts(n),
            id: id.clone(),
            url: "u".into(),
            path: id.clone(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let rm = Event::Rm { ts: ts(n + 1), id: id.clone() };
        let baseline = fold(vec![add.clone(), rm.clone()]);
        let with_noise = fold(vec![
            add,
            rm,
            Event::Update {
                ts: ts(n + 2),
                id: id.clone(),
                field: "ref".into(),
                value: serde_json::Value::String(v),
            },
            Event::Sync { ts: ts(n + 3), id, sha },
        ]);
        prop_assert_eq!(baseline, with_noise);
    }

    /// add-rm-add cycle: final state must reflect the second add's fields
    /// and its timestamp, not the first's.
    #[test]
    fn add_rm_add_cycle(
        id in arb_id(),
        n in 0i64..500,
        gap in 1i64..500,
    ) {
        let t0 = ts(n);
        let t1 = ts(n + gap);
        let t2 = ts(n + 2 * gap);
        let events = vec![
            Event::Add {
                ts: t0,
                id: id.clone(),
                url: "u1".into(),
                path: "p1".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
            Event::Rm { ts: t1, id: id.clone() },
            Event::Add {
                ts: t2,
                id: id.clone(),
                url: "u2".into(),
                path: "p2".into(),
                pack_type: "imperative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        ];
        let st = fold(events);
        let p = &st[&id];
        prop_assert_eq!(&p.url, "u2");
        prop_assert_eq!(&p.path, "p2");
        prop_assert_eq!(&p.pack_type, "imperative");
        prop_assert_eq!(p.added_at, t2);
        prop_assert_eq!(p.updated_at, t2);
        prop_assert_eq!(p.ref_spec.as_deref(), None);
        prop_assert_eq!(p.last_sync_sha.as_deref(), None);
    }
}

#[test]
fn add_rm_add_has_later_added_at() {
    let events = vec![
        Event::Add {
            ts: ts(0),
            id: "a".into(),
            url: "u".into(),
            path: "a".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        },
        Event::Rm { ts: ts(1), id: "a".into() },
        Event::Add {
            ts: ts(5),
            id: "a".into(),
            url: "u2".into(),
            path: "a".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        },
    ];
    let st = fold(events);
    assert_eq!(st["a"].added_at, ts(5));
    assert_eq!(st["a"].url, "u2");
}
