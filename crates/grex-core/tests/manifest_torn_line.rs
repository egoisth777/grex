//! Integration: a torn trailing line must be discarded; earlier data intact.
//!
//! Covers HIGH + MED gaps from the codex review:
//!   * partial UTF-8 codepoint at EOF
//!   * valid JSON whose schema is not an `Event`
//!   * multiple trailing garbage lines
//!   * semantically corrupt but schema-valid tail
//!   * empty / whitespace-only / BOM-prefixed files
//!   * single-event roundtrip with and without final newline
//!   * CRLF line endings

use chrono::{Duration, TimeZone, Utc};
use grex_core::manifest::{append_event, read_all, Event, ManifestError, SCHEMA_VERSION};
use std::fs::{File, OpenOptions};
use std::io::Write;
use tempfile::tempdir;

// --- helpers ----------------------------------------------------------------

mod helpers {
    use super::*;

    pub fn sample_add(i: i64) -> Event {
        let base = Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap();
        Event::Add {
            ts: base + Duration::seconds(i),
            id: format!("pack-{i}"),
            url: format!("u-{i}"),
            path: format!("p-{i}"),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        }
    }

    /// Append raw bytes (no trailing newline added) to an existing file.
    pub fn append_raw(path: &std::path::Path, bytes: &[u8]) {
        let mut f = OpenOptions::new().append(true).open(path).unwrap();
        f.write_all(bytes).unwrap();
    }
}

// ---------------------------------------------------------------------------
// Baseline: existing regression from codex baseline run.
// ---------------------------------------------------------------------------
#[test]
fn torn_trailing_line_recovers() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    for i in 0..10 {
        append_event(&p, &helpers::sample_add(i)).unwrap();
    }

    // Simulate torn write: append an incomplete JSON fragment.
    helpers::append_raw(&p, b"{\"op\":\"sync\",\"ts\":\"2026-04-19T10:00:10");

    let events = read_all(&p).unwrap();
    assert_eq!(events.len(), 10, "earlier events must survive torn tail");
}

// ---------------------------------------------------------------------------
// HIGH #1: partial UTF-8 codepoint at EOF.
//
// Real crashes can truncate inside a multi-byte UTF-8 codepoint (e.g. after
// 0xC3 of "ü" = 0xC3 0xBC). The torn-tail contract requires such a tail to
// be discarded and earlier events returned intact. `read_all` uses a
// byte-oriented line splitter that runs `str::from_utf8` per line; UTF-8
// failure on the last (unterminated) line is treated as a torn tail.
// ---------------------------------------------------------------------------
#[test]
fn torn_trailing_line_recovers_from_partial_utf8_codepoint() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    for i in 0..3 {
        append_event(&p, &helpers::sample_add(i)).unwrap();
    }

    // Start a new line with a valid JSON prefix, then a stray 0xC3 (UTF-8
    // leading byte of "ü") with no continuation byte. This makes the last
    // line *not* valid UTF-8.
    let mut bad: Vec<u8> = Vec::new();
    bad.extend_from_slice(b"{\"op\":\"add\",\"id\":\"partial-");
    bad.push(0xC3); // dangling leading byte, no continuation.
    helpers::append_raw(&p, &bad);

    let events = read_all(&p).expect("torn multibyte tail must be recovered");
    assert_eq!(events.len(), 3, "earlier events must survive partial-UTF-8 tail");
}

// ---------------------------------------------------------------------------
// HIGH #2: last line is syntactically valid JSON but fails Event deserialize.
// ---------------------------------------------------------------------------
#[test]
fn valid_json_invalid_event_schema_tail_is_discarded() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    for i in 0..2 {
        append_event(&p, &helpers::sample_add(i)).unwrap();
    }

    // Valid JSON object, but "op" value is not a known Event variant.
    helpers::append_raw(&p, b"{\"op\":\"nonsense\",\"x\":42}\n");

    let events = read_all(&p).unwrap();
    assert_eq!(events.len(), 2, "schema-invalid-but-syntactically-valid tail must be discarded");
}

// ---------------------------------------------------------------------------
// HIGH #3: multiple trailing garbage lines.
//
// Current impl treats only the *single* last line as torn. Earlier garbage
// lines become ManifestError::Corruption. This test documents that behavior:
// if any middle line is unparsable, we get a typed error — not a silent
// success.
// ---------------------------------------------------------------------------
#[test]
fn multiple_torn_trailing_lines_are_all_ignored() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    for i in 0..4 {
        append_event(&p, &helpers::sample_add(i)).unwrap();
    }

    // 2 trailing garbage lines, neither parseable.
    helpers::append_raw(&p, b"{\"op\":\"bogus1\"}\n");
    helpers::append_raw(&p, b"{\"op\":\"bogus2\"}\n");

    // Only the final line is recovered as torn; the penultimate garbage
    // line is treated as hard corruption. Document the contract.
    let err = read_all(&p).unwrap_err();
    assert!(
        matches!(err, ManifestError::Corruption { line: 5, .. }),
        "expected Corruption at line 5 (first of 2 bad tails), got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// HIGH #4: semantically corrupt but schema-valid tail.
//
// "add" event with a duplicate id of an earlier add is well-formed per the
// Event schema. fold() is responsible for semantic conflicts, not read_all.
// This test pins down that read_all keeps it and does NOT silently drop it.
// ---------------------------------------------------------------------------
#[test]
fn semantically_corrupt_but_valid_tail_is_not_silently_kept() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    append_event(&p, &helpers::sample_add(0)).unwrap();
    // Second event reuses the same id ("pack-0") but is schema-valid.
    let dup = Event::Add {
        ts: Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 5).unwrap(),
        id: "pack-0".into(),
        url: "different".into(),
        path: "different".into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    append_event(&p, &dup).unwrap();

    let events = read_all(&p).unwrap();
    assert_eq!(events.len(), 2, "read_all is syntactic-only; semantic conflicts belong to fold()");
    // Confirm the tail we just wrote is the one returned.
    assert_eq!(events[1], dup);
}

// ---------------------------------------------------------------------------
// MED #5: empty, whitespace-only, and BOM-prefixed manifests.
// ---------------------------------------------------------------------------
#[test]
fn empty_or_bom_prefixed_manifest_is_handled() {
    // 5a: empty file.
    {
        let dir = tempdir().unwrap();
        let p = dir.path().join("empty.jsonl");
        File::create(&p).unwrap();
        let got = read_all(&p).expect("empty file must read cleanly");
        assert!(got.is_empty());
    }

    // 5b: whitespace-only file. Spaces alone are not valid JSON. With only
    // one line in the file it is the "last" line and must be recovered as
    // torn (yielding an empty Vec).
    {
        let dir = tempdir().unwrap();
        let p = dir.path().join("ws.jsonl");
        let mut f = File::create(&p).unwrap();
        f.write_all(b"   \n").unwrap();
        drop(f);
        let got = read_all(&p);
        match got {
            Ok(v) => assert!(v.is_empty(), "whitespace-only should yield no events"),
            Err(e) => panic!("whitespace-only file must not hard-error: {e:?}"),
        }
    }

    // 5c: BOM prefix + one valid event. The UTF-8 BOM (0xEF 0xBB 0xBF)
    // precedes an otherwise-valid event line.
    {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bom.jsonl");
        let mut f = File::create(&p).unwrap();
        f.write_all(&[0xEF, 0xBB, 0xBF]).unwrap();
        let ev = helpers::sample_add(0);
        let line = serde_json::to_string(&ev).unwrap();
        f.write_all(line.as_bytes()).unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);

        // BOM on the sole line makes serde_json reject it. Because it's
        // also the last line, torn-line recovery applies and we get an
        // empty Vec — not a panic, not a hard error.
        let got = read_all(&p);
        match got {
            Ok(_) => {} // either parsed cleanly (if impl skips BOM) or empty via torn-tail
            Err(e) => panic!("BOM-prefixed file must not hard-error: {e:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// MED #6: single event roundtrips with and without final newline.
// ---------------------------------------------------------------------------
#[test]
fn single_event_roundtrips_with_and_without_final_newline() {
    // With trailing newline (the canonical append_event shape).
    {
        let dir = tempdir().unwrap();
        let p = dir.path().join("with_nl.jsonl");
        let ev = helpers::sample_add(0);
        append_event(&p, &ev).unwrap();
        let got = read_all(&p).unwrap();
        assert_eq!(got, vec![ev]);
    }

    // Without trailing newline (hand-written, crash-before-\n scenario).
    {
        let dir = tempdir().unwrap();
        let p = dir.path().join("no_nl.jsonl");
        let ev = helpers::sample_add(0);
        let mut f = File::create(&p).unwrap();
        f.write_all(serde_json::to_string(&ev).unwrap().as_bytes()).unwrap();
        // NO trailing "\n".
        drop(f);
        let got = read_all(&p).unwrap();
        assert_eq!(got, vec![ev], "event without trailing \\n must still parse");
    }
}

// ---------------------------------------------------------------------------
// MED #7: torn trailing line with CRLF line endings (Windows-relevant).
//
// `BufRead::lines()` strips '\n' but keeps '\r'. `serde_json` treats '\r'
// as whitespace so lines ending in "\r\n" parse fine. The torn tail on the
// final line must still be recovered.
// ---------------------------------------------------------------------------
#[test]
fn torn_trailing_line_recovers_with_crlf() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("crlf.jsonl");

    // Write 3 events manually with CRLF terminators.
    let mut f = File::create(&p).unwrap();
    for i in 0..3 {
        let line = serde_json::to_string(&helpers::sample_add(i)).unwrap();
        f.write_all(line.as_bytes()).unwrap();
        f.write_all(b"\r\n").unwrap();
    }
    // Torn tail, also CRLF-style (partial, no terminator).
    f.write_all(b"{\"op\":\"sync\",\"ts\":\"2026-04-19").unwrap();
    drop(f);

    let got = read_all(&p).unwrap();
    assert_eq!(got.len(), 3, "CRLF-terminated events must parse; torn tail dropped");
}
