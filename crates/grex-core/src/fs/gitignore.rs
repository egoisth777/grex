//! Per-pack managed-block writer for `.gitignore` files.
//!
//! Each managed block is a contiguous region bounded by marker comments:
//!
//! ```text
//! # >>> grex:<pack_name> >>>
//! <pattern lines>
//! # <<< grex:<pack_name> <<<
//! ```
//!
//! Multiple packs' blocks may coexist in the same file; content outside
//! any managed block is preserved verbatim. Writes are atomic
//! (tmp-file + rename) and preserve the file's line-ending convention
//! (LF vs CRLF). See R-M5-08 in `openspec/feat-grex/spec.md`.
//!
//! Integration into pack-type plugins is deliberately deferred to a
//! later M5-2 stage; this module is the pure primitive.
//!
//! # Invariants
//!
//! * A block begins at the first line matching `# >>> grex:<name> >>>`
//!   and ends at the next line matching `# <<< grex:<name> <<<`.
//! * An opening marker without a matching closer → `UnclosedBlock`.
//! * Pack names are validated: no ASCII control, no `<`, `>`, no newline.
//! * `upsert` on a missing file creates it; `remove` on a missing file
//!   is a no-op `Ok`.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::atomic::atomic_write;

/// Errors produced by the gitignore managed-block API.
#[derive(Debug, thiserror::Error)]
pub enum GitignoreError {
    /// I/O error while reading or writing the target file.
    #[error("io error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    /// File has an opening marker for `pack` at `line` with no matching closer.
    #[error("malformed managed block for pack {pack}: unclosed marker at line {line}")]
    UnclosedBlock { pack: String, line: usize },
    /// Pack name contains disallowed characters.
    #[error("invalid pack name: {0}")]
    InvalidPackName(String),
}

/// Validate a pack name. Rejects ASCII control chars, `<`, `>`, and newlines.
fn validate_pack_name(name: &str) -> Result<(), GitignoreError> {
    if name.is_empty() {
        return Err(GitignoreError::InvalidPackName(name.to_string()));
    }
    for ch in name.chars() {
        if ch.is_ascii_control() || ch == '<' || ch == '>' || ch == '\n' || ch == '\r' {
            return Err(GitignoreError::InvalidPackName(name.to_string()));
        }
    }
    Ok(())
}

/// Detect the predominant line ending in `text`. Defaults to LF when the
/// file is empty or has no line terminator.
fn detect_line_ending(text: &str) -> &'static str {
    // If any `\r\n` appears, assume CRLF. Otherwise LF. This is the
    // "preserve the file's convention" rule: mixed-ending files collapse
    // to CRLF on rewrite, which is the safer default on Windows.
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Build the open/close marker strings for `pack`.
fn markers(pack: &str) -> (String, String) {
    (format!("# >>> grex:{pack} >>>"), format!("# <<< grex:{pack} <<<"))
}

/// Read file contents, returning `Ok(None)` if the file is absent.
fn read_opt(target: &Path) -> Result<Option<String>, GitignoreError> {
    match fs::read_to_string(target) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(GitignoreError::Io { path: target.to_path_buf(), source }),
    }
}

/// Split `text` into (before_block, block_inner_lines, after_block).
/// Returns `None` if `pack`'s opening marker is not present.
///
/// * `before` and `after` are the raw substrings (including any trailing
///   newline on the last line of `before` and leading newline absent on
///   `after`).
/// * `inner` is the list of pattern lines between the markers, already
///   trimmed of the trailing EOL.
///
/// On an opening marker with no closer, returns `UnclosedBlock`.
#[allow(clippy::type_complexity)]
fn split_block<'a>(
    text: &'a str,
    pack: &str,
) -> Result<Option<(&'a str, Vec<&'a str>, &'a str)>, GitignoreError> {
    let (open, close) = markers(pack);
    let lines: Vec<&str> = text.split_inclusive(['\n']).collect();

    // Find the opening marker (trimmed of EOL).
    let open_idx = lines.iter().position(|l| strip_eol(l) == open);
    let Some(open_idx) = open_idx else {
        return Ok(None);
    };

    // Find the closing marker AFTER the opener.
    let close_rel = lines[open_idx + 1..].iter().position(|l| strip_eol(l) == close);
    let Some(close_rel) = close_rel else {
        return Err(GitignoreError::UnclosedBlock {
            pack: pack.to_string(),
            line: open_idx + 1, // 1-based line number
        });
    };
    let close_idx = open_idx + 1 + close_rel;

    // Compute byte offsets.
    let before_end: usize = lines[..open_idx].iter().map(|l| l.len()).sum();
    let after_start: usize = lines[..=close_idx].iter().map(|l| l.len()).sum();

    let before = &text[..before_end];
    let after = &text[after_start..];
    let inner: Vec<&str> = lines[open_idx + 1..close_idx].iter().map(|l| strip_eol(l)).collect();

    Ok(Some((before, inner, after)))
}

/// Strip a trailing `\n` or `\r\n` from a line slice.
fn strip_eol(line: &str) -> &str {
    line.strip_suffix("\r\n").or_else(|| line.strip_suffix('\n')).unwrap_or(line)
}

/// Render a managed block for `pack` with `patterns`, using `eol` between
/// lines and after the final line. Does NOT emit a leading newline.
fn render_block(pack: &str, patterns: &[&str], eol: &str) -> String {
    let (open, close) = markers(pack);
    let mut out = String::new();
    out.push_str(&open);
    out.push_str(eol);
    for p in patterns {
        out.push_str(p);
        out.push_str(eol);
    }
    out.push_str(&close);
    out.push_str(eol);
    out
}

/// Write or update the managed block for `pack_name` in the `.gitignore`
/// at `target`. Content outside the block (including other packs' blocks)
/// is preserved verbatim.
///
/// * If `target` does not exist, it is created.
/// * If the block exists, its contents are replaced with `patterns`.
/// * If absent, a new block is appended.
/// * Empty `patterns` is valid — the block is kept with zero pattern lines.
///
/// Writes are atomic (tmp + rename) and preserve the file's line-ending
/// convention (LF vs CRLF). A new file defaults to LF.
///
/// # Errors
///
/// * [`GitignoreError::InvalidPackName`] if `pack_name` contains control
///   chars, `<`, `>`, or a newline.
/// * [`GitignoreError::UnclosedBlock`] if an existing block for this pack
///   has an opener but no closer.
/// * [`GitignoreError::Io`] on any filesystem error.
pub fn upsert_managed_block(
    target: &Path,
    pack_name: &str,
    patterns: &[&str],
) -> Result<(), GitignoreError> {
    validate_pack_name(pack_name)?;
    let existing = read_opt(target)?;
    let (text, eol) = match &existing {
        Some(t) => (t.as_str(), detect_line_ending(t)),
        None => ("", "\n"),
    };

    let new_contents = match split_block(text, pack_name)? {
        Some((before, _old, after)) => {
            // Replace in place.
            let mut out = String::with_capacity(text.len() + 64);
            out.push_str(before);
            out.push_str(&render_block(pack_name, patterns, eol));
            out.push_str(after);
            out
        }
        None => {
            // Append. Ensure existing trailing newline before our block so
            // the opening marker starts on its own line.
            let mut out = String::with_capacity(text.len() + 64);
            out.push_str(text);
            if !text.is_empty() && !text.ends_with('\n') {
                out.push_str(eol);
            }
            out.push_str(&render_block(pack_name, patterns, eol));
            out
        }
    };

    atomic_write(target, new_contents.as_bytes())
        .map_err(|source| GitignoreError::Io { path: target.to_path_buf(), source })
}

/// Remove the managed block for `pack_name` from the `.gitignore` at
/// `target`. Other packs' blocks and user content outside the block are
/// preserved.
///
/// * If `target` does not exist, this is a no-op `Ok(())`.
/// * If the block is absent, this is a no-op `Ok(())`.
///
/// # Errors
///
/// * [`GitignoreError::InvalidPackName`] if `pack_name` is invalid.
/// * [`GitignoreError::UnclosedBlock`] if the block has an opener but no
///   closer.
/// * [`GitignoreError::Io`] on any filesystem error.
pub fn remove_managed_block(target: &Path, pack_name: &str) -> Result<(), GitignoreError> {
    validate_pack_name(pack_name)?;
    let Some(text) = read_opt(target)? else {
        return Ok(());
    };

    let Some((before, _inner, after)) = split_block(&text, pack_name)? else {
        return Ok(());
    };

    let mut out = String::with_capacity(before.len() + after.len());
    out.push_str(before);
    out.push_str(after);

    atomic_write(target, out.as_bytes())
        .map_err(|source| GitignoreError::Io { path: target.to_path_buf(), source })
}

/// Read the current pattern lines inside `pack_name`'s managed block.
///
/// Returns `Ok(None)` if the file is missing or the block is absent.
/// Returns `Ok(Some(vec))` with zero-or-more pattern strings otherwise.
///
/// # Errors
///
/// * [`GitignoreError::InvalidPackName`] if `pack_name` is invalid.
/// * [`GitignoreError::UnclosedBlock`] if the block has an opener but no
///   closer.
/// * [`GitignoreError::Io`] on read failure.
pub fn read_managed_block(
    target: &Path,
    pack_name: &str,
) -> Result<Option<Vec<String>>, GitignoreError> {
    validate_pack_name(pack_name)?;
    let Some(text) = read_opt(target)? else {
        return Ok(None);
    };
    let Some((_before, inner, _after)) = split_block(&text, pack_name)? else {
        return Ok(None);
    };
    Ok(Some(inner.into_iter().map(|s| s.to_string()).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    // ---- helpers ----

    fn read(p: &Path) -> String {
        fs::read_to_string(p).unwrap()
    }

    // 1. upsert into empty file
    #[test]
    fn upsert_into_empty_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "").unwrap();
        upsert_managed_block(&p, "foo", &["a", "b"]).unwrap();
        let got = read(&p);
        assert!(got.starts_with("# >>> grex:foo >>>\n"));
        assert!(got.contains("\na\n"));
        assert!(got.contains("\nb\n"));
        assert!(got.ends_with("# <<< grex:foo <<<\n"));
    }

    // 2. upsert into nonexistent file (creates)
    #[test]
    fn upsert_creates_missing_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        assert!(!p.exists());
        upsert_managed_block(&p, "foo", &["target/"]).unwrap();
        assert!(p.exists());
        let got = read(&p);
        assert!(got.contains("target/"));
    }

    // 3. upsert into file with only user content — appends block after
    #[test]
    fn upsert_appends_after_user_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "# my rules\nnode_modules/\n").unwrap();
        upsert_managed_block(&p, "foo", &["x"]).unwrap();
        let got = read(&p);
        // User content is preserved at the top.
        assert!(got.starts_with("# my rules\nnode_modules/\n"));
        // Block comes after.
        assert!(got.contains("# >>> grex:foo >>>\nx\n# <<< grex:foo <<<\n"));
    }

    // 4. upsert for 2nd pack — both blocks coexist
    #[test]
    fn upsert_two_packs_coexist() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        upsert_managed_block(&p, "a", &["x"]).unwrap();
        upsert_managed_block(&p, "b", &["y"]).unwrap();
        let got = read(&p);
        assert!(got.contains("# >>> grex:a >>>\nx\n# <<< grex:a <<<\n"));
        assert!(got.contains("# >>> grex:b >>>\ny\n# <<< grex:b <<<\n"));
        // Insertion order: a before b.
        let ia = got.find("grex:a").unwrap();
        let ib = got.find("grex:b").unwrap();
        assert!(ia < ib);
    }

    // 5. update existing block (add/remove/reorder)
    #[test]
    fn update_existing_block_replaces_patterns() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        upsert_managed_block(&p, "foo", &["a", "b", "c"]).unwrap();
        upsert_managed_block(&p, "foo", &["c", "a"]).unwrap();
        let patterns = read_managed_block(&p, "foo").unwrap().unwrap();
        assert_eq!(patterns, vec!["c".to_string(), "a".to_string()]);
        // Exactly one block in the file.
        assert_eq!(read(&p).matches("# >>> grex:foo >>>").count(), 1);
    }

    // 6. remove block — other blocks preserved
    #[test]
    fn remove_preserves_other_blocks() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        upsert_managed_block(&p, "a", &["x"]).unwrap();
        upsert_managed_block(&p, "b", &["y"]).unwrap();
        remove_managed_block(&p, "a").unwrap();
        let got = read(&p);
        assert!(!got.contains("grex:a"));
        assert!(got.contains("# >>> grex:b >>>\ny\n# <<< grex:b <<<\n"));
    }

    // 7. remove block — user content outside preserved
    #[test]
    fn remove_preserves_user_content() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "user1\n").unwrap();
        upsert_managed_block(&p, "foo", &["x"]).unwrap();
        // Append trailing user content AFTER the block.
        let with_tail = format!("{}user2\n", read(&p));
        fs::write(&p, with_tail).unwrap();
        remove_managed_block(&p, "foo").unwrap();
        let got = read(&p);
        assert!(got.contains("user1\n"));
        assert!(got.contains("user2\n"));
        assert!(!got.contains("grex:foo"));
    }

    // 8. remove non-existent block = no-op Ok
    #[test]
    fn remove_absent_block_noop() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "user\n").unwrap();
        remove_managed_block(&p, "foo").unwrap();
        assert_eq!(read(&p), "user\n");

        // Also: missing file is ok.
        let p2 = dir.path().join("missing");
        remove_managed_block(&p2, "foo").unwrap();
        assert!(!p2.exists());
    }

    // 9. read existing block returns Some(patterns)
    #[test]
    fn read_existing_block_some() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        upsert_managed_block(&p, "foo", &["a", "b"]).unwrap();
        let got = read_managed_block(&p, "foo").unwrap();
        assert_eq!(got, Some(vec!["a".to_string(), "b".to_string()]));
    }

    // 10. read absent block returns Ok(None)
    #[test]
    fn read_absent_block_none() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        // Missing file.
        assert_eq!(read_managed_block(&p, "foo").unwrap(), None);
        // Present file, no block.
        fs::write(&p, "user\n").unwrap();
        assert_eq!(read_managed_block(&p, "foo").unwrap(), None);
    }

    // 11. malformed unclosed block — returns UnclosedBlock
    #[test]
    fn unclosed_block_error() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "# >>> grex:foo >>>\na\nb\n").unwrap();
        let err = read_managed_block(&p, "foo").unwrap_err();
        match err {
            GitignoreError::UnclosedBlock { pack, line } => {
                assert_eq!(pack, "foo");
                assert_eq!(line, 1);
            }
            other => panic!("expected UnclosedBlock, got {other:?}"),
        }
    }

    // 12. invalid pack name (contains `>`) rejected
    #[test]
    fn invalid_pack_name_angle() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        let err = upsert_managed_block(&p, "bad>name", &[]).unwrap_err();
        assert!(matches!(err, GitignoreError::InvalidPackName(_)));
        let err = remove_managed_block(&p, "bad<name").unwrap_err();
        assert!(matches!(err, GitignoreError::InvalidPackName(_)));
        let err = read_managed_block(&p, "bad>name").unwrap_err();
        assert!(matches!(err, GitignoreError::InvalidPackName(_)));
    }

    // 13. invalid pack name (contains newline) rejected
    #[test]
    fn invalid_pack_name_newline() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        let err = upsert_managed_block(&p, "bad\nname", &[]).unwrap_err();
        assert!(matches!(err, GitignoreError::InvalidPackName(_)));
        // Also rejects empty and control chars.
        let err = upsert_managed_block(&p, "", &[]).unwrap_err();
        assert!(matches!(err, GitignoreError::InvalidPackName(_)));
        let err = upsert_managed_block(&p, "bad\tname", &[]).unwrap_err();
        assert!(matches!(err, GitignoreError::InvalidPackName(_)));
    }

    // 14. LF line endings preserved
    #[test]
    fn lf_line_endings_preserved() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "user1\nuser2\n").unwrap();
        upsert_managed_block(&p, "foo", &["x", "y"]).unwrap();
        let got = read(&p);
        assert!(!got.contains("\r\n"), "must not introduce CRLF: {got:?}");
        assert!(got.contains("\nx\n"));
    }

    // 15. CRLF line endings preserved
    #[test]
    fn crlf_line_endings_preserved() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "user1\r\nuser2\r\n").unwrap();
        upsert_managed_block(&p, "foo", &["x", "y"]).unwrap();
        let got = fs::read(&p).unwrap();
        // Verify the marker + pattern lines all end in \r\n.
        let s = String::from_utf8(got).unwrap();
        assert!(s.contains("# >>> grex:foo >>>\r\n"));
        assert!(s.contains("\r\nx\r\n"));
        assert!(s.contains("\r\ny\r\n"));
        assert!(s.contains("# <<< grex:foo <<<\r\n"));
    }

    // 15b. mixed line-ending file — documented behaviour: any `\r\n`
    // triggers CRLF-normalisation for the whole file. `detect_line_ending`
    // picks CRLF as soon as it sees one `\r\n`, so LF-only runs get
    // rewritten to `\r\n`. This is the conservative default on Windows
    // and keeps the managed-block writer deterministic (no majority-vote
    // heuristic required).
    #[test]
    fn upsert_into_mixed_line_ending_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        // One LF line, one CRLF line.
        fs::write(&p, "lf-only\ncrlf-line\r\n").unwrap();
        upsert_managed_block(&p, "foo", &["x"]).unwrap();
        let s = fs::read_to_string(&p).unwrap();
        // User content preserved (the parser reads lines separated by
        // either `\n` or `\r\n`), block written with CRLF.
        assert!(s.contains("# >>> grex:foo >>>\r\n"), "block must use CRLF: {s:?}");
        assert!(s.contains("\r\nx\r\n"), "pattern must use CRLF: {s:?}");
        assert!(s.contains("# <<< grex:foo <<<\r\n"), "close must use CRLF: {s:?}");
    }

    // 16. atomic write — file exists throughout operation (no leftover temp)
    #[test]
    fn atomic_write_leaves_no_temp_files() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "user\n").unwrap();
        upsert_managed_block(&p, "foo", &["x"]).unwrap();
        // Only the target file should exist in the directory; the atomic
        // writer's tmp suffix is cleaned up on success.
        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(entries, vec![".gitignore".to_string()]);
        // And the target itself exists (rename landed).
        assert!(p.exists());
    }

    // 17. upsert idempotency — running twice with same args = same file
    #[test]
    fn upsert_is_idempotent() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        fs::write(&p, "user\n").unwrap();
        upsert_managed_block(&p, "foo", &["x", "y"]).unwrap();
        let first = read(&p);
        upsert_managed_block(&p, "foo", &["x", "y"]).unwrap();
        let second = read(&p);
        assert_eq!(first, second);
    }

    // Extra: empty-patterns block is preserved intact.
    #[test]
    fn upsert_empty_patterns_keeps_block() {
        let dir = tempdir().unwrap();
        let p = dir.path().join(".gitignore");
        upsert_managed_block(&p, "foo", &[]).unwrap();
        let got = read(&p);
        assert!(got.contains("# >>> grex:foo >>>\n# <<< grex:foo <<<\n"));
        let patterns = read_managed_block(&p, "foo").unwrap().unwrap();
        assert!(patterns.is_empty());
    }
}
