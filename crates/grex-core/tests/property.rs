//! Property tests for fold + compaction invariants.

use chrono::{Duration, TimeZone, Utc};
use grex_core::manifest::{
    append_event, compact, fold, read_all, Event, PackId, PackState, SCHEMA_VERSION,
};
use grex_core::pack::{parse, PackManifest};
use grex_core::tree::build_graph;
use grex_core::{ClonedRepo, GitBackend, GitError, PackLoader, TreeError};
use proptest::prelude::*;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
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
        let p = dir.path().join(".grex/events.jsonl");
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
        let p = dir.path().join(".grex/events.jsonl");
        // `compact` rewrites via `atomic_write`, which does NOT create
        // parent dirs. Empty `events` would skip `append_event` (which
        // does create parents on demand), so seed `.grex/` up front.
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
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

// ---------------------------------------------------------------------------
// 2c-T-proptest — random DAG dichotomy.
//
// Generates random pack-tree shapes from a fixed pack-id pool and asserts
// `tree::build_graph` returns exactly one of:
//   * `Ok(PackGraph)` — no cycle on the URL ladder of the walked tree, OR
//   * `Err(TreeError::CycleDetected { .. })` — at least one cycle.
//
// Any other `TreeError` variant fails the property: by construction the
// generator emits manifests that pass child-path validation, name
// verification, and loader resolution, so a non-cycle error means the
// walker took a third path it must not.
//
// The walker must also TERMINATE (the proptest harness will hang the
// runner if it does not) and must NEVER PANIC (a panic in the worker
// poisons the proptest case and propagates as a test failure).
// ---------------------------------------------------------------------------

/// Pool of pack-ids the generator draws from. A small pool maximises the
/// odds that random sibling URLs collide with an ancestor URL on the
/// recursion stack and seed a cycle. Each id maps to URL
/// `https://e.com/<id>.git` and effective path `<id>`.
const PACK_IDS: &[&str] = &["a", "b", "c", "d", "e"];

/// In-memory `PackLoader` keyed by pack-id (last path segment) plus a
/// special root entry. Mirrors the helper used in
/// `tests/walker_parallel_stress.rs` but keeps lookup O(1) by basename
/// so the same manifest serves every recursion path that ends in
/// `<id>`.
struct InMemPackLoader {
    root_path: PathBuf,
    by_id: BTreeMap<String, PackManifest>,
    root: PackManifest,
}

impl PackLoader for InMemPackLoader {
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError> {
        if path == self.root_path {
            return Ok(self.root.clone());
        }
        let id = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| TreeError::ManifestNotFound(path.to_path_buf()))?;
        self.by_id.get(id).cloned().ok_or_else(|| TreeError::ManifestNotFound(path.to_path_buf()))
    }
}

/// No-op git backend: `build_graph` only invokes `head_sha`, and only
/// when `dest.join(".git")` exists on disk. The proptest never creates
/// any `.git/` directory, so the backend's methods are only reached if
/// the walker probes a synthetic `.git` we never wrote — in which case
/// returning a stub SHA keeps the dichotomy clean.
struct NoopGit;

impl GitBackend for NoopGit {
    fn name(&self) -> &'static str {
        "proptest-noop-git"
    }
    fn clone(
        &self,
        _url: &str,
        dest: &Path,
        _ref: Option<&str>,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<ClonedRepo, GitError> {
        Ok(ClonedRepo { path: dest.to_path_buf(), head_sha: "0".repeat(40) })
    }
    fn fetch(
        &self,
        _dest: &Path,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<(), GitError> {
        Ok(())
    }
    fn checkout(
        &self,
        _dest: &Path,
        _ref: &str,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<(), GitError> {
        Ok(())
    }
    fn head_sha(&self, _dest: &Path) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

/// Strategy: pick a vector of distinct pack-ids (preserving order) of
/// length up to `max_len`. Distinctness within a sibling list keeps the
/// child-path validator quiet — duplicate sibling paths would surface
/// `ManifestPathEscape` (NFC duplicate), a third outcome we want the
/// generator to avoid so the dichotomy is the only signal under test.
fn arb_distinct_children(max_len: usize) -> impl Strategy<Value = Vec<String>> {
    // Permutation-by-mask: for each id, decide include? Then keep order.
    // Truncate to `max_len` so branching factor stays bounded.
    prop::collection::vec(any::<bool>(), PACK_IDS.len()).prop_map(move |mask| {
        mask.iter()
            .enumerate()
            .filter_map(|(i, keep)| if *keep { Some(PACK_IDS[i].to_string()) } else { None })
            .take(max_len)
            .collect()
    })
}

/// Generate a `pack-id -> children` map covering every id in the pool.
/// Branching factor capped at 4 per the task spec.
fn arb_pack_defs() -> impl Strategy<Value = BTreeMap<String, Vec<String>>> {
    let strategies: Vec<_> = PACK_IDS.iter().map(|_| arb_distinct_children(4)).collect();
    strategies.prop_map(|kids_per_pack| {
        PACK_IDS.iter().map(|id| id.to_string()).zip(kids_per_pack).collect()
    })
}

/// Render a pack manifest YAML for pack-id `name` with the given child
/// list (each child resolves to URL `https://e.com/<id>.git` and the
/// default URL-derived effective path `<id>`).
fn render_manifest_yaml(name: &str, kids: &[String]) -> String {
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\n");
    if kids.is_empty() {
        yaml.push_str("children: []\n");
    } else {
        yaml.push_str("children:\n");
        for k in kids {
            yaml.push_str(&format!("  - url: https://e.com/{k}.git\n"));
        }
    }
    yaml
}

proptest! {
    // Cap cases low enough to keep total runtime well under 30s on the
    // default 64-thread proptest config. Each case is in-mem (no git, no
    // disk writes beyond a single tempdir per case) and walks at most
    // a depth-6 tree with branching ≤4, so 128 cases finishes in < 2s
    // locally and leaves headroom for shrink iterations.
    #![proptest_config(ProptestConfig {
        cases: 128,
        max_shrink_iters: 64,
        ..ProptestConfig::default()
    })]

    /// Random ManifestTree dichotomy: `build_graph` is `Ok(_)` (no
    /// cycle) XOR `Err(CycleDetected)` (cycle on the URL ladder).
    /// Termination + no-panic are implicit — proptest fails loudly on
    /// either.
    #[test]
    fn random_dag_dichotomy(
        defs in arb_pack_defs(),
        root_kids in arb_distinct_children(4),
    ) {
        // Materialise per-pack manifests via the production parser so
        // we never struct-literal across the `#[non_exhaustive]`
        // boundary on `PackManifest`.
        let mut by_id = BTreeMap::new();
        for (id, kids) in &defs {
            let m = parse(&render_manifest_yaml(id, kids))
                .expect("generated child manifest must parse");
            by_id.insert(id.clone(), m);
        }
        let root = parse(&render_manifest_yaml("root", &root_kids))
            .expect("generated root manifest must parse");

        let dir = tempdir().unwrap();
        let root_path = dir.path().to_path_buf();
        let loader = InMemPackLoader { root_path: root_path.clone(), by_id, root };
        let backend = NoopGit;

        let result = build_graph(&root_path, &backend, &loader, None);

        match result {
            Ok(_graph) => {} // success branch: no cycle on this random tree
            Err(TreeError::CycleDetected { chain }) => {
                // Cycle branch: chain MUST be non-empty and the last
                // element MUST recur earlier in the chain (basic
                // structural sanity on the reported cycle).
                prop_assert!(!chain.is_empty(), "cycle chain must be non-empty");
                let last = chain.last().unwrap();
                let recurs = chain.iter().take(chain.len() - 1).any(|s| s == last);
                prop_assert!(recurs, "last element must recur earlier in chain: {chain:?}");
            }
            Err(other) => {
                prop_assert!(
                    false,
                    "third outcome from build_graph: {other:?} (expected Ok or CycleDetected)",
                );
            }
        }
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
