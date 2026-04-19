//! Append-and-read for the manifest JSONL log.

use super::error::ManifestError;
use super::event::Event;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// Heal a torn trailing line in the manifest, if present.
///
/// If the file exists and does NOT end with `\n`, scan backwards to find the
/// last newline and truncate everything after it. This prevents the next
/// append from fusing its bytes onto a partial line (which would turn an
/// otherwise-recoverable torn tail into mid-line corruption).
///
/// No-ops on:
///   * missing file
///   * empty file
///   * file already ending with `\n`
fn heal_torn_trailing_line(path: &Path) -> Result<(), ManifestError> {
    let mut file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(ManifestError::Io(e)),
    };
    let len = file.metadata()?.len();
    if len == 0 {
        return Ok(());
    }

    // Peek last byte. If it's \n, file is clean.
    let mut buf = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))?;
    file.read_exact(&mut buf)?;
    if buf[0] == b'\n' {
        return Ok(());
    }

    // Scan backwards for the last newline. Truncate to just past it.
    // pos is the index of the byte we're about to inspect.
    let mut pos = len - 1;
    loop {
        if pos == 0 {
            // No newline anywhere → whole file is a torn partial line.
            tracing::warn!("healing manifest: truncating entire torn tail (no prior newline)");
            file.set_len(0)?;
            file.sync_data()?;
            return Ok(());
        }
        pos -= 1;
        file.seek(SeekFrom::Start(pos))?;
        file.read_exact(&mut buf)?;
        if buf[0] == b'\n' {
            // Keep bytes [0, pos] (inclusive of this newline); drop the rest.
            let keep = pos + 1;
            tracing::warn!(
                truncated_from = len,
                truncated_to = keep,
                "healing manifest: truncating torn trailing line"
            );
            file.set_len(keep)?;
            file.sync_data()?;
            return Ok(());
        }
    }
}

/// Append one event to the manifest log, creating the file if missing.
///
/// Writes `<serialized-json>\n` and fsyncs the data portion. Callers
/// holding an exclusive [`crate::fs::ManifestLock`] are guaranteed that no
/// torn-interleave can occur across processes.
///
/// Before writing, a torn trailing line (file not ending in `\n`) is healed
/// by truncating back to the last newline. This prevents a prior crash from
/// fusing partial bytes with the next valid append.
///
/// # Errors
///
/// Returns [`ManifestError::Io`] on I/O failure or
/// [`ManifestError::Serialize`] if the event cannot be serialized.
pub fn append_event(path: &Path, event: &Event) -> Result<(), ManifestError> {
    heal_torn_trailing_line(path)?;
    let mut file = OpenOptions::new().append(true).create(true).open(path)?;
    let line = serde_json::to_string(event).map_err(ManifestError::Serialize)?;
    file.write_all(line.as_bytes())?;
    file.write_all(b"\n")?;
    // fsync the data blocks; metadata flush is not strictly needed for
    // append-only semantics.
    file.sync_data()?;
    Ok(())
}

/// Read every event from the manifest log.
///
/// Byte-oriented line splitter (tolerant of non-UTF-8 in a torn tail).
/// Missing file → empty `Vec`.
///
/// # Torn-line recovery
///
/// A parse error (invalid UTF-8 or invalid JSON) on the **last sequential
/// line** is treated as a torn write left by a crash: the line is discarded
/// with a `tracing::warn!` and earlier events are returned intact. A parse
/// error on any **earlier** line returns [`ManifestError::Corruption`].
///
/// We collect all raw lines up front (byte-oriented) so `is_last` can be
/// decided by line index rather than by the presence of a trailing `\n`.
pub fn read_all(path: &Path) -> Result<Vec<Event>, ManifestError> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(ManifestError::Io(e)),
    };
    let mut reader = BufReader::new(file);
    let mut raw_lines: Vec<Vec<u8>> = Vec::new();
    loop {
        let mut buf: Vec<u8> = Vec::new();
        let n = reader.read_until(b'\n', &mut buf)?;
        if n == 0 {
            break;
        }
        raw_lines.push(buf);
    }
    let total = raw_lines.len();
    let mut events = Vec::new();

    for (idx, bytes) in raw_lines.into_iter().enumerate() {
        let line_num = idx + 1;
        let is_last = line_num == total;

        // Strip trailing \n and optional \r.
        let mut content_end = bytes.len();
        if bytes.last() == Some(&b'\n') {
            content_end -= 1;
            if content_end > 0 && bytes[content_end - 1] == b'\r' {
                content_end -= 1;
            }
        }
        let content = &bytes[..content_end];

        // Empty / whitespace-only lines are skipped silently. ASCII
        // whitespace is enough; we don't attempt UTF-8 decoding for this
        // check so partial-multibyte tails still fall through to the parse
        // step below.
        if content.iter().all(|b| b.is_ascii_whitespace()) {
            continue;
        }

        let s = match std::str::from_utf8(content) {
            Ok(s) => s,
            Err(_) if is_last => {
                tracing::warn!(
                    line = line_num,
                    "discarding torn trailing line in manifest (invalid UTF-8)"
                );
                break;
            }
            Err(_) => {
                tracing::error!(line = line_num, "manifest corruption detected (invalid UTF-8)");
                return Err(ManifestError::Corruption {
                    line: line_num,
                    source: serde_json::Error::io(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid UTF-8 in manifest line",
                    )),
                });
            }
        };

        match serde_json::from_str::<Event>(s) {
            Ok(ev) => events.push(ev),
            Err(e) => {
                if is_last {
                    tracing::warn!(
                        line = line_num,
                        error = %e,
                        "discarding torn trailing line in manifest"
                    );
                    break;
                }
                tracing::error!(
                    line = line_num,
                    error = %e,
                    "manifest corruption detected"
                );
                return Err(ManifestError::Corruption { line: line_num, source: e });
            }
        }
    }
    Ok(events)
}

#[cfg(test)]
mod tests {
    use super::super::event::SCHEMA_VERSION;
    use super::*;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn sample() -> Event {
        Event::Add {
            ts: Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap(),
            id: "a".into(),
            url: "u".into(),
            path: "a".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        }
    }

    #[test]
    fn append_and_read_roundtrip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        let e = sample();
        append_event(&p, &e).unwrap();
        let got = read_all(&p).unwrap();
        assert_eq!(got, vec![e]);
    }

    #[test]
    fn read_missing_file_is_empty() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("absent.jsonl");
        assert!(read_all(&p).unwrap().is_empty());
    }

    #[test]
    fn torn_trailing_line_is_discarded() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        append_event(&p, &sample()).unwrap();
        // Simulate a torn append: partial JSON on a new trailing line.
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"op\":\"add\",\"ts\":\"2026-04").unwrap();
        drop(f);
        let got = read_all(&p).unwrap();
        assert_eq!(got.len(), 1);
    }

    #[test]
    fn earlier_corruption_is_hard_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        // Line 1 is garbage, line 2 is valid — so garbage is NOT the last line.
        let mut f = OpenOptions::new().create(true).append(true).open(&p).unwrap();
        f.write_all(b"not-json\n").unwrap();
        drop(f);
        append_event(&p, &sample()).unwrap();
        let err = read_all(&p).unwrap_err();
        assert!(matches!(err, ManifestError::Corruption { line: 1, .. }));
    }

    #[test]
    fn empty_lines_are_skipped() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        append_event(&p, &sample()).unwrap();
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"\n").unwrap();
        drop(f);
        append_event(&p, &sample()).unwrap();
        assert_eq!(read_all(&p).unwrap().len(), 2);
    }

    #[test]
    fn heal_on_append_truncates_torn_tail() {
        // Prior complete event + partial trailing fragment (no \n).
        // Next append must heal the fragment so the fused bytes don't
        // become a middle-line corruption on next read_all.
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.jsonl");
        append_event(&p, &sample()).unwrap();
        let mut f = OpenOptions::new().append(true).open(&p).unwrap();
        f.write_all(b"{\"op\":\"add\",\"ts\":\"2026").unwrap();
        drop(f);

        append_event(&p, &sample()).unwrap();
        let got = read_all(&p).unwrap();
        assert_eq!(got.len(), 2, "healed torn fragment; both valid events present");
    }
}
