//! Integration: corruption on a non-last line must surface as a hard error.

use chrono::{DateTime, TimeZone, Utc};
use grex_core::manifest::{append_event, read_all, Event, ManifestError, SCHEMA_VERSION};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use tempfile::tempdir;

fn ts(sec: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, sec).unwrap()
}

fn add_event(i: usize) -> Event {
    Event::Add {
        ts: ts((i % 60) as u32),
        id: format!("pack-{i}"),
        url: format!("url-{i}"),
        path: format!("path-{i}"),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    }
}

fn write_n_events(path: &Path, n: usize) {
    for i in 0..n {
        append_event(path, &add_event(i)).unwrap();
    }
}

/// Corrupt the `target` line (1-based) of `path` by replacing its leading
/// `{` with an ASCII letter, which keeps the file valid UTF-8 but makes
/// JSON parsing of that line fail.
fn corrupt_line(path: &Path, target: usize) {
    let mut bytes = fs::read(path).unwrap();
    let mut line_no = 1usize;
    let mut at_line_start = true;
    let mut corrupted = false;
    for b in bytes.iter_mut() {
        if *b == b'\n' {
            line_no += 1;
            at_line_start = true;
            continue;
        }
        if line_no == target && at_line_start {
            assert_eq!(*b, b'{', "expected JSON object at line {target}");
            *b = b'x';
            corrupted = true;
            break;
        }
        at_line_start = false;
    }
    assert!(corrupted, "failed to corrupt line {target}");
    fs::write(path, bytes).unwrap();
}

#[test]
fn earlier_corruption_hard_errors() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.jsonl");

    // Write garbage as line 1.
    let mut f = OpenOptions::new().create(true).append(true).open(&p).unwrap();
    f.write_all(b"this-is-not-json\n").unwrap();
    drop(f);

    // Append a valid event as line 2 — now garbage is NOT the last line.
    append_event(&p, &Event::Rm { ts: ts(0), id: "x".into() }).unwrap();
    // Also add a valid trailing sentinel so the garbage is clearly mid-file.
    append_event(
        &p,
        &Event::Add {
            ts: ts(1),
            id: "y".into(),
            url: "u".into(),
            path: "y".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        },
    )
    .unwrap();

    match read_all(&p).unwrap_err() {
        ManifestError::Corruption { line, .. } => assert_eq!(line, 1),
        other => panic!("expected Corruption, got {other:?}"),
    }
}

/// Parameterized: corruption anywhere except the trailing line must be a
/// hard error, and the reported line number must match the corrupted line.
#[test]
fn corruption_at_various_line_positions() {
    const N: usize = 100;
    // line 1, line 2, mid-file, and second-to-last — all non-trailing.
    let targets = [1usize, 2, N / 2, N - 1];

    for &target in &targets {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        write_n_events(&p, N);
        corrupt_line(&p, target);

        match read_all(&p).unwrap_err() {
            ManifestError::Corruption { line, .. } => {
                assert_eq!(line, target, "case target={target}: got Corruption at line {line}")
            }
            other => panic!("case target={target}: expected Corruption, got {other:?}"),
        }
    }
}
