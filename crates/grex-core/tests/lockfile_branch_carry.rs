//! v1.3.1 B14 — lockfile `branch` field carries `manifest.ref`.
//!
//! Regression cover for the v1.3.0 dogfood finding: every lockfile
//! entry was emitted with `branch: ""` regardless of the parent
//! manifest's `ref:` value. The fix lives in
//! `crates/grex-core/src/lockfile/writer.rs::write_entry`, exposed at
//! the crate root as
//! [`grex_core::lockfile::write_entry_from_child`].
//!
//! These tests are pure-data-transform — no walker, no fixture beyond
//! a small YAML manifest that the public `pack::parse` API turns into
//! `ChildRef` values. They mirror the Lean theorem
//! `Grex.Lockfile.lockfile_branch_mirrors_manifest_ref`
//! (`proof/Grex/Lockfile.lean`), which states: for every child
//! reference `c`, the lockfile entry produced from `c` has
//! `branch == branchOf c.ref` (with `branchOf : Option String → String`
//! defined as `None ↦ "", Some s ↦ s`).
//!
//! Why parse a YAML manifest instead of constructing `ChildRef`
//! directly: `ChildRef` is `#[non_exhaustive]`, so struct-literal
//! construction is forbidden across the crate boundary. Driving the
//! tests through `pack::parse` is the public, future-proof entry
//! point — and it doubles as a smoke check that `ref:` survives
//! YAML → manifest in-memory representation.
//!
//! Coverage:
//!
//! | Case | `manifest.ref`                     | Expected `entry.branch` |
//! |------|------------------------------------|-------------------------|
//! | 1    | `Some("main")`                     | `"main"`                |
//! | 2    | `Some("v1.0.0")` (tag)             | `"v1.0.0"`              |
//! | 3    | `Some("<40-char-sha>")`            | `"<40-char-sha>"`       |
//! | 4    | `None`                             | `""`                    |

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, TimeZone, Utc};
use grex_core::lockfile::{branch_of, write_entry_from_child};
use grex_core::pack::{parse, ChildRef};
use grex_core::{
    ClonedRepo, FsPackLoader, GitBackend, GitError, PackLoader, PackManifest, TreeError, Walker,
};
use tempfile::TempDir;

fn ts() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 5, 2, 10, 0, 0).unwrap()
}

/// Build a single-child meta manifest YAML and return the parsed
/// child. Going through `pack::parse` keeps the tests honest about
/// the public API surface — and avoids the `#[non_exhaustive]`
/// struct-literal restriction that would otherwise block this file
/// from constructing `ChildRef` values directly.
fn parse_single_child(url: &str, manifest_ref: Option<&str>) -> ChildRef {
    let ref_line = manifest_ref.map_or(String::new(), |r| format!("    ref: {r}\n"));
    let yaml = format!(
        "schema_version: \"1\"\n\
         name: parent\n\
         type: meta\n\
         children:\n\
         \x20\x20- url: {url}\n\
         {ref_line}",
    );
    let manifest = parse(&yaml).expect("fixture YAML must parse");
    assert_eq!(manifest.children.len(), 1, "fixture invariant: one child");
    manifest.children.into_iter().next().expect("child exists")
}

/// Case 1 — branch ref. `ref: main` in the parent manifest must land
/// in the lockfile entry as `branch: "main"`. This is the v1.3.0
/// regression case verbatim: dogfood found `branch: ""` here.
#[test]
fn branch_ref_main_carried_into_lockfile_entry() {
    let child = parse_single_child("https://example.invalid/alpha.git", Some("main"));
    let entry = write_entry_from_child(&child, "deadbeef", ts(), "h", "1");
    assert_eq!(entry.branch, "main", "B14: manifest `ref: main` must land in `LockEntry.branch`");
    // sanity: id derives from URL tail; sha + schema_version pass through.
    assert_eq!(entry.id, "alpha");
    assert_eq!(entry.sha, "deadbeef");
    assert_eq!(entry.schema_version, "1");
}

/// Case 2 — tag ref. `ref: v1.0.0` is a tag, not a branch, but the
/// lockfile schema records refs verbatim (per
/// `.omne/lockfile.md` §"branch field": "ref-as-recorded, may be
/// branch / tag / sha"). The carry must be lossless.
#[test]
fn tag_ref_v1_0_0_carried_verbatim() {
    let child = parse_single_child("https://example.invalid/beta.git", Some("v1.0.0"));
    let entry = write_entry_from_child(&child, "cafebabe", ts(), "h", "1");
    assert_eq!(entry.branch, "v1.0.0", "B14: tag refs must carry verbatim");
}

/// Case 3 — SHA ref. A 40-char hex SHA is technically not a branch
/// name, but the lockfile carries it as the resolved-ref column so
/// downstream consumers (doctor, ls) can show what the operator
/// pinned. Verifies the writer does no mangling of the input string.
#[test]
fn sha_ref_carried_as_branch_string_unchanged() {
    let sha = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(sha.len(), 40, "fixture invariant: 40-char SHA");
    let child = parse_single_child("https://example.invalid/gamma.git", Some(sha));
    let entry = write_entry_from_child(&child, sha, ts(), "h", "1");
    assert_eq!(entry.branch, sha, "B14: SHA refs must round-trip without mangling");
}

/// Case 4 — explicit no-ref. A manifest child without `ref:` must
/// yield `branch: ""` (Lean: `branchOf None = ""`). This pins the
/// empty-default rule so a future refactor cannot accidentally widen
/// the schema (e.g. to `Option<String>`) without revisiting the
/// fidelity contract.
#[test]
fn missing_ref_yields_empty_branch() {
    let child = parse_single_child("https://example.invalid/delta.git", None);
    assert!(child.r#ref.is_none(), "fixture invariant: child has no manifest ref");
    let entry = write_entry_from_child(&child, "abc", ts(), "h", "1");
    assert_eq!(entry.branch, "", "B14: missing manifest ref ↦ empty branch");
}

/// Direct exercise of the `branch_of` lift, mirroring the Lean
/// `branchOf` def. Belt-and-braces: the writer's behaviour is also
/// covered above, but pinning `branch_of` independently makes future
/// refactors of `write_entry` safer (the helper can be reused in
/// other lockfile-write paths without re-deriving the rule).
#[test]
fn branch_of_matches_lean_branch_of_total() {
    assert_eq!(branch_of(None), "");
    assert_eq!(branch_of(Some("main")), "main");
    assert_eq!(branch_of(Some("v1.2.3")), "v1.2.3");
    assert_eq!(branch_of(Some("")), "");
}

// ---------------------------------------------------------------------------
// Walker-driven plumbing test (B14 wiring end-to-end)
// ---------------------------------------------------------------------------
//
// The pure-helper coverage above proves `write_entry_from_child` and
// `branch_of` are correct in isolation. The piece v1.3.0 actually
// regressed was the *plumbing*: the parent manifest's `ref:` value
// was dropped before reaching the lockfile writer, so the entry was
// written with `branch: ""` regardless of the helper's contract.
//
// This test pins the wiring: the walker MUST capture
// `child.r#ref` into `PackNode::manifest_ref` so the sync orchestrator
// (`crates/grex-core/src/sync.rs::run_actions`) can hand it to
// `upsert_lock_entry`, where it lands in `LockEntry.branch` via
// `branch_of`. We exercise just the walker → graph step here because
// it is the load-bearing slice; the upsert side is covered by the
// in-crate tests `upsert_lock_entry_*` in `sync.rs`.

/// In-memory loader: maps `path → manifest`.
struct MockLoader {
    manifests: HashMap<PathBuf, PackManifest>,
}

impl MockLoader {
    fn with(path: PathBuf, manifest: PackManifest) -> Self {
        let mut m = HashMap::new();
        m.insert(path, manifest);
        Self { manifests: m }
    }
    fn add(mut self, path: PathBuf, manifest: PackManifest) -> Self {
        self.manifests.insert(path, manifest);
        self
    }
}

impl PackLoader for MockLoader {
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError> {
        self.manifests
            .get(path)
            .cloned()
            .ok_or_else(|| TreeError::ManifestNotFound(path.to_path_buf()))
    }
}

/// Mock git backend: records calls; clone creates the dest dir so
/// subsequent walker checks see a hydrated child.
#[allow(dead_code)]
struct MockGitBackend {
    calls: Mutex<Vec<String>>,
}

impl MockGitBackend {
    fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()) }
    }
}

impl GitBackend for MockGitBackend {
    fn name(&self) -> &'static str {
        "mock-git-b14"
    }
    fn clone(
        &self,
        _url: &str,
        dest: &Path,
        _ref: Option<&str>,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<ClonedRepo, GitError> {
        self.calls.lock().unwrap().push("clone".into());
        fs::create_dir_all(dest).unwrap();
        Ok(ClonedRepo { path: dest.to_path_buf(), head_sha: "0".repeat(40) })
    }
    fn fetch(
        &self,
        _dest: &Path,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<(), GitError> {
        Ok(())
    }
    fn checkout(
        &self,
        _dest: &Path,
        _ref: &str,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<(), GitError> {
        Ok(())
    }
    fn head_sha(&self, _dest: &Path) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

fn meta_yaml_one_child(child_url: &str, child_path: &str, child_ref: Option<&str>) -> String {
    let mut s = "schema_version: \"1\"\nname: parent\ntype: meta\nchildren:\n".to_string();
    s.push_str(&format!("  - url: {child_url}\n    path: {child_path}\n"));
    if let Some(r) = child_ref {
        s.push_str(&format!("    ref: {r}\n"));
    }
    s
}

fn child_yaml(name: &str) -> String {
    format!("schema_version: \"1\"\nname: {name}\ntype: declarative\n")
}

/// AC: when the parent manifest declares `ref: main` for a child,
/// `PackNode::manifest_ref` for that child carries `Some("main")` —
/// the load-bearing capture for v1.3.1 B14. Without this, the
/// `upsert_lock_entry` site has nothing to feed into `branch_of` and
/// must fall back to the empty-default — exactly the regressed v1.3.0
/// behaviour.
#[test]
fn walker_captures_manifest_ref_for_branch_carry() {
    let ws = TempDir::new().unwrap();
    let root_path = ws.path().join("parent");
    fs::create_dir_all(&root_path).unwrap();
    let child_dest = ws.path().join("alpha");

    let root_manifest =
        parse(&meta_yaml_one_child("https://example.invalid/alpha.git", "alpha", Some("main")))
            .unwrap();
    let child_manifest = parse(&child_yaml("alpha")).unwrap();
    let loader =
        MockLoader::with(root_path.clone(), root_manifest).add(child_dest.clone(), child_manifest);
    let backend = MockGitBackend::new();

    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root_path).expect("walk succeeds");

    let nodes = graph.nodes();
    assert_eq!(nodes.len(), 2, "root + one child");
    // Root: no parent ChildRef → no manifest_ref.
    assert!(nodes[0].manifest_ref.is_none(), "root has no parent ref");
    // Child: parent declared `ref: main` → mirrored verbatim onto
    // `PackNode.manifest_ref` so the sync upsert path can feed it
    // through `branch_of` into `LockEntry.branch`.
    assert_eq!(
        nodes[1].manifest_ref.as_deref(),
        Some("main"),
        "B14: walker must capture child.ref into PackNode.manifest_ref"
    );
    // And the helper round-trip lands at `"main"` — the very value
    // the sync orchestrator writes into the lockfile.
    assert_eq!(branch_of(nodes[1].manifest_ref.as_deref()), "main");
}

/// AC: `ref: v1.0.0` (a tag, not a branch) round-trips verbatim
/// through the walker into `PackNode::manifest_ref`. Pins the
/// `lossless-carry` invariant — the walker must not interpret the
/// string, only forward it.
#[test]
fn walker_captures_manifest_ref_tag_value() {
    let ws = TempDir::new().unwrap();
    let root_path = ws.path().join("parent");
    fs::create_dir_all(&root_path).unwrap();
    let child_dest = ws.path().join("beta");

    let root_manifest =
        parse(&meta_yaml_one_child("https://example.invalid/beta.git", "beta", Some("v1.0.0")))
            .unwrap();
    let child_manifest = parse(&child_yaml("beta")).unwrap();
    let loader =
        MockLoader::with(root_path.clone(), root_manifest).add(child_dest.clone(), child_manifest);
    let backend = MockGitBackend::new();

    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root_path).expect("walk succeeds");

    assert_eq!(
        graph.nodes()[1].manifest_ref.as_deref(),
        Some("v1.0.0"),
        "B14: tag refs carry verbatim into PackNode.manifest_ref"
    );
}

/// AC: a child with no `ref:` produces `PackNode::manifest_ref =
/// None`, which the helper lifts to `""` — preserving the
/// empty-default rule that pre-existed v1.3.0.
#[test]
fn walker_no_manifest_ref_yields_none_node_ref() {
    let ws = TempDir::new().unwrap();
    let root_path = ws.path().join("parent");
    fs::create_dir_all(&root_path).unwrap();
    let child_dest = ws.path().join("delta");

    let root_manifest =
        parse(&meta_yaml_one_child("https://example.invalid/delta.git", "delta", None)).unwrap();
    let child_manifest = parse(&child_yaml("delta")).unwrap();
    let loader =
        MockLoader::with(root_path.clone(), root_manifest).add(child_dest.clone(), child_manifest);
    let backend = MockGitBackend::new();

    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root_path).expect("walk succeeds");

    assert!(graph.nodes()[1].manifest_ref.is_none(), "no manifest ref ⇒ None on PackNode");
    assert_eq!(branch_of(graph.nodes()[1].manifest_ref.as_deref()), "");
}

// Silence unused-import warnings for the cargo lint pass when the
// module compiles standalone — `FsPackLoader` is referenced only via
// its trait above (PackLoader). Keep the explicit `use` so future
// authors can swap to the real loader without hunting imports.
#[allow(dead_code)]
fn _silence_fs_pack_loader_unused(_: FsPackLoader) {}
