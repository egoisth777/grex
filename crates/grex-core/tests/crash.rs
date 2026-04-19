//! Crash-injection tests: torn writes, interrupted atomic replaces, and
//! corruption-boundary coverage.

use chrono::{Duration, TimeZone, Utc};
use grex_core::manifest::{append_event, read_all, Event, ManifestError, SCHEMA_VERSION};
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use tempfile::tempdir;

fn base() -> chrono::DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap()
}

/// Write `n` Add events to `path`.
fn seed_n(path: &Path, n: usize) {
    for i in 0..n {
        append_event(
            path,
            &Event::Add {
                ts: base() + Duration::seconds(i as i64),
                id: format!("p{i}"),
                url: "u".into(),
                path: format!("p{i}"),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
    }
}

#[test]
fn truncated_last_event_recovers_99() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 100);

    // Truncate a few bytes off the end to corrupt the final line.
    let mut f = OpenOptions::new().read(true).write(true).open(&p).unwrap();
    let len = f.seek(SeekFrom::End(0)).unwrap();
    f.set_len(len - 10).unwrap();
    drop(f);

    let events = read_all(&p).unwrap();
    assert_eq!(events.len(), 99, "last (torn) event discarded");
}

#[test]
fn truncated_middle_event_corruption_error() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 100);

    // Scramble a byte somewhere in the middle.
    let mut data = Vec::new();
    OpenOptions::new().read(true).open(&p).unwrap().read_to_end(&mut data).unwrap();
    let mid = data.len() / 2;
    // Find a '{' around the middle to break.
    let brace = data[..mid].iter().rposition(|b| *b == b'{').unwrap_or(mid);
    data[brace] = b'X';
    std::fs::write(&p, &data).unwrap();

    match read_all(&p).unwrap_err() {
        ManifestError::Corruption { .. } => {}
        other => panic!("expected Corruption, got {other:?}"),
    }
}

#[test]
fn atomic_write_interruption_keeps_original() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("lock.jsonl");
    std::fs::write(&p, b"v1").unwrap();

    // Simulate crash: tmp file present but no rename.
    let tmp = {
        let mut s = p.as_os_str().to_owned();
        s.push(".tmp");
        std::path::PathBuf::from(s)
    };
    let mut f = OpenOptions::new().create(true).write(true).truncate(true).open(&tmp).unwrap();
    f.write_all(b"partial v2").unwrap();
    drop(f);

    assert_eq!(std::fs::read(&p).unwrap(), b"v1");
    // Next successful atomic_write supersedes the target. Under the
    // uniquified-temp design (Fix 3) a foreign `.tmp` leftover is left
    // strictly alone — each writer owns its own pid/nanos-suffixed temp,
    // so touching another writer's file would be incorrect.
    grex_core::fs::atomic_write(&p, b"v2").unwrap();
    assert_eq!(std::fs::read(&p).unwrap(), b"v2");
    assert!(tmp.exists(), "foreign legacy tmp is left for the caller to GC");
}

// --- HIGH #1: truncation byte-boundary sweep ----------------------------
//
// Seed 100 events, then truncate the last line by `k` content bytes for
// k ∈ {1,2,3,5,8,13,21,34,55}. Each offset lands somewhere inside the
// JSON payload of line 100 (mid-key, mid-value, mid-comma, mid-digit, …),
// guaranteeing a torn trailing line that must be discarded while the
// first 99 events stay intact.
//
// The trailing '\n' is stripped first (offset=0 in that space is the
// boundary case — line is still complete; offset>=1 cuts actual JSON).
fn run_truncation_sweep_for_offset(offset: u64) {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 100);

    // Measure the last line's content length (excluding trailing '\n')
    // so we never over-truncate into line 99.
    let bytes = std::fs::read(&p).unwrap();
    assert_eq!(*bytes.last().unwrap(), b'\n', "seed writes must end in \\n");
    let last_nl = bytes[..bytes.len() - 1]
        .iter()
        .rposition(|b| *b == b'\n')
        .expect("at least one earlier newline");
    let last_line_len = (bytes.len() - 1 - last_nl) as u64; // excludes final '\n'
    assert!(
        offset <= last_line_len,
        "offset {offset} exceeds last-line content length {last_line_len}"
    );

    // Drop the trailing '\n' (+1) and then `offset` bytes of JSON content.
    let f = OpenOptions::new().read(true).write(true).open(&p).unwrap();
    let total_len = bytes.len() as u64;
    f.set_len(total_len - 1 - offset).unwrap();
    drop(f);

    let events =
        read_all(&p).unwrap_or_else(|e| panic!("offset={offset} expected recovery, got {e:?}"));
    assert_eq!(
        events.len(),
        99,
        "offset={offset}: torn trailing line must be discarded, 99 prior events preserved"
    );
}

#[test]
fn truncation_byte_boundary_sweep_1() {
    run_truncation_sweep_for_offset(1);
}
#[test]
fn truncation_byte_boundary_sweep_2() {
    run_truncation_sweep_for_offset(2);
}
#[test]
fn truncation_byte_boundary_sweep_3() {
    run_truncation_sweep_for_offset(3);
}
#[test]
fn truncation_byte_boundary_sweep_5() {
    run_truncation_sweep_for_offset(5);
}
#[test]
fn truncation_byte_boundary_sweep_8() {
    run_truncation_sweep_for_offset(8);
}
#[test]
fn truncation_byte_boundary_sweep_13() {
    run_truncation_sweep_for_offset(13);
}
#[test]
fn truncation_byte_boundary_sweep_21() {
    run_truncation_sweep_for_offset(21);
}
#[test]
fn truncation_byte_boundary_sweep_34() {
    run_truncation_sweep_for_offset(34);
}
#[test]
fn truncation_byte_boundary_sweep_55() {
    run_truncation_sweep_for_offset(55);
}

// --- HIGH #2: middle-line corruption is a hard error --------------------

#[test]
fn truncation_in_middle_line_is_hard_error() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 100);

    // Locate the start of line 50 and overwrite its opening '{' with 'X'.
    let bytes = std::fs::read(&p).unwrap();
    let mut line_no = 1usize;
    let mut line_start = 0usize;
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'\n' {
            line_no += 1;
            if line_no == 50 {
                line_start = i + 1;
                break;
            }
        }
    }
    let mut corrupted = bytes.clone();
    // Replace the line's leading '{' with 'X' so it fails JSON parse.
    assert_eq!(corrupted[line_start], b'{');
    corrupted[line_start] = b'X';
    std::fs::write(&p, &corrupted).unwrap();

    match read_all(&p).unwrap_err() {
        ManifestError::Corruption { line, .. } => {
            assert_eq!(line, 50, "Corruption must point at the torn line");
        }
        other => panic!("expected ManifestError::Corruption, got {other:?}"),
    }
}

// --- HIGH #3: semantic corruption (wrong id) is silently accepted -------
//
// The manifest is an *intent log*; `read_all` only validates JSON shape,
// not cross-event semantics. A line that deserializes as a valid `Event`
// with a "wrong" id is accepted as-is. This test pins that current
// behaviour so a future src-change can tighten it intentionally.
//
// IMPL GAP: read_all has no semantic cross-check (e.g. "Update must
// reference an id introduced by a prior Add"). Fold does partial checks.
// Flag for src-change if stricter validation is desired.
#[test]
fn semantic_corruption_wrong_event_is_flagged_or_documented() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 5);

    // Append a valid Event::Add but with an id that collides / is nonsense
    // in the log's context (duplicate of p0, which fold would reject but
    // read_all does not).
    let colliding = Event::Add {
        ts: base() + Duration::seconds(999),
        id: "p0".into(), // duplicate of event 0
        url: "bogus".into(),
        path: "p0".into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    append_event(&p, &colliding).unwrap();

    // Current behaviour: accepted silently at the read_all layer.
    let events = read_all(&p).expect("read_all tolerates semantic collisions");
    assert_eq!(
        events.len(),
        6,
        "IMPL GAP: read_all accepts semantically-wrong events; \
         cross-event validation lives in fold, not read_all"
    );
    // Sanity: the wrong event is present.
    assert!(matches!(events.last(), Some(Event::Add { url, .. }) if url == "bogus"));
}

// --- HIGH #4: non-UTF8 bytes in the tail must not panic -----------------

#[test]
fn non_utf8_byte_injection_in_tail() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 10);

    // Append a line of invalid UTF-8 terminated with \n. Since it is the
    // last sequential line, torn-tail recovery applies: read_all must
    // discard it and return the 10 prior events intact (no panic, no error).
    let mut f = OpenOptions::new().append(true).open(&p).unwrap();
    f.write_all(&[0xFF, 0xFE, 0xFD, b'\n']).unwrap();
    drop(f);

    let events = read_all(&p).expect("non-UTF8 torn tail must be recovered");
    assert_eq!(events.len(), 10, "earlier events must survive non-UTF8 tail");
}

// --- HIGH #5: concurrent read during active append ----------------------
//
// Thread A takes an app-level mutex, appends an event, releases.
// Thread B waits on a Barrier then calls read_all.
// The barrier releases mid-operation (i.e. before A drops the mutex).
// Contract: B observes a *consistent* log — either N or N+1 fully-parsed
// events, never a parse error and never a panic.
//
// NOTE: `append_event` is itself atomic via O_APPEND at the OS level, so
// the worst observable interleave is "B reads before A's flush lands"
// (==> N events) or "after" (==> N+1). We assert one of those outcomes.
#[test]
fn read_all_during_active_append_write_race() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");
    seed_n(&p, 20);

    let write_lock = Arc::new(Mutex::new(()));
    let barrier = Arc::new(Barrier::new(2));

    let p_a = p.clone();
    let lock_a = write_lock.clone();
    let bar_a = barrier.clone();
    let t_a = thread::spawn(move || {
        let _guard = lock_a.lock().unwrap();
        // Synchronize: both threads reach here before A performs the append.
        bar_a.wait();
        append_event(
            &p_a,
            &Event::Add {
                ts: base() + Duration::seconds(9999),
                id: "race".into(),
                url: "u".into(),
                path: "race".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        // _guard drops here.
    });

    let p_b = p.clone();
    let bar_b = barrier.clone();
    let t_b = thread::spawn(move || {
        bar_b.wait();
        // Read as aggressively as possible while A is still writing.
        read_all(&p_b)
    });

    t_a.join().unwrap();
    let got = t_b.join().unwrap().expect("read_all must not error under concurrent append");
    assert!(
        got.len() == 20 || got.len() == 21,
        "reader saw inconsistent state: len={} (want 20 or 21)",
        got.len()
    );

    // Final state must always be 21 — appends are durable.
    let settled = read_all(&p).unwrap();
    assert_eq!(settled.len(), 21);
}

// --- MED #6: NUL byte inside a string value -----------------------------
//
// JSON permits \u0000 inside strings. serde_json emits it escaped; the
// round-trip should succeed and preserve the byte.
#[test]
fn nul_byte_in_line_handled() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    let with_nul = Event::Add {
        ts: base(),
        id: "nul".into(),
        url: "before\u{0000}after".into(),
        path: "nul".into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    append_event(&p, &with_nul).unwrap();

    let events = read_all(&p).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Add { url, .. } => {
            assert_eq!(url, "before\u{0000}after", "NUL byte must round-trip");
        }
        other => panic!("unexpected event: {other:?}"),
    }
}

// --- MED #7: oversized line (10 MB string payload) ----------------------
//
// Documents the current cap: BufReader::lines has no hard line-size limit
// in std, so a multi-MB value round-trips correctly. Scaled to 10 MB to
// keep the test under a second on CI; a 100 MB case is left as a manual
// stress target.
#[test]
fn oversized_line_handled() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    let big = "A".repeat(10 * 1024 * 1024); // 10 MiB
    let ev = Event::Add {
        ts: base(),
        id: "big".into(),
        url: big.clone(),
        path: "big".into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    };
    append_event(&p, &ev).unwrap();

    let events = read_all(&p).unwrap();
    assert_eq!(events.len(), 1);
    match &events[0] {
        Event::Add { url, .. } => assert_eq!(url.len(), big.len()),
        other => panic!("unexpected event: {other:?}"),
    }
}
