//! v1.3.1 (B4) — walker dry-run gate regression suite.
//!
//! Pins the `dry_run = true` invariants for `sync_meta`:
//!
//! 1. `dry_run = true` on a meta with one declared child → no clone
//!    subprocess fired (recorded by the in-memory `InMemGit` mock; no
//!    `Clone { .. }` calls), no `<dest>/.git` directory created on
//!    disk, the report carries exactly one `DryRunWouldCloneRecord`
//!    whose `id` equals the on-disk folder name.
//! 2. `dry_run = true` on a nested meta → walk completes, no FS
//!    mutation, multiple `DryRunWouldCloneRecord`s accumulate across
//!    Phase 3 recursion frames (one per discoverable child).
//! 3. `dry_run = false` on the same fixture → the walker DOES fire
//!    Phase 1 clones (sanity check that the gate is not always-on);
//!    the in-memory backend records `Clone { .. }` calls.
//!
//! The suite uses the same `InMemGit` mock pattern as
//! `cancellation_no_deadlock.rs` so no real network call ever runs.
//! This is the side-effect-free contract Lean theorem
//! `Grex.Walker.dry_run_no_side_effects` formalises.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use grex_core::pack::{parse, PackManifest};
use grex_core::tree::{sync_meta, SyncMetaOptions};
use grex_core::{ClonedRepo, GitBackend, GitError, PackLoader, TreeError};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Mock infrastructure — mirrors `cancellation_no_deadlock.rs`.
// ---------------------------------------------------------------------------

struct InMemLoader {
    manifests: HashMap<PathBuf, PackManifest>,
}

impl InMemLoader {
    fn new() -> Self {
        Self { manifests: HashMap::new() }
    }
    fn with(mut self, dir: impl Into<PathBuf>, m: PackManifest) -> Self {
        self.manifests.insert(dir.into(), m);
        self
    }
}

impl PackLoader for InMemLoader {
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError> {
        self.manifests
            .get(path)
            .cloned()
            .ok_or_else(|| TreeError::ManifestNotFound(path.to_path_buf()))
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum BackendCall {
    Clone { url: String, dest: PathBuf },
    Fetch { dest: PathBuf },
    Checkout { dest: PathBuf, r#ref: String },
}

struct InMemGit {
    calls: Mutex<Vec<BackendCall>>,
}

impl InMemGit {
    fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()) }
    }
    fn calls(&self) -> Vec<BackendCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl GitBackend for InMemGit {
    fn name(&self) -> &'static str {
        "v1_3_1-b4-dry-run-mock-git"
    }
    fn clone(&self, url: &str, dest: &Path, _ref: Option<&str>) -> Result<ClonedRepo, GitError> {
        std::fs::create_dir_all(dest.join(".git")).unwrap();
        self.calls
            .lock()
            .unwrap()
            .push(BackendCall::Clone { url: url.to_string(), dest: dest.to_path_buf() });
        Ok(ClonedRepo { path: dest.to_path_buf(), head_sha: "0".repeat(40) })
    }
    fn fetch(&self, dest: &Path) -> Result<(), GitError> {
        self.calls.lock().unwrap().push(BackendCall::Fetch { dest: dest.to_path_buf() });
        Ok(())
    }
    fn checkout(&self, dest: &Path, r#ref: &str) -> Result<(), GitError> {
        self.calls
            .lock()
            .unwrap()
            .push(BackendCall::Checkout { dest: dest.to_path_buf(), r#ref: r#ref.to_string() });
        Ok(())
    }
    fn head_sha(&self, _dest: &Path) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

fn meta_with_children(name: &str, kids: &[(String, String)]) -> PackManifest {
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\nchildren:\n");
    for (url, path) in kids {
        yaml.push_str(&format!("  - url: {url}\n    path: {path}\n"));
    }
    parse(&yaml).expect("fixture yaml must parse")
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// Case 1 — single-child dry-run is side-effect free.
#[test]
fn dry_run_single_child_no_clone_no_fs_mutation() {
    let tmp = TempDir::new().expect("tempdir");
    let root_dir = tmp.path().to_path_buf();
    let url = "https://example.com/warp-cfgs.git".to_string();
    let path = "warp-cfgs".to_string();

    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[(url.clone(), path.clone())]));
    let backend = InMemGit::new();

    let mut opts = SyncMetaOptions::default();
    opts.dry_run = true;

    let report =
        sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("dry-run walk must succeed");

    // No clone subprocess fired.
    let calls = backend.calls();
    assert!(
        calls.iter().all(|c| !matches!(c, BackendCall::Clone { .. })),
        "dry_run = true must NOT fire Clone, got {calls:?}"
    );
    // No <dest>/.git directory materialised.
    let dest = root_dir.join(&path);
    assert!(!dest.join(".git").exists(), "dry_run = true must NOT create <dest>/.git directory");
    // Report carries exactly one DryRunWouldCloneRecord with the right id.
    assert_eq!(
        report.dry_run_would_clone.len(),
        1,
        "expected one would-clone record, got {:?}",
        report.dry_run_would_clone
    );
    let rec = &report.dry_run_would_clone[0];
    assert_eq!(rec.id, "warp-cfgs", "id must equal on-disk folder name");
    assert_eq!(rec.url, url);
    // No events.jsonl appended (file should not exist or be empty —
    // the walker emits to the in-memory report, not events.jsonl).
    let events_log = root_dir.join(".grex").join("events.jsonl");
    let bytes = std::fs::read(&events_log).unwrap_or_default();
    assert!(
        bytes.is_empty(),
        "dry_run = true must NOT write events.jsonl, got {} bytes",
        bytes.len()
    );
}

/// Case 2 — nested-meta dry-run aggregates would-clone records across
/// Phase 3 recursion frames.
#[test]
fn dry_run_nested_meta_aggregates_records_no_fs_mutation() {
    let tmp = TempDir::new().expect("tempdir");
    let root_dir = tmp.path().to_path_buf();
    // root → meta-a (sub-meta) with two leaf children
    let a_dir = root_dir.join("a");
    let url_a = "https://example.com/a.git".to_string();
    let url_b = "https://example.com/b.git".to_string();
    let url_c = "https://example.com/c.git".to_string();

    // Pre-populate the sub-meta's pack.yaml + .git so Phase 1 classifies
    // it as PresentDeclared. The dry-run gate applies to PresentDeclared
    // too — we want to assert the report carries records for `a` AND
    // its children even when `a` is on-disk.
    std::fs::create_dir_all(a_dir.join(".grex")).unwrap();
    std::fs::create_dir_all(a_dir.join(".git")).unwrap();
    std::fs::write(a_dir.join(".grex/pack.yaml"), "schema_version: \"1\"\nname: a\ntype: meta\n")
        .unwrap();

    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[(url_a.clone(), "a".to_string())]))
        .with(
            a_dir.clone(),
            meta_with_children(
                "a",
                &[(url_b.clone(), "b".to_string()), (url_c.clone(), "c".to_string())],
            ),
        );
    let backend = InMemGit::new();

    let mut opts = SyncMetaOptions::default();
    opts.dry_run = true;

    let report =
        sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("dry-run walk must succeed");

    // No FS mutation under leaves.
    assert!(!root_dir.join("a/b").exists(), "dry_run = true must NOT create child 'b' on disk");
    assert!(!root_dir.join("a/c").exists(), "dry_run = true must NOT create child 'c' on disk");
    // No backend mutation calls (only Clone/Fetch/Checkout count).
    let calls = backend.calls();
    let mutating: Vec<&BackendCall> = calls
        .iter()
        .filter(|c| matches!(c, BackendCall::Clone { .. } | BackendCall::Fetch { .. }))
        .collect();
    assert!(mutating.is_empty(), "dry_run = true must NOT fire any Clone/Fetch, got {mutating:?}");
    // Report aggregates records across both meta frames: 1 for `a`
    // (parent-side) + 2 for `b`/`c` (Phase 3 recursion). Order is
    // walker-dependent; we just check the set.
    let ids: std::collections::HashSet<&str> =
        report.dry_run_would_clone.iter().map(|r| r.id.as_str()).collect();
    assert!(
        ids.contains("a") && ids.contains("b") && ids.contains("c"),
        "expected ids {{a, b, c}}, got {ids:?}"
    );
}

/// Case 3 — sanity: `dry_run = false` on the same fixture DOES fire
/// Phase 1 clones. Pins that the gate is not stuck in always-skip.
#[test]
fn dry_run_false_still_clones() {
    let tmp = TempDir::new().expect("tempdir");
    let root_dir = tmp.path().to_path_buf();
    let url = "https://example.com/warp-cfgs.git".to_string();
    let path = "warp-cfgs".to_string();

    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[(url.clone(), path.clone())]));
    let backend = InMemGit::new();

    let mut opts = SyncMetaOptions::default();
    opts.dry_run = false;
    // Disable Phase 3 recursion for the fixture (the leaf has no
    // pack.yaml on disk so the test only exercises Phase 1).
    opts.recurse = false;

    let report =
        sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("real walk must succeed");

    let calls = backend.calls();
    assert!(
        calls.iter().any(|c| matches!(c, BackendCall::Clone { .. })),
        "dry_run = false MUST fire Clone, got {calls:?}"
    );
    // Real path does NOT populate `dry_run_would_clone`.
    assert!(
        report.dry_run_would_clone.is_empty(),
        "dry_run = false must leave dry_run_would_clone empty, got {:?}",
        report.dry_run_would_clone
    );
}
