//! Integration: pack-tree walker + graph validators (M3 Stage B slice 4).
//!
//! The tests split into two classes:
//!
//! 1. **Mock-driven** — use `MockLoader` + `MockGitBackend` to exercise
//!    walker logic in isolation from disk / git.
//! 2. **Real-backend** — hydrate from a local bare repo fixture (same
//!    pattern as `tests/git_backend.rs`) to prove end-to-end works.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use grex_core::git::gix_backend::file_url_from_path;
use grex_core::pack::{parse, PackValidationError};
use grex_core::{
    ClonedRepo, EdgeKind, FsPackLoader, GitBackend, GitError, GixBackend, PackLoader, PackManifest,
    TreeError, Walker,
};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Mock infrastructure
// ---------------------------------------------------------------------------

/// In-memory loader: path → canned result. Paths are matched verbatim.
struct MockLoader {
    manifests: HashMap<PathBuf, Result<PackManifest, MockLoaderError>>,
}

/// Cheap cloneable error shim so `MockLoader` can hand out the same error
/// more than once (`TreeError` is not `Clone`).
#[derive(Debug, Clone)]
enum MockLoaderError {
    Parse(String),
}

impl MockLoader {
    fn new() -> Self {
        Self { manifests: HashMap::new() }
    }

    fn with(mut self, path: impl Into<PathBuf>, manifest: PackManifest) -> Self {
        self.manifests.insert(path.into(), Ok(manifest));
        self
    }

    fn with_parse_error(mut self, path: impl Into<PathBuf>, detail: &str) -> Self {
        self.manifests.insert(path.into(), Err(MockLoaderError::Parse(detail.to_string())));
        self
    }
}

impl PackLoader for MockLoader {
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError> {
        match self.manifests.get(path) {
            Some(Ok(m)) => Ok(m.clone()),
            Some(Err(MockLoaderError::Parse(d))) => {
                Err(TreeError::ManifestParse { path: path.to_path_buf(), detail: d.clone() })
            }
            None => Err(TreeError::ManifestNotFound(path.to_path_buf())),
        }
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)] // dest fields are asserted on some tests only; retained for debug output.
enum BackendCall {
    Clone { url: String, dest: PathBuf, r#ref: Option<String> },
    Fetch { dest: PathBuf },
    Checkout { dest: PathBuf, r#ref: String },
    HeadSha { dest: PathBuf },
}

struct MockGitBackend {
    calls: Mutex<Vec<BackendCall>>,
    /// When true, Clone "creates" the dest directory so subsequent file-exists
    /// checks in the walker see the repo as hydrated.
    create_on_clone: bool,
}

impl MockGitBackend {
    fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()), create_on_clone: true }
    }

    fn calls(&self) -> Vec<BackendCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl GitBackend for MockGitBackend {
    fn name(&self) -> &'static str {
        "mock-git"
    }

    fn clone(&self, url: &str, dest: &Path, r#ref: Option<&str>) -> Result<ClonedRepo, GitError> {
        self.calls.lock().unwrap().push(BackendCall::Clone {
            url: url.to_string(),
            dest: dest.to_path_buf(),
            r#ref: r#ref.map(str::to_string),
        });
        if self.create_on_clone {
            fs::create_dir_all(dest).unwrap();
        }
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

    fn head_sha(&self, dest: &Path) -> Result<String, GitError> {
        self.calls.lock().unwrap().push(BackendCall::HeadSha { dest: dest.to_path_buf() });
        Ok("0".repeat(40))
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn pack_yaml(name: &str) -> String {
    format!("schema_version: \"1\"\nname: {name}\ntype: declarative\n")
}

fn pack_yaml_with_children(name: &str, children: &[(&str, &str, Option<&str>)]) -> String {
    let mut s = format!("schema_version: \"1\"\nname: {name}\ntype: meta\nchildren:\n");
    for (url, path, r) in children {
        s.push_str(&format!("  - url: {url}\n    path: {path}\n"));
        if let Some(rr) = r {
            s.push_str(&format!("    ref: {rr}\n"));
        }
    }
    s
}

fn pack_yaml_with_deps(name: &str, deps: &[&str]) -> String {
    let mut s = format!("schema_version: \"1\"\nname: {name}\ntype: declarative\ndepends_on:\n");
    for d in deps {
        s.push_str(&format!("  - {d}\n"));
    }
    s
}

fn parse_pack(yaml: &str) -> PackManifest {
    parse(yaml).expect("fixture yaml must parse")
}

fn mock_git() -> MockGitBackend {
    MockGitBackend::new()
}

// ---------------------------------------------------------------------------
// Walker tests — mock driven
// ---------------------------------------------------------------------------

#[test]
fn walk_single_pack_no_children() {
    let root_path = PathBuf::from("/virt/root");
    let loader = MockLoader::new().with(root_path.clone(), parse_pack(&pack_yaml("solo")));
    let backend = mock_git();
    let workspace = PathBuf::from("/virt/ws");

    let walker = Walker::new(&loader, &backend, workspace);
    let graph = walker.walk(&root_path).expect("walk");
    assert_eq!(graph.nodes().len(), 1);
    assert_eq!(graph.edges().len(), 0);
    assert_eq!(graph.root().name, "solo");
}

#[test]
fn walk_two_level_children() {
    let ws = TempDir::new().unwrap();
    let root_path = ws.path().join("root");
    fs::create_dir_all(&root_path).unwrap();
    let child_a_path = ws.path().join("a");
    let child_b_path = ws.path().join("b");

    let root_yaml = pack_yaml_with_children(
        "root",
        &[("git://x/a.git", "a", None), ("git://x/b.git", "b", None)],
    );
    let loader = MockLoader::new()
        .with(root_path.clone(), parse_pack(&root_yaml))
        .with(child_a_path.clone(), parse_pack(&pack_yaml("a")))
        .with(child_b_path.clone(), parse_pack(&pack_yaml("b")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root_path).expect("walk");

    assert_eq!(graph.nodes().len(), 3);
    assert_eq!(graph.edges().iter().filter(|e| e.kind == EdgeKind::Child).count(), 2);
    assert_eq!(graph.root().name, "root");
    let kids: Vec<&str> = graph.children_of(0).map(|n| n.name.as_str()).collect();
    assert_eq!(kids, vec!["a", "b"]);
}

#[test]
fn walk_three_level_nested_via_mock() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");
    let b = ws.path().join("b");
    let c = ws.path().join("c");
    fs::create_dir_all(&root).unwrap();

    let root_yaml = pack_yaml_with_children("root", &[("git://x/a.git", "a", None)]);
    let a_yaml =
        pack_yaml_with_children("a", &[("git://x/b.git", "b", None), ("git://x/c.git", "c", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&a_yaml))
        .with(b.clone(), parse_pack(&pack_yaml("b")))
        .with(c.clone(), parse_pack(&pack_yaml("c")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root).expect("walk");

    assert_eq!(graph.nodes().len(), 4);
    // A has parent = root (0)
    let a_node = graph.find_by_name("a").unwrap();
    assert_eq!(a_node.parent, Some(0));
    // B and C both parent to A
    let b_node = graph.find_by_name("b").unwrap();
    let c_node = graph.find_by_name("c").unwrap();
    assert_eq!(b_node.parent, Some(a_node.id));
    assert_eq!(c_node.parent, Some(a_node.id));
}

// ---------------------------------------------------------------------------
// B1 — pre-walk path-traversal gate (v1.1.0 post-review)
//
// A malicious parent pack with `children[].path: "../escape"` must be
// rejected BEFORE any clone fires. Plan-phase validation runs after the
// walker historically, so the walker now also runs `ChildPathValidator`
// on each loaded manifest's children list before resolving them on
// disk. Backend `Clone` must not be observed for the offending child.
// ---------------------------------------------------------------------------

#[test]
fn walker_rejects_parent_traversal_in_child_path_pre_clone() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    fs::create_dir_all(&root).unwrap();

    // pack.yaml with a hostile child path. Parser accepts (no validator
    // runs at parse time); the walker must catch it before issuing a
    // clone that would create `<ws>/../escape/`.
    let root_yaml =
        pack_yaml_with_children("root", &[("git://host/escape.git", "../escape", None)]);
    let loader = MockLoader::new().with(root.clone(), parse_pack(&root_yaml));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());

    let err = walker.walk(&root).expect_err("walker must reject path traversal");
    match err {
        TreeError::ChildPathInvalid { path, reason, .. } => {
            assert_eq!(path, "../escape");
            assert!(reason.contains("separator"), "reason: {reason}");
        }
        other => panic!("wrong variant: {other:?}"),
    }

    // Critical: backend never saw a Clone for the offending child.
    let calls = backend.calls();
    assert!(
        !calls.iter().any(|c| matches!(c, BackendCall::Clone { .. })),
        "no clone may fire for a traversal-bearing child path; got: {calls:?}",
    );

    // Sibling check: the resolved on-disk path must not exist either.
    // We deliberately walk to `<ws>/../escape` which would land outside
    // `ws.path()`. Compose it manually so the assertion does not depend
    // on backend behaviour.
    let escaped = ws.path().join("../escape");
    assert!(
        !escaped.exists(),
        "no `../escape` directory must be created; found: {}",
        escaped.display(),
    );
}

#[test]
fn walker_rejects_traversal_in_grandchild_pack_pre_clone() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let mid = ws.path().join("mid");
    fs::create_dir_all(&root).unwrap();

    // Root → mid (clean). Mid's pack.yaml lists a hostile grandchild.
    // Walker must clone mid (legitimate) but reject before cloning
    // `<ws>/../grand`.
    let root_yaml = pack_yaml_with_children("root", &[("git://host/mid.git", "mid", None)]);
    let mid_yaml =
        pack_yaml_with_children("mid", &[("git://host/grand.git", "../grand", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(mid.clone(), parse_pack(&mid_yaml));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());

    let err = walker.walk(&root).expect_err("grandchild traversal must be rejected");
    assert!(matches!(err, TreeError::ChildPathInvalid { .. }), "got: {err:?}");

    // Mid clones (legitimate); grand never does.
    let calls = backend.calls();
    let grand_clones = calls
        .iter()
        .filter(|c| matches!(c, BackendCall::Clone { url, .. } if url.contains("grand")))
        .count();
    assert_eq!(grand_clones, 0, "no clone may fire for grandchild traversal; got: {calls:?}");
}

#[test]
fn walker_uses_git_backend_for_children() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");

    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", Some("v1"))]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    walker.walk(&root).expect("walk");

    let calls = backend.calls();
    assert_eq!(calls.len(), 1);
    match &calls[0] {
        BackendCall::Clone { url, dest, r#ref } => {
            assert_eq!(url, "git://host/a.git");
            assert_eq!(dest, &a);
            assert_eq!(r#ref.as_deref(), Some("v1"));
        }
        other => panic!("expected Clone, got {other:?}"),
    }
}

#[test]
fn walker_skips_existing_child_destinations() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");
    // Pre-populate destination with a .git marker so the walker's existence
    // probe treats it as already-hydrated.
    fs::create_dir_all(a.join(".git")).unwrap();

    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    walker.walk(&root).expect("walk");

    let calls = backend.calls();
    assert!(calls.iter().any(|c| matches!(c, BackendCall::Fetch { .. })));
    assert!(
        !calls.iter().any(|c| matches!(c, BackendCall::Clone { .. })),
        "clone must NOT be called when dest exists"
    );
}

// ---------------------------------------------------------------------------
// M4-D D1 — `--ref` override surface
//
// `Walker::with_ref_override` sets a global ref that wins over each child's
// declared `ref` in the parent manifest. Cover both hydration paths:
//
// 1. Dest missing → backend `clone(…, Some(override))`.
// 2. Dest present → backend `checkout(…, override)`.
//
// `ref_override = None` and `ref_override = Some("")` must both no-op (the
// builder filters empty strings explicitly).
// ---------------------------------------------------------------------------

#[test]
fn walker_ref_override_wins_over_declared_on_clone() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");

    // Child declares ref=v1; override asks for v2 → clone must receive v2.
    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", Some("v1"))]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf())
        .with_ref_override(Some("v2-override".to_string()));
    walker.walk(&root).expect("walk");

    let calls = backend.calls();
    match calls.iter().find(|c| matches!(c, BackendCall::Clone { .. })) {
        Some(BackendCall::Clone { r#ref, .. }) => {
            assert_eq!(
                r#ref.as_deref(),
                Some("v2-override"),
                "override must win over declared `v1`"
            );
        }
        other => panic!("expected Clone with override ref, got {other:?}"),
    }
}

#[test]
fn walker_ref_override_wins_over_declared_on_checkout() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");
    // Pre-hydrate the destination so the walker takes the fetch+checkout path.
    fs::create_dir_all(a.join(".git")).unwrap();

    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", Some("main"))]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf())
        .with_ref_override(Some("release/1.0".to_string()));
    walker.walk(&root).expect("walk");

    let calls = backend.calls();
    let checkout_ref = calls.iter().find_map(|c| match c {
        BackendCall::Checkout { r#ref, .. } => Some(r#ref.clone()),
        _ => None,
    });
    assert_eq!(
        checkout_ref.as_deref(),
        Some("release/1.0"),
        "override must win over declared `main`"
    );
}

#[test]
fn walker_empty_ref_override_is_equivalent_to_none() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");

    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", Some("v1"))]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    // Empty string override must be filtered by `with_ref_override` so the
    // declared `v1` survives.
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf())
        .with_ref_override(Some(String::new()));
    walker.walk(&root).expect("walk");

    let calls = backend.calls();
    match calls.iter().find(|c| matches!(c, BackendCall::Clone { .. })) {
        Some(BackendCall::Clone { r#ref, .. }) => {
            assert_eq!(r#ref.as_deref(), Some("v1"), "empty override must be inert");
        }
        other => panic!("expected Clone, got {other:?}"),
    }
}

#[test]
fn walker_applies_ref_when_specified_on_existing_dest() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");
    fs::create_dir_all(a.join(".git")).unwrap();

    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", Some("main"))]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    walker.walk(&root).expect("walk");

    let calls = backend.calls();
    assert!(calls
        .iter()
        .any(|c| matches!(c, BackendCall::Checkout { r#ref, .. } if r#ref == "main")));
}

#[test]
fn walker_error_on_manifest_parse_fail() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");

    let root_yaml = pack_yaml_with_children("root", &[("git://host/a.git", "a", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with_parse_error(a.clone(), "bogus YAML");
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let err = walker.walk(&root).unwrap_err();
    match err {
        TreeError::ManifestParse { detail, .. } => assert!(detail.contains("bogus")),
        other => panic!("expected ManifestParse, got {other:?}"),
    }
}

#[test]
fn walker_cycle_self_reference() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");

    // Self-reference: root's child has the same url+ref pair that would
    // be pushed again on recursion. The walker tracks url+ref pairs,
    // and its root identity is a path, so we emulate a cycle by having
    // the child point back via a url that matches itself.
    let yaml = pack_yaml_with_children("root", &[("git://self/self.git", "self", None)]);
    // The child manifest again lists itself as a child.
    let self_path = ws.path().join("self");
    let self_yaml = pack_yaml_with_children("self", &[("git://self/self.git", "self", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&yaml))
        .with(self_path, parse_pack(&self_yaml));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let err = walker.walk(&root).unwrap_err();
    match err {
        TreeError::CycleDetected { chain } => {
            assert!(chain.len() >= 2, "chain must include at least 2 entries: {chain:?}");
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
}

#[test]
fn walker_cycle_indirect_a_b_a() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");
    let b = ws.path().join("b");

    let root_yaml = pack_yaml_with_children("root", &[("git://x/a.git", "a", None)]);
    let a_yaml = pack_yaml_with_children("a", &[("git://x/b.git", "b", None)]);
    // b points back to a via the same url+ref: cycle.
    let b_yaml = pack_yaml_with_children("b", &[("git://x/a.git", "a", None)]);

    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&a_yaml))
        .with(b.clone(), parse_pack(&b_yaml));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let err = walker.walk(&root).unwrap_err();
    match err {
        TreeError::CycleDetected { chain } => {
            assert!(
                chain.len() >= 3,
                "A→B→A cycle should produce chain length >= 3, got {chain:?}"
            );
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }
}

#[test]
fn walker_pack_name_mismatch_errors() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");

    // Parent expects child at path "a" but the child manifest declares
    // name "not-a" — walker must reject.
    let root_yaml = pack_yaml_with_children("root", &[("git://x/a.git", "a", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("not-a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let err = walker.walk(&root).unwrap_err();
    match err {
        TreeError::PackNameMismatch { got, expected, .. } => {
            assert_eq!(got, "not-a");
            assert_eq!(expected, "a");
        }
        other => panic!("expected PackNameMismatch, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Graph validator tests
// ---------------------------------------------------------------------------

#[test]
fn graph_validate_passes_for_clean_tree() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let a = ws.path().join("a");

    let root_yaml = pack_yaml_with_children("root", &[("git://x/a.git", "a", None)]);
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(a.clone(), parse_pack(&pack_yaml("a")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root).unwrap();
    graph.validate().expect("clean graph validates");
}

#[test]
fn graph_validate_depends_on_satisfied_by_name() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let b = ws.path().join("b");

    // Root's depends_on names "b"; "b" exists as a child.
    let root_yaml = "schema_version: \"1\"\nname: root\ntype: meta\ndepends_on:\n  - b\nchildren:\n  - url: git://x/b.git\n    path: b\n".to_string();
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(b.clone(), parse_pack(&pack_yaml("b")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root).unwrap();
    graph.validate().expect("depends_on satisfied");
}

#[test]
fn graph_validate_depends_on_unsatisfied() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");

    let yaml = pack_yaml_with_deps("root", &["z"]);
    let loader = MockLoader::new().with(root.clone(), parse_pack(&yaml));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root).unwrap();
    let errs = graph.validate().unwrap_err();
    assert!(
        errs.iter().any(|e| matches!(
            e,
            PackValidationError::DependsOnUnsatisfied { required, .. } if required == "z"
        )),
        "must flag missing z, got {errs:?}"
    );
}

#[test]
fn graph_validate_depends_on_satisfied_by_url() {
    let ws = TempDir::new().unwrap();
    let root = ws.path().join("root");
    let b = ws.path().join("b");

    let root_yaml = "schema_version: \"1\"\nname: root\ntype: meta\ndepends_on:\n  - git://x/b.git\nchildren:\n  - url: git://x/b.git\n    path: b\n".to_string();
    let loader = MockLoader::new()
        .with(root.clone(), parse_pack(&root_yaml))
        .with(b.clone(), parse_pack(&pack_yaml("b")));
    let backend = mock_git();
    let walker = Walker::new(&loader, &backend, ws.path().to_path_buf());
    let graph = walker.walk(&root).unwrap();
    graph.validate().expect("depends_on by url satisfied");
}

// ---------------------------------------------------------------------------
// Real backend integration
// ---------------------------------------------------------------------------

fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
    });
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Build a bare repo whose working copy seeds `.grex/pack.yaml` with `yaml`.
fn bare_with_manifest(tmp: &Path, name: &str, yaml: &str) -> PathBuf {
    init_git_identity();
    let work = tmp.join(format!("seed-{name}-work"));
    fs::create_dir_all(work.join(".grex")).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    fs::write(work.join(".grex/pack.yaml"), yaml).unwrap();
    run_git(&work, &["add", ".grex/pack.yaml"]);
    run_git(&work, &["commit", "-q", "-m", "seed"]);

    let bare = tmp.join(format!("{name}.git"));
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

#[test]
fn walker_integrates_with_real_git_backend() {
    let tmp = TempDir::new().unwrap();
    // Build a child bare repo first so we can embed its url in the root.
    let child_bare = bare_with_manifest(tmp.path(), "child", &pack_yaml("child"));
    let child_url = file_url_from_path(&child_bare);

    // Build the root pack *without* git — we'll load it directly from a
    // directory on disk via FsPackLoader.
    let root_dir = tmp.path().join("root");
    fs::create_dir_all(root_dir.join(".grex")).unwrap();
    let root_yaml = format!(
        "schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n  - url: {child_url}\n    path: child\n"
    );
    fs::write(root_dir.join(".grex/pack.yaml"), &root_yaml).unwrap();

    let ws = tmp.path().join("ws");
    fs::create_dir_all(&ws).unwrap();
    let loader = FsPackLoader::new();
    let backend = GixBackend::new();
    let walker = Walker::new(&loader, &backend, ws.clone());

    let graph = walker.walk(&root_dir).expect("walk");
    assert_eq!(graph.nodes().len(), 2);
    assert_eq!(graph.root().name, "root");
    assert!(ws.join("child").join(".grex").join("pack.yaml").is_file());
    assert!(ws.join("child").join(".git").exists());
    graph.validate().expect("clean real-backend graph validates");
}
