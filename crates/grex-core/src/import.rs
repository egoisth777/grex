//! `grex import` — ingest legacy flat `REPOS.json` into a manifest.
//!
//! Parses `[{url, path}, …]`, classifies each entry by a small heuristic
//! (git URL → `scripted`, empty/path-only → `declarative`), and dispatches
//! equivalent add registrations into the target manifest while skipping
//! any path that already exists.
//!
//! Scope (feat-m7-4a):
//! * Only the flat `REPOS.json` schema from the meta-repo.
//! * Skip-on-collision (never overwrite).
//! * `--dry-run` short-circuits before any manifest write.
//!
//! Dispatch: each accepted entry is dispatched through
//! [`crate::add::add_pack`], which is the single shared registration
//! path used by `grex add` and the MCP edge — so `import` and `add`
//! emit byte-identical `Event::Add` rows for the same input.

use crate::add::{add_pack, AddError, AddOpts, AddRequest};
use crate::fs::ManifestLock;
use crate::manifest;
use crate::pack::validate::child_path::reject_reason;
use serde::Deserialize;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Pack kind assigned by the import heuristic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportedKind {
    Scripted,
    Declarative,
}

impl ImportedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ImportedKind::Scripted => "scripted",
            ImportedKind::Declarative => "declarative",
        }
    }
}

/// One entry from a `REPOS.json` array.
#[derive(Debug, Clone, Deserialize)]
struct RawEntry {
    #[serde(default)]
    url: String,
    path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportEntry {
    pub path: String,
    pub url: String,
    pub kind: ImportedKind,
    pub would_dispatch: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    DuplicateInInput,
    PathCollision,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportSkip {
    pub path: String,
    pub reason: SkipReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportFailure {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportPlan {
    pub imported: Vec<ImportEntry>,
    pub skipped: Vec<ImportSkip>,
    pub failed: Vec<ImportFailure>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOpts {
    pub dry_run: bool,
}

#[derive(Debug, Error)]
pub enum ImportError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("malformed REPOS.json at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("manifest write failed: {0}")]
    Manifest(#[from] manifest::ManifestError),
    #[error("manifest lock failed at {path}: {source}")]
    Lock {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Classify a `REPOS.json` entry into a pack kind.
pub fn classify(url: &str) -> ImportedKind {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return ImportedKind::Declarative;
    }
    let low = trimmed.to_ascii_lowercase();
    if low.starts_with("file://") {
        return ImportedKind::Scripted;
    }
    let looks_git = low.starts_with("http://")
        || low.starts_with("https://")
        || low.starts_with("git@")
        || low.starts_with("ssh://")
        || low.starts_with("git://")
        || low.ends_with(".git");
    if looks_git {
        ImportedKind::Scripted
    } else {
        ImportedKind::Declarative
    }
}

fn parse_repos_json(repos_json: &Path) -> Result<Vec<RawEntry>, ImportError> {
    let bytes = std::fs::read(repos_json)
        .map_err(|source| ImportError::Io { path: repos_json.to_path_buf(), source })?;
    let parsed: Vec<RawEntry> = serde_json::from_slice(&bytes)
        .map_err(|source| ImportError::Parse { path: repos_json.to_path_buf(), source })?;
    Ok(parsed)
}

fn existing_paths(manifest_path: &Path) -> Result<std::collections::HashSet<String>, ImportError> {
    let events = manifest::read_all(manifest_path)?;
    let state = manifest::fold(events);
    Ok(state.values().map(|s| s.path.clone()).collect())
}

/// Ingest a flat `REPOS.json` into the manifest at `manifest_path`.
pub fn import_from_repos_json(
    repos_json: &Path,
    manifest_path: &Path,
    opts: ImportOpts,
) -> Result<ImportPlan, ImportError> {
    let raw = parse_repos_json(repos_json)?;

    // CR2: read existing manifest state and commit new rows under a single
    // exclusive ManifestLock so a concurrent writer cannot insert a row
    // between the snapshot and the commit loop. Lock sidecar lives next to
    // the event log at `<ws>/.grex/events.lock` (parent dir created lazily
    // because ManifestLock::open does not auto-create like ScopedLock).
    let lock_path = prepare_lock_path(manifest_path)?;
    let mut lock = ManifestLock::open(manifest_path, &lock_path)
        .map_err(|source| ImportError::Lock { path: lock_path.clone(), source })?;

    lock.write(|| {
        let existing = existing_paths(manifest_path)?;
        let plan = compute_plan(raw, &existing, opts.dry_run);
        if !opts.dry_run {
            commit_plan(&plan, manifest_path)?;
        }
        Ok(plan)
    })
    .map_err(|source| ImportError::Lock { path: lock_path.clone(), source })?
}

fn prepare_lock_path(manifest_path: &Path) -> Result<PathBuf, ImportError> {
    let lock_path = manifest_path
        .parent()
        .map(|p| p.join("events.lock"))
        .unwrap_or_else(|| PathBuf::from(".grex/events.lock"));
    if let Some(parent) = lock_path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|source| ImportError::Lock { path: lock_path.clone(), source })?;
        }
    }
    Ok(lock_path)
}

fn compute_plan(
    raw: Vec<RawEntry>,
    existing: &std::collections::HashSet<String>,
    dry_run: bool,
) -> ImportPlan {
    let mut plan = ImportPlan::default();
    let mut seen_in_input: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in raw {
        classify_entry(entry, existing, &mut seen_in_input, &mut plan, dry_run);
    }
    plan
}

fn classify_entry(
    entry: RawEntry,
    existing: &std::collections::HashSet<String>,
    seen_in_input: &mut std::collections::HashSet<String>,
    plan: &mut ImportPlan,
    dry_run: bool,
) {
    let path = entry.path.clone();
    // Bare-name validation BEFORE any manifest write — refuses to
    // ingest a `path` that would later trip `ChildPathValidator`
    // (separators, `.` / `..`, regex mismatch, empty). Without this
    // gate, `migration.md`'s promise that import "validates" was
    // untrue: bad rows landed as `Event::Add` rows that only failed
    // at sync time. Fail-fast at import is a much friendlier signal.
    if let Some(reason) = reject_reason(&path) {
        plan.failed.push(ImportFailure { path, error: format!("invalid `path`: {reason}") });
        return;
    }
    // NOTE: PackId currently aliases path (identity derivation), so
    // path-dedup also catches PackId collisions. When PackId
    // derivation diverges from path (future change), add a parallel
    // `existing_ids` set + dedup pass. Tracked in
    // openspec/changes/feat-v1.3.2/design.md review-delta CR1.
    if existing.contains(&path) {
        plan.skipped.push(ImportSkip { path, reason: SkipReason::PathCollision });
        return;
    }
    if !seen_in_input.insert(path.clone()) {
        plan.skipped.push(ImportSkip { path, reason: SkipReason::DuplicateInInput });
        return;
    }
    let kind = classify(&entry.url);
    plan.imported.push(ImportEntry { path, url: entry.url, kind, would_dispatch: dry_run });
}

fn commit_plan(plan: &ImportPlan, manifest_path: &Path) -> Result<(), ImportError> {
    for entry in &plan.imported {
        add_pack(
            manifest_path,
            AddRequest::new(entry.url.clone(), entry.path.clone(), entry.kind.as_str().to_string()),
            AddOpts { dry_run: false },
        )
        .map_err(add_error_to_import_error)?;
    }
    Ok(())
}

fn add_error_to_import_error(err: AddError) -> ImportError {
    match err {
        AddError::Manifest(err) => ImportError::Manifest(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{Event, SCHEMA_VERSION};
    use chrono::Utc;
    use tempfile::tempdir;

    fn write_json(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn classify_https_git_url_is_scripted() {
        assert_eq!(classify("https://github.com/x/y.git"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_http_git_url_is_scripted() {
        assert_eq!(classify("http://example.com/x.git"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_ssh_git_url_is_scripted() {
        assert_eq!(classify("git@github.com:x/y.git"), ImportedKind::Scripted);
        assert_eq!(classify("ssh://git@host/x.git"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_git_protocol_is_scripted() {
        assert_eq!(classify("git://host/x.git"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_dot_git_suffix_is_scripted() {
        assert_eq!(classify("some-weird-host/x.git"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_empty_url_is_declarative() {
        assert_eq!(classify(""), ImportedKind::Declarative);
    }

    #[test]
    fn classify_whitespace_url_is_declarative() {
        assert_eq!(classify("   "), ImportedKind::Declarative);
    }

    #[test]
    fn classify_bare_path_is_declarative() {
        assert_eq!(classify("foo/bar"), ImportedKind::Declarative);
        assert_eq!(classify("my-pack"), ImportedKind::Declarative);
    }

    #[test]
    fn classify_case_insensitive() {
        assert_eq!(classify("HTTPS://X/Y.GIT"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_file_url_is_scripted() {
        assert_eq!(classify("file:///tmp/foo"), ImportedKind::Scripted);
    }

    #[test]
    fn classify_relative_path_is_declarative() {
        assert_eq!(classify("./local/path"), ImportedKind::Declarative);
    }

    #[test]
    fn imported_kind_str_is_stable() {
        assert_eq!(ImportedKind::Scripted.as_str(), "scripted");
        assert_eq!(ImportedKind::Declarative.as_str(), "declarative");
    }

    #[test]
    fn parse_rejects_missing_file() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("absent.json");
        let err = parse_repos_json(&p).unwrap_err();
        assert!(matches!(err, ImportError::Io { .. }));
    }

    #[test]
    fn parse_rejects_malformed_json() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bad.json");
        write_json(&p, r#"[{"url": "x", "path": "a",}]"#);
        let err = parse_repos_json(&p).unwrap_err();
        assert!(matches!(err, ImportError::Parse { .. }));
    }

    #[test]
    fn parse_rejects_non_array_shape() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bad.json");
        write_json(&p, r#"{"url": "x", "path": "a"}"#);
        let err = parse_repos_json(&p).unwrap_err();
        assert!(matches!(err, ImportError::Parse { .. }));
    }

    #[test]
    fn parse_rejects_entry_missing_path_field() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bad.json");
        write_json(&p, r#"[{"url": "x"}]"#);
        let err = parse_repos_json(&p).unwrap_err();
        assert!(matches!(err, ImportError::Parse { .. }));
    }

    #[test]
    fn parse_rejects_array_of_strings() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("bad.json");
        write_json(&p, r#"["foo", "bar"]"#);
        let err = parse_repos_json(&p).unwrap_err();
        assert!(matches!(err, ImportError::Parse { .. }));
    }

    #[test]
    fn parse_accepts_empty_array() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("empty.json");
        write_json(&p, "[]");
        let out = parse_repos_json(&p).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn parse_accepts_missing_url_field() {
        let dir = tempdir().unwrap();
        let p = dir.path().join("ok.json");
        write_json(&p, r#"[{"path": "foo"}]"#);
        let out = parse_repos_json(&p).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].url, "");
        assert_eq!(out[0].path, "foo");
    }

    #[test]
    fn import_parses_flat_repos_json_three_entries() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(
            &input,
            r#"[
                {"url": "https://github.com/a/a.git", "path": "a"},
                {"url": "git@github.com:b/b.git", "path": "b"},
                {"url": "", "path": "c"}
            ]"#,
        );
        let plan = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: true }).unwrap();
        assert_eq!(plan.imported.len(), 3);
        assert!(plan.skipped.is_empty());
        assert!(plan.failed.is_empty());
        assert_eq!(plan.imported[0].kind, ImportedKind::Scripted);
        assert_eq!(plan.imported[1].kind, ImportedKind::Scripted);
        assert_eq!(plan.imported[2].kind, ImportedKind::Declarative);
        assert!(plan.imported.iter().all(|e| e.would_dispatch));
    }

    #[test]
    fn import_dry_run_does_not_write_manifest() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(&input, r#"[{"url": "https://x/y.git", "path": "foo"}]"#);
        let _ = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: true }).unwrap();
        assert!(!manifest.exists());
    }

    #[test]
    fn import_real_run_appends_one_row_per_entry() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(
            &input,
            r#"[
                {"url": "https://github.com/a/a.git", "path": "a"},
                {"url": "", "path": "b"}
            ]"#,
        );
        let plan =
            import_from_repos_json(&input, &manifest, ImportOpts { dry_run: false }).unwrap();
        assert_eq!(plan.imported.len(), 2);
        let events = manifest::read_all(&manifest).unwrap();
        assert_eq!(events.len(), 2);
        match &events[0] {
            Event::Add { path, pack_type, .. } => {
                assert_eq!(path, "a");
                assert_eq!(pack_type, "scripted");
            }
            _ => panic!("expected Add"),
        }
        match &events[1] {
            Event::Add { path, pack_type, .. } => {
                assert_eq!(path, "b");
                assert_eq!(pack_type, "declarative");
            }
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn import_event_shape_matches_add_run() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let import_manifest = dir.path().join("grex-import.jsonl");
        let add_manifest = dir.path().join("grex-add.jsonl");
        write_json(&input, r#"[{"url": "https://x/y.git", "path": "foo"}]"#);

        import_from_repos_json(&input, &import_manifest, ImportOpts { dry_run: false }).unwrap();
        crate::add::add_pack(
            &add_manifest,
            crate::add::AddRequest::new("https://x/y.git", "foo", "scripted"),
            crate::add::AddOpts { dry_run: false },
        )
        .unwrap();

        let import_events = manifest::read_all(&import_manifest).unwrap();
        let add_events = manifest::read_all(&add_manifest).unwrap();
        assert_eq!(add_event_fields(&import_events[0]), add_event_fields(&add_events[0]));
    }

    fn add_event_fields(event: &Event) -> (&str, &str, &str, &str, &str) {
        match event {
            Event::Add { id, url, path, pack_type, schema_version, .. } => {
                (id, url, path, pack_type, schema_version)
            }
            _ => panic!("expected Add"),
        }
    }

    #[test]
    fn import_skips_existing_manifest_row() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        manifest::append_event(
            &manifest,
            &Event::Add {
                ts: Utc::now(),
                id: "a".into(),
                url: "pre".into(),
                path: "a".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        write_json(
            &input,
            r#"[
                {"url": "https://x/a.git", "path": "a"},
                {"url": "", "path": "b"}
            ]"#,
        );
        let plan =
            import_from_repos_json(&input, &manifest, ImportOpts { dry_run: false }).unwrap();
        assert_eq!(plan.imported.len(), 1);
        assert_eq!(plan.imported[0].path, "b");
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].path, "a");
        assert_eq!(plan.skipped[0].reason, SkipReason::PathCollision);
    }

    #[test]
    fn import_is_idempotent_on_second_run() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(&input, r#"[{"url": "https://x/y.git", "path": "foo"}]"#);
        let p1 = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: false }).unwrap();
        assert_eq!(p1.imported.len(), 1);
        let p2 = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: false }).unwrap();
        assert_eq!(p2.imported.len(), 0);
        assert_eq!(p2.skipped.len(), 1);
        let events = manifest::read_all(&manifest).unwrap();
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn import_detects_duplicate_paths_within_input() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(
            &input,
            r#"[
                {"url": "https://x/y.git", "path": "foo"},
                {"url": "https://other.git", "path": "foo"}
            ]"#,
        );
        let plan = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: true }).unwrap();
        assert_eq!(plan.imported.len(), 1);
        assert_eq!(plan.skipped.len(), 1);
        assert_eq!(plan.skipped[0].reason, SkipReason::DuplicateInInput);
    }

    #[test]
    fn import_empty_array_produces_empty_plan() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(&input, "[]");
        let plan =
            import_from_repos_json(&input, &manifest, ImportOpts { dry_run: false }).unwrap();
        assert!(plan.imported.is_empty());
        assert!(plan.skipped.is_empty());
        assert!(!manifest.exists());
    }

    #[test]
    fn import_missing_input_surfaces_io_error() {
        let dir = tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let err = import_from_repos_json(
            &dir.path().join("no-such.json"),
            &manifest,
            ImportOpts::default(),
        )
        .unwrap_err();
        assert!(matches!(err, ImportError::Io { .. }));
    }

    #[test]
    fn import_malformed_surfaces_parse_error() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(&input, "not json at all");
        let err = import_from_repos_json(&input, &manifest, ImportOpts::default()).unwrap_err();
        assert!(matches!(err, ImportError::Parse { .. }));
    }

    #[test]
    fn import_rejects_path_with_separator_into_failed() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(
            &input,
            r#"[
                {"url": "https://x/a.git", "path": "../escape"},
                {"url": "https://x/b.git", "path": "good"}
            ]"#,
        );
        let plan =
            import_from_repos_json(&input, &manifest, ImportOpts { dry_run: false }).unwrap();
        assert_eq!(plan.imported.len(), 1, "only the good row imports");
        assert_eq!(plan.imported[0].path, "good");
        assert_eq!(plan.failed.len(), 1, "the traversal-bearing row goes to failed");
        assert_eq!(plan.failed[0].path, "../escape");
        // v1.2.0: slash-paths accepted, but `..` segments still rejected.
        // Diagnostic now leads with the dot-segment failure mode.
        assert!(
            plan.failed[0].error.contains(".."),
            "error must explain the rejection: {}",
            plan.failed[0].error,
        );
        // Manifest must NOT contain a row for `../escape`.
        let events = manifest::read_all(&manifest).unwrap();
        assert!(
            events.iter().all(|e| !matches!(e, Event::Add { path, .. } if path == "../escape")),
            "no Event::Add may be written for a rejected path",
        );
    }

    #[test]
    fn import_rejects_dot_dotdot_uppercase_empty() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(
            &input,
            r#"[
                {"url": "u", "path": "."},
                {"url": "u", "path": ".."},
                {"url": "u", "path": "Foo"},
                {"url": "u", "path": ""},
                {"url": "u", "path": "foo\\bar"}
            ]"#,
        );
        let plan = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: true }).unwrap();
        assert_eq!(plan.imported.len(), 0);
        assert_eq!(plan.failed.len(), 5);
    }

    #[test]
    fn property_every_imported_entry_matches_classify() {
        let dir = tempdir().unwrap();
        let input = dir.path().join("REPOS.json");
        let manifest = dir.path().join(".grex/events.jsonl");
        write_json(
            &input,
            r#"[
                {"url": "https://a/a.git", "path": "a"},
                {"url": "git@b:b/b.git", "path": "b"},
                {"url": "not-a-url", "path": "c"},
                {"url": "", "path": "d"},
                {"url": "git://h/x.git", "path": "e"}
            ]"#,
        );
        let plan = import_from_repos_json(&input, &manifest, ImportOpts { dry_run: true }).unwrap();
        assert_eq!(plan.imported.len(), 5);
        for entry in &plan.imported {
            assert_eq!(entry.kind, classify(&entry.url));
        }
    }
}
