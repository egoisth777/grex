//! Read/write helpers for `grex.lock.jsonl`.

use super::entry::{LockEntry, LockfileError};
use crate::fs::atomic_write;
use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader};
use std::path::Path;

/// Read the lockfile into a map keyed by pack id.
///
/// Missing file → empty map. Unlike the manifest, any parse error is fatal
/// — the lockfile is always rewritten atomically, so a partial line
/// indicates real corruption, not a torn append.
pub fn read_lockfile(path: &Path) -> Result<HashMap<String, LockEntry>, LockfileError> {
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(LockfileError::Io(e)),
    };
    let reader = BufReader::new(file);
    let mut out = HashMap::new();
    for (idx, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let entry: LockEntry = serde_json::from_str(&line)
            .map_err(|e| LockfileError::Corruption { line: idx + 1, source: e })?;
        out.insert(entry.id.clone(), entry);
    }
    Ok(out)
}

/// Atomically replace the lockfile with the given entries.
///
/// Entries are serialized one-per-line in **sorted order by id** so
/// successive writes produce byte-stable output (easier diffing, no noise
/// from `HashMap` iteration order).
pub fn write_lockfile(
    path: &Path,
    entries: &HashMap<String, LockEntry>,
) -> Result<(), LockfileError> {
    let mut ids: Vec<&String> = entries.keys().collect();
    ids.sort();
    let mut buf = String::new();
    for id in ids {
        let line = serde_json::to_string(&entries[id]).map_err(LockfileError::Serialize)?;
        buf.push_str(&line);
        buf.push('\n');
    }
    atomic_write(path, buf.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;

    fn entry(id: &str) -> LockEntry {
        LockEntry {
            id: id.into(),
            sha: "abc".into(),
            branch: "main".into(),
            installed_at: Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap(),
            actions_hash: "".into(),
            schema_version: "1".into(),
        }
    }

    #[test]
    fn read_missing_is_empty() {
        let dir = tempdir().unwrap();
        assert!(read_lockfile(&dir.path().join("absent")).unwrap().is_empty());
    }

    #[test]
    fn write_read_roundtrip() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.lock.jsonl");
        let mut map = HashMap::new();
        map.insert("a".into(), entry("a"));
        map.insert("b".into(), entry("b"));
        write_lockfile(&p, &map).unwrap();
        let back = read_lockfile(&p).unwrap();
        assert_eq!(back, map);
    }

    #[test]
    fn corruption_is_hard_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.lock.jsonl");
        std::fs::write(&p, b"not-json\n").unwrap();
        assert!(matches!(
            read_lockfile(&p).unwrap_err(),
            LockfileError::Corruption { line: 1, .. }
        ));
    }

    #[test]
    fn write_is_deterministic() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("grex.lock.jsonl");
        let mut map = HashMap::new();
        map.insert("z".into(), entry("z"));
        map.insert("a".into(), entry("a"));
        write_lockfile(&p, &map).unwrap();
        let b1 = std::fs::read(&p).unwrap();
        write_lockfile(&p, &map).unwrap();
        let b2 = std::fs::read(&p).unwrap();
        assert_eq!(b1, b2);
        let text = String::from_utf8(b1).unwrap();
        // sorted ids → "a" line precedes "z"
        let a = text.find("\"id\":\"a\"").unwrap();
        let z = text.find("\"id\":\"z\"").unwrap();
        assert!(a < z);
    }
}
