//! Integration: append events, read back, assert full-field equality.

use chrono::{Duration, TimeZone, Timelike, Utc};
use grex_core::manifest::{append_event, fold, read_all, Event, SCHEMA_VERSION};
use tempfile::tempdir;

#[test]
fn all_fields_verified_after_roundtrip() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    let base = Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap();

    // Build the full set of events we want to round-trip.
    let mut written: Vec<Event> = Vec::with_capacity(100);
    for i in 0..100 {
        let ev = Event::Add {
            ts: base + Duration::seconds(i),
            id: format!("pack-{i}"),
            url: format!("url://pack-{i}"),
            path: format!("path/to/pack-{i}"),
            pack_type: if i % 2 == 0 { "declarative" } else { "imperative" }.into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        append_event(&p, &ev).unwrap();
        written.push(ev);
    }

    let back = read_all(&p).unwrap();
    assert_eq!(back.len(), 100);

    // Every field on every event must survive the round-trip.
    for (orig, got) in written.iter().zip(back.iter()) {
        assert_eq!(orig, got, "full Event equality must hold");
        // Spot-check the specific fields the gap report called out.
        if let (
            Event::Add { ts: ts_o, path: path_o, pack_type: pt_o, schema_version: sv_o, .. },
            Event::Add { ts: ts_g, path: path_g, pack_type: pt_g, schema_version: sv_g, .. },
        ) = (orig, got)
        {
            assert_eq!(ts_o, ts_g);
            assert_eq!(path_o, path_g);
            assert_eq!(pt_o, pt_g);
            assert_eq!(sv_o, sv_g);
        } else {
            panic!("expected Event::Add");
        }
    }

    // And the folded state still looks right.
    let state = fold(back);
    assert_eq!(state.len(), 100);
    for i in 0..100 {
        let id = format!("pack-{i}");
        let s = state.get(&id).expect("pack present");
        assert_eq!(s.url, format!("url://{id}"));
        assert_eq!(s.path, format!("path/to/{id}"));
    }
}

#[test]
fn all_event_variants_roundtrip() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    let ts = Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap();

    let events = vec![
        Event::Add {
            ts,
            id: "warp-cfg".into(),
            url: "git@example:warp".into(),
            path: "warp-cfg".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        },
        Event::Update {
            ts: ts + Duration::seconds(1),
            id: "warp-cfg".into(),
            field: "ref".into(),
            value: serde_json::json!("v0.2.0"),
        },
        Event::Sync {
            ts: ts + Duration::seconds(2),
            id: "warp-cfg".into(),
            sha: "abc123def456".into(),
        },
        Event::Rm { ts: ts + Duration::seconds(3), id: "warp-cfg".into() },
    ];

    for ev in &events {
        append_event(&p, ev).unwrap();
    }

    let back = read_all(&p).unwrap();
    assert_eq!(back.len(), events.len());
    for (orig, got) in events.iter().zip(back.iter()) {
        assert_eq!(orig, got, "variant round-trip must preserve full equality");
    }
}

#[test]
fn timestamp_precision_preserved() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    // Sub-second precision: RFC3339 serialization must not round this off.
    let ts = Utc
        .with_ymd_and_hms(2026, 4, 19, 10, 0, 0)
        .unwrap()
        .with_nanosecond(123_456_789)
        .expect("valid nanos");

    let ev = Event::Add {
        ts,
        id: "precision".into(),
        url: "url://precision".into(),
        path: "precision".into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    append_event(&p, &ev).unwrap();

    let back = read_all(&p).unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0], ev, "exact event equality including timestamp");

    // Spell out the precision check so a regression is unambiguous.
    let got_ts = back[0].ts();
    assert_eq!(got_ts, ts);
    assert_eq!(got_ts.nanosecond(), 123_456_789);
}
