//! Fix 1: `read_all` emits `tracing::warn!` for semantic anomalies.
//!
//! Two anomaly classes are covered:
//!
//!   * duplicate `Add` for the same pack id
//!   * `Update`/`Sync`/`Rm` referring to an id that never had a prior `Add`
//!
//! The fold layer collapses these to a valid state, so the warnings are
//! diagnostic only. We assert the messages are emitted using a manual
//! `tracing` subscriber that captures events into a shared buffer.

use chrono::{TimeZone, Utc};
use grex_core::manifest::{append_event, read_all, Event, SCHEMA_VERSION};
use std::sync::{Arc, Mutex};
use tempfile::tempdir;
use tracing::field::{Field, Visit};
use tracing::subscriber::with_default;
use tracing::{Event as TEvent, Level, Metadata, Subscriber};

// ---------------------------------------------------------------------------
// in-memory tracing subscriber
// ---------------------------------------------------------------------------

#[derive(Default)]
struct CapturedEvent {
    level: Option<Level>,
    message: String,
    fields: Vec<(String, String)>,
}

struct CollectingSubscriber {
    events: Arc<Mutex<Vec<CapturedEvent>>>,
}

struct FieldCollector<'a> {
    target: &'a mut CapturedEvent,
}

impl<'a> Visit for FieldCollector<'a> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let v = format!("{value:?}");
        if field.name() == "message" {
            // strip surrounding quotes from Debug of &str / String
            self.target.message = v.trim_matches('"').to_owned();
        } else {
            self.target.fields.push((field.name().to_owned(), v));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.target.message = value.to_owned();
        } else {
            self.target.fields.push((field.name().to_owned(), value.to_owned()));
        }
    }
}

impl Subscriber for CollectingSubscriber {
    fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}

    fn event(&self, event: &TEvent<'_>) {
        let mut captured =
            CapturedEvent { level: Some(*event.metadata().level()), ..Default::default() };
        let mut collector = FieldCollector { target: &mut captured };
        event.record(&mut collector);
        if let Ok(mut events) = self.events.lock() {
            events.push(captured);
        }
    }

    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

fn capture<F, R>(f: F) -> (R, Vec<CapturedEvent>)
where
    F: FnOnce() -> R,
{
    let events: Arc<Mutex<Vec<CapturedEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let sub = CollectingSubscriber { events: Arc::clone(&events) };
    let out = with_default(sub, f);
    let drained = events.lock().map(|g| g.clone()).unwrap_or_default();
    (out, drained)
}

impl Clone for CapturedEvent {
    fn clone(&self) -> Self {
        Self { level: self.level, message: self.message.clone(), fields: self.fields.clone() }
    }
}

// ---------------------------------------------------------------------------
// test helpers
// ---------------------------------------------------------------------------

fn ts(sec: i64) -> chrono::DateTime<chrono::Utc> {
    Utc.timestamp_opt(sec, 0).unwrap()
}

fn ev_add(id: &str, sec: i64) -> Event {
    Event::Add {
        ts: ts(sec),
        id: id.into(),
        url: "u".into(),
        path: id.into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    }
}

fn ev_update(id: &str, sec: i64) -> Event {
    Event::Update {
        ts: ts(sec),
        id: id.into(),
        field: "url".into(),
        value: serde_json::Value::String("new".into()),
    }
}

fn ev_rm(id: &str, sec: i64) -> Event {
    Event::Rm { ts: ts(sec), id: id.into() }
}

fn ev_sync(id: &str, sec: i64) -> Event {
    Event::Sync { ts: ts(sec), id: id.into(), sha: "deadbeef".into() }
}

// ---------------------------------------------------------------------------
// tests
// ---------------------------------------------------------------------------

#[test]
fn duplicate_add_warns() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    append_event(&p, &ev_add("pkg", 1)).unwrap();
    append_event(&p, &ev_add("pkg", 2)).unwrap();

    let (events, logs) = capture(|| read_all(&p).unwrap());
    assert_eq!(events.len(), 2, "both Add events are returned as-parsed");

    let warn_msgs: Vec<&str> =
        logs.iter().filter(|e| e.level == Some(Level::WARN)).map(|e| e.message.as_str()).collect();
    assert!(
        warn_msgs.iter().any(|m| m.contains("duplicate Add")),
        "expected duplicate-Add warning, got: {warn_msgs:?}"
    );
}

#[test]
fn orphan_update_warns() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    // No Add for "ghost" — just an Update.
    append_event(&p, &ev_update("ghost", 1)).unwrap();

    let (_events, logs) = capture(|| read_all(&p).unwrap());
    let warn_msgs: Vec<&str> =
        logs.iter().filter(|e| e.level == Some(Level::WARN)).map(|e| e.message.as_str()).collect();
    assert!(
        warn_msgs.iter().any(|m| m.contains("unknown pack id")),
        "expected orphan-Update warning, got: {warn_msgs:?}"
    );
}

#[test]
fn orphan_sync_warns() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    append_event(&p, &ev_sync("ghost", 1)).unwrap();

    let (_events, logs) = capture(|| read_all(&p).unwrap());
    assert!(
        logs.iter()
            .any(|e| { e.level == Some(Level::WARN) && e.message.contains("unknown pack id") }),
        "expected orphan-Sync warning"
    );
}

#[test]
fn orphan_rm_warns() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    append_event(&p, &ev_rm("ghost", 1)).unwrap();

    let (_events, logs) = capture(|| read_all(&p).unwrap());
    let warn_msgs: Vec<&str> =
        logs.iter().filter(|e| e.level == Some(Level::WARN)).map(|e| e.message.as_str()).collect();
    assert!(
        warn_msgs.iter().any(|m| m.contains("Rm for unknown pack id")),
        "expected orphan-Rm warning, got: {warn_msgs:?}"
    );
}

#[test]
fn well_formed_log_emits_no_semantic_warnings() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    append_event(&p, &ev_add("pkg", 1)).unwrap();
    append_event(&p, &ev_update("pkg", 2)).unwrap();
    append_event(&p, &ev_sync("pkg", 3)).unwrap();
    append_event(&p, &ev_rm("pkg", 4)).unwrap();

    let (events, logs) = capture(|| read_all(&p).unwrap());
    assert_eq!(events.len(), 4);

    // No warn-level events should be emitted for a clean log. (Heal/torn
    // warnings come from other code paths and never fire on a complete log.)
    let warns: Vec<&str> =
        logs.iter().filter(|e| e.level == Some(Level::WARN)).map(|e| e.message.as_str()).collect();
    assert!(warns.is_empty(), "expected no warnings, got: {warns:?}");
}
