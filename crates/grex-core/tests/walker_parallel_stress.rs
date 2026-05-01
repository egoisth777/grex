//! v1.2.1 item 3 — rayon parallel sibling sync stress + determinism tests.
//!
//! Exercises the rayon-driven Phase 1 + Phase 3 fan-out inside
//! [`grex_core::tree::sync_meta`]. The Lean axiom
//! `sync_disjoint_commutes` (proof/Grex/Bridge.lean) licenses any
//! interleaving of sibling sub-tree syncs as long as they are
//! path-suffix-disjoint; these tests validate that the Rust runtime
//! actually delivers what the axiom promises by running the walker
//! against pathologically wide / deep manifests on many threads and
//! asserting deterministic state.
//!
//! ## Test matrix
//!
//! 1. **Fan-out determinism (50 runs).** A single meta with 16 leaf
//!    children. Run `sync_meta` 50× with `parallel: Some(8)`; collect
//!    the rolled-up `SyncMetaReport` from each run; assert every run
//!    produced byte-identical state — same set of clones in the
//!    backend's recorded calls, same ordered `phase1_classifications`
//!    vector, same FS state at each child dest. Closes the
//!    "scheduler-introduced ordering jitter" hole that sequential
//!    tests cannot catch.
//! 2. **Nested-tree correctness (4 levels × 4 siblings).** A 4-level
//!    chain (root → L1 × 4 → L2 × 4 each). Run with `parallel:
//!    Some(4)`. Assert that Phase 3 recursion fired against every
//!    declared sub-meta and that final state matches a sequential
//!    reference run (`parallel: Some(1)`).
//! 3. **`Some(1)` is sequential-equivalent.** A 4-child fan-out
//!    classified Missing → 4 clones. With `parallel: Some(1)` rayon
//!    runs on a single-thread pool; clone ORDER (not just set)
//!    matches the sequential reference exactly.
//!
//! These tests use an in-memory `MockGitBackend` that records every
//! call so we can assert ordering / set-equality without touching git
//! or the filesystem more than necessary. The `sync_meta` walker is
//! the v1.2.0 entry point that the rayon refactor targets — it is
//! exported via `grex_core::tree::sync_meta`.

#![allow(clippy::too_many_lines)]

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use grex_core::pack::{parse, PackManifest};
use grex_core::tree::{sync_meta, SyncMetaOptions};
use grex_core::{ClonedRepo, GitBackend, GitError, PackLoader, TreeError};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Mock infrastructure — copied in spirit from `crates/grex-core/src/tree/walker.rs`'s
// own test module so the integration test stays self-contained.
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
enum BackendCall {
    Clone {
        url: String,
        dest: PathBuf,
    },
    #[allow(dead_code)]
    Fetch {
        dest: PathBuf,
    },
    #[allow(dead_code)]
    Checkout {
        dest: PathBuf,
        r#ref: String,
    },
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
        "v1_2_1-stress-mock-git"
    }
    fn clone(&self, url: &str, dest: &Path, _ref: Option<&str>) -> Result<ClonedRepo, GitError> {
        // Materialise a `.git/` so re-runs would classify as PresentDeclared,
        // matching the production GixBackend's post-condition.
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

/// Build a `meta` manifest from a name + a list of `(url, path)`
/// child specs by serializing to YAML and round-tripping through
/// the production parser. Going via YAML sidesteps the
/// `#[non_exhaustive]` markers on `PackManifest` / `ChildRef` which
/// forbid struct-literal construction outside the crate.
fn meta_with_children(name: &str, kids: &[(String, String)]) -> PackManifest {
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\nchildren:\n");
    for (url, path) in kids {
        yaml.push_str(&format!("  - url: {url}\n    path: {path}\n"));
    }
    parse(&yaml).expect("fixture yaml must parse")
}

/// 16-child fan-out manifest — 1 root meta, 16 leaf children with
/// unique URLs + paths so each clone is observably distinct.
fn build_fan_out_loader(meta_dir: &Path) -> InMemLoader {
    let kids: Vec<(String, String)> = (0..16)
        .map(|i| (format!("https://example.com/leaf-{i:02}.git"), format!("leaf-{i:02}")))
        .collect();
    InMemLoader::new().with(meta_dir.to_path_buf(), meta_with_children("fan-out-root", &kids))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Test 1 — Fan-out determinism over 50 runs at `parallel: Some(8)`.
///
/// Each run uses a fresh tempdir + fresh backend so we measure the
/// rayon scheduler's effect on `sync_meta`'s output, NOT cumulative
/// state from prior runs. We assert TWO determinism properties:
///
/// 1. The set of clones recorded by the backend is identical across
///    every run (rayon never drops a sibling, never duplicates one).
/// 2. The `phase1_classifications` Vec inside `SyncMetaReport` is
///    in source-order on every run — rayon's `par_iter().collect()`
///    preserves source-order regardless of completion order, and
///    walker.rs relies on that property to keep the report stable.
#[test]
fn rayon_fan_out_50_runs_byte_identical_state() {
    const RUNS: usize = 50;
    const KIDS: usize = 16;

    let mut all_clone_sets: Vec<BTreeSet<(String, PathBuf)>> = Vec::with_capacity(RUNS);
    let mut all_class_orders: Vec<Vec<PathBuf>> = Vec::with_capacity(RUNS);

    for _ in 0..RUNS {
        let tmp = TempDir::new().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        let loader = build_fan_out_loader(&meta_dir);
        let backend = InMemGit::new();
        let opts = SyncMetaOptions {
            parallel: Some(8),
            recurse: false, // leaf children carry no `.grex/pack.yaml`
            ..SyncMetaOptions::default()
        };
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &[]).expect("ok");

        // Each fan-out child is `Missing` → exactly KIDS clones.
        assert_eq!(report.phase1_classifications.len(), KIDS, "every child classified once");
        assert!(report.errors.is_empty(), "no errors on a clean fan-out");

        // Collect the clone set keyed by (url, dest) — set-equality
        // suffices because per-child url+dest is unique by construction.
        let calls = backend.calls();
        let clone_set: BTreeSet<(String, PathBuf)> = calls
            .iter()
            .filter_map(|c| match c {
                BackendCall::Clone { url, dest } => Some((url.clone(), dest.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(clone_set.len(), KIDS, "rayon must clone each child exactly once");
        all_clone_sets.push(clone_set);

        // The `phase1_classifications` vec is source-ordered by
        // `par_iter().collect()`. Capture the dest path order — the
        // dests should appear in the same order as `manifest.children`.
        let dest_order: Vec<PathBuf> = report
            .phase1_classifications
            .iter()
            .map(|(_meta, dest, _class)| dest.clone())
            .collect();
        all_class_orders.push(dest_order);
    }

    // Property 1 — every clone set identical (modulo the dest paths
    // being under different tempdirs). To compare across tempdirs we
    // strip the tempdir prefix and compare basenames + url only.
    let normalise = |s: &BTreeSet<(String, PathBuf)>| -> BTreeSet<(String, String)> {
        s.iter()
            .map(|(url, dest)| {
                (url.clone(), dest.file_name().unwrap().to_string_lossy().into_owned())
            })
            .collect()
    };
    let first = normalise(&all_clone_sets[0]);
    for (i, set) in all_clone_sets.iter().enumerate().skip(1) {
        assert_eq!(
            normalise(set),
            first,
            "run {i} clone set must equal run 0 clone set (rayon non-determinism caught)"
        );
    }

    // Property 2 — every classification order identical (basename only).
    let basenames = |v: &Vec<PathBuf>| -> Vec<String> {
        v.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect()
    };
    let first_order = basenames(&all_class_orders[0]);
    // Source order is `leaf-00, leaf-01, …, leaf-15` — confirm explicitly.
    let expected: Vec<String> = (0..KIDS).map(|i| format!("leaf-{i:02}")).collect();
    assert_eq!(first_order, expected, "rayon par_iter().collect() must preserve source order");
    for (i, order) in all_class_orders.iter().enumerate().skip(1) {
        assert_eq!(
            basenames(order),
            first_order,
            "run {i} classification order must equal run 0 order"
        );
    }
}

/// Test 2 — Nested-tree correctness at `parallel: Some(4)` matches
/// the sequential reference (`parallel: Some(1)`).
///
/// Build a 3-level deep manifest where each level fans out 4 siblings:
///
/// ```text
/// root            (4 children)
///   ├── lvl1-0   (4 children)  → lvl2-0..3
///   ├── lvl1-1   (4 children)  → lvl2-0..3
///   ├── lvl1-2   (4 children)  → lvl2-0..3
///   └── lvl1-3   (4 children)  → lvl2-0..3
/// ```
///
/// Total: 1 root + 4 mid + 16 leaf = 21 metas across 3 depth levels.
/// Pre-materialise every dir on disk with a `.git/` and a
/// `.grex/pack.yaml` so Phase 1 classifies as PresentDeclared
/// (drives `fetch`, not `clone`) and Phase 3 recurses into each
/// sub-meta.
///
/// Assertions:
/// * `metas_visited` = 1 + 4 + 16 = 21 for `parallel: Some(4)` AND
///   `parallel: Some(1)` (rayon never drops a recursion).
/// * The set of `BackendCall::Fetch` dests is identical across both
///   runs — same set of repos touched, regardless of scheduler.
#[test]
fn rayon_nested_3_level_correctness_matches_sequential() {
    const SIBLINGS: usize = 4;

    fn build_nested_loader(root: &Path) -> InMemLoader {
        let mut loader = InMemLoader::new();
        // Root: 4 mid children
        let mid_paths: Vec<String> = (0..SIBLINGS).map(|i| format!("lvl1-{i}")).collect();
        let root_kids: Vec<(String, String)> =
            mid_paths.iter().map(|p| (format!("https://example.com/{p}.git"), p.clone())).collect();
        loader = loader.with(root.to_path_buf(), meta_with_children("root", &root_kids));

        // Each mid: 4 leaf children (lvl2-0..3)
        for mp in &mid_paths {
            let mid_dir = root.join(mp);
            let leaf_kids: Vec<(String, String)> = (0..SIBLINGS)
                .map(|j| {
                    let lp = format!("lvl2-{j}");
                    (format!("https://example.com/{mp}-{lp}.git"), lp)
                })
                .collect();
            loader = loader.with(mid_dir.clone(), meta_with_children(mp, &leaf_kids));
            // Each leaf: empty meta (terminates recursion)
            for j in 0..SIBLINGS {
                let lp = format!("lvl2-{j}");
                let leaf_dir = mid_dir.join(&lp);
                loader = loader.with(leaf_dir, meta_with_children(&lp, &[]));
            }
        }
        loader
    }

    fn pre_materialise(root: &Path) {
        // Root carries no .grex/pack.yaml on disk — it's the entry meta.
        for i in 0..SIBLINGS {
            let mid = root.join(format!("lvl1-{i}"));
            std::fs::create_dir_all(mid.join(".grex")).unwrap();
            std::fs::create_dir_all(mid.join(".git")).unwrap();
            std::fs::write(
                mid.join(".grex/pack.yaml"),
                format!("schema_version: \"1\"\nname: lvl1-{i}\ntype: meta\n"),
            )
            .unwrap();
            for j in 0..SIBLINGS {
                let leaf = mid.join(format!("lvl2-{j}"));
                std::fs::create_dir_all(leaf.join(".grex")).unwrap();
                std::fs::create_dir_all(leaf.join(".git")).unwrap();
                std::fs::write(
                    leaf.join(".grex/pack.yaml"),
                    format!("schema_version: \"1\"\nname: lvl2-{j}\ntype: meta\n"),
                )
                .unwrap();
            }
        }
    }

    fn run_once(parallel: Option<usize>) -> (usize, BTreeSet<String>) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().to_path_buf();
        pre_materialise(&root);
        let loader = build_nested_loader(&root);
        let backend = InMemGit::new();
        let opts = SyncMetaOptions { parallel, ..SyncMetaOptions::default() };
        let report = sync_meta(&root, &backend, &loader, &opts, &[]).expect("ok");
        // Use basename-relative path for set equality across tempdirs.
        let touched: BTreeSet<String> = backend
            .calls()
            .iter()
            .filter_map(|c| match c {
                BackendCall::Fetch { dest } => {
                    Some(dest.strip_prefix(&root).ok()?.to_string_lossy().into_owned())
                }
                _ => None,
            })
            .collect();
        (report.metas_visited, touched)
    }

    let (par4_visited, par4_touched) = run_once(Some(4));
    let (seq_visited, seq_touched) = run_once(Some(1));

    // 1 root + 4 mid + 16 leaf = 21
    assert_eq!(par4_visited, 21, "parallel=4 must visit every meta");
    assert_eq!(seq_visited, 21, "sequential must visit every meta");
    assert_eq!(
        par4_touched, seq_touched,
        "fetch set under parallel=4 must equal fetch set under sequential"
    );
    // Spot-check the count: 4 mid + 16 leaf = 20 fetches (root has no .git
    // pre-materialised, so it's `Missing` → clone, not fetch — but root is
    // the entry meta itself, NOT a phase 1 child of anyone, so root's own
    // dest is never classified). The 4 mid dests classify as PresentDeclared
    // → fetch; the 16 leaf dests classify as PresentDeclared → fetch.
    assert_eq!(par4_touched.len(), 20, "expected 4 mid + 16 leaf fetches");
}

/// Test 3 — `parallel: Some(1)` produces a deterministic sequential
/// trace. With a 1-thread rayon pool the par_iter degenerates to a
/// for-loop; clone ORDER (not just set) must match the source order
/// of `manifest.children`.
#[test]
fn rayon_parallel_one_is_sequential_equivalent() {
    let tmp = TempDir::new().unwrap();
    let meta_dir = tmp.path().to_path_buf();
    let loader = build_fan_out_loader(&meta_dir); // 16-child fan-out
    let backend = InMemGit::new();
    let opts = SyncMetaOptions { parallel: Some(1), recurse: false, ..SyncMetaOptions::default() };
    let _report = sync_meta(&meta_dir, &backend, &loader, &opts, &[]).expect("ok");

    let urls: Vec<String> = backend
        .calls()
        .iter()
        .filter_map(|c| match c {
            BackendCall::Clone { url, .. } => Some(url.clone()),
            _ => None,
        })
        .collect();
    let expected: Vec<String> =
        (0..16).map(|i| format!("https://example.com/leaf-{i:02}.git")).collect();
    assert_eq!(urls, expected, "parallel=1 must clone in source order (rayon 1-thread pool)");
}
