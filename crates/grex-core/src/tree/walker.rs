//! Recursive pack-tree walker.
//!
//! The walker hydrates a `pack.yaml` tree: it loads the root manifest, clones
//! (or fetches + checks out) every `children:` entry via the injected
//! [`GitBackend`], and recurses. `depends_on` entries are recorded as edges
//! but never walked — they are *external prereqs* verified by
//! [`crate::pack::validate::DependsOnValidator`] after the graph is built.
//!
//! # Cycle detection
//!
//! Cycles are detected **during** the walk, not post-hoc. Each recursion
//! maintains a walk stack of pack identifiers (source-url when present,
//! otherwise the canonical on-disk path). If a child is about to be entered
//! whose identifier is already on the stack, the walker short-circuits with
//! [`TreeError::CycleDetected`]. A separate `CycleValidator` runs
//! post-hoc as a belt-and-suspenders check so manually-constructed graphs
//! cannot sneak through.
//!
//! # Cyclomatic discipline
//!
//! The walk is decomposed so each helper stays well under CC 15:
//! `walk` → `walk_recursive` → `process_children` → `handle_child` →
//! `resolve_destination` | `record_depends_on`.

use std::path::{Path, PathBuf};

use crate::git::GitBackend;
use crate::pack::validate::child_path::check_one as check_child_path;
use crate::pack::{ChildRef, PackManifest, PackValidationError};

use super::error::TreeError;
use super::graph::{EdgeKind, PackEdge, PackGraph, PackNode};
use super::loader::PackLoader;

/// Recursive walker. Composes a [`PackLoader`] (for manifests) with a
/// [`GitBackend`] (for child hydration).
///
/// The walker owns no state across calls: each invocation of [`Walker::walk`]
/// produces a fresh [`PackGraph`] and leaves no footprint.
pub struct Walker<'a> {
    loader: &'a dyn PackLoader,
    backend: &'a dyn GitBackend,
    workspace: PathBuf,
    /// Optional global ref override (M4-D `grex sync --ref <sha|branch|tag>`).
    /// When `Some`, every child clone/checkout uses this ref instead of the
    /// declared `child.ref` from the parent manifest. `None` preserves M3
    /// semantics.
    ref_override: Option<String>,
}

impl<'a> Walker<'a> {
    /// Construct a new walker.
    ///
    /// `workspace` is the directory under which child packs will be cloned,
    /// using each [`ChildRef::effective_path`] as the sub-directory name.
    #[must_use]
    pub fn new(
        loader: &'a dyn PackLoader,
        backend: &'a dyn GitBackend,
        workspace: PathBuf,
    ) -> Self {
        Self { loader, backend, workspace, ref_override: None }
    }

    /// Set a global ref override applied to every child pack.
    ///
    /// Surfaced as `grex sync --ref <sha|branch|tag>` (M4-D). The override
    /// replaces each child's declared `ref` in its parent manifest. An
    /// empty string is treated as "no override" — callers should reject
    /// empty values at the CLI layer before reaching this point.
    #[must_use]
    pub fn with_ref_override(mut self, r#ref: Option<String>) -> Self {
        self.ref_override = r#ref.filter(|s| !s.is_empty());
        self
    }

    /// Walk the tree rooted at `root_pack_path`, returning the fully
    /// hydrated graph.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError`] on any loader, git, cycle, or name-mismatch
    /// failure. The walk aborts on the first failure — the spec-level
    /// "fail loud, fail fast" default.
    pub fn walk(&self, root_pack_path: &Path) -> Result<PackGraph, TreeError> {
        let mut state = BuildState::default();
        let root_manifest = self.loader.load(root_pack_path)?;
        // Pre-walk path-traversal gate: reject any malicious
        // `children[].path` (or URL-derived tail) BEFORE any clone fires.
        // Closes the v1.1.0 flat-sibling exploit window where a `path:
        // ../escape` would materialise a child outside the pack root
        // before plan-phase validation could see it.
        validate_children_paths(&root_manifest)?;
        let root_commit_sha = probe_head_sha(self.backend, root_pack_path);
        let root_id = state.push_node(PackNode {
            id: 0,
            name: root_manifest.name.clone(),
            path: root_pack_path.to_path_buf(),
            source_url: None,
            manifest: root_manifest.clone(),
            parent: None,
            commit_sha: root_commit_sha,
        });
        let root_identity = pack_identity_for_root(root_pack_path);
        self.walk_recursive(root_id, &root_manifest, &mut state, &mut vec![root_identity])?;
        Ok(PackGraph::new(state.nodes, state.edges))
    }

    /// Recursive step. `stack` carries the pack identifiers currently on
    /// the walk path — pushed on entry, popped on return.
    ///
    /// Each loaded manifest's `children[]` is path-traversal-validated
    /// before any of those children are resolved on disk; the entry
    /// point pre-validates the root manifest, so by the time
    /// `walk_recursive` runs for a child, that child's own `children[]`
    /// is what needs gating before the next descent.
    fn walk_recursive(
        &self,
        parent_id: usize,
        manifest: &PackManifest,
        state: &mut BuildState,
        stack: &mut Vec<String>,
    ) -> Result<(), TreeError> {
        self.record_depends_on(parent_id, manifest, state);
        self.process_children(parent_id, manifest, state, stack)
    }

    /// Record one `DependsOn` edge per `depends_on` entry. Resolution
    /// against actual graph nodes happens later in `DependsOnValidator`.
    /// We emit edges only where the target already exists in the graph so
    /// the edge list stays in-bounds; unresolved deps are surfaced by the
    /// validator, not carried as dangling edges.
    fn record_depends_on(&self, parent_id: usize, manifest: &PackManifest, state: &mut BuildState) {
        for dep in &manifest.depends_on {
            if let Some(to) = find_node_id_by_name_or_url(&state.nodes, dep) {
                state.edges.push(PackEdge { from: parent_id, to, kind: EdgeKind::DependsOn });
            }
        }
    }

    fn process_children(
        &self,
        parent_id: usize,
        manifest: &PackManifest,
        state: &mut BuildState,
        stack: &mut Vec<String>,
    ) -> Result<(), TreeError> {
        for child in &manifest.children {
            self.handle_child(parent_id, child, state, stack)?;
        }
        Ok(())
    }

    fn handle_child(
        &self,
        parent_id: usize,
        child: &ChildRef,
        state: &mut BuildState,
        stack: &mut Vec<String>,
    ) -> Result<(), TreeError> {
        let identity = pack_identity_for_child(child);
        if stack.iter().any(|s| s == &identity) {
            let mut chain = stack.clone();
            chain.push(identity);
            return Err(TreeError::CycleDetected { chain });
        }
        let dest = self.resolve_destination(child, state)?;
        let child_manifest = self.loader.load(&dest)?;
        verify_child_name(&child_manifest.name, child, &dest)?;
        // Validate this child's own `children[]` before its descent
        // resolves any of them on disk. Mirrors the root-manifest gate
        // in `walk`; together they ensure no clone can fire for a
        // grandchild whose parent declared a traversal-bearing path.
        validate_children_paths(&child_manifest)?;

        let commit_sha = probe_head_sha(self.backend, &dest);
        let child_id = state.push_node(PackNode {
            id: state.nodes.len(),
            name: child_manifest.name.clone(),
            path: dest.clone(),
            source_url: Some(child.url.clone()),
            manifest: child_manifest.clone(),
            parent: Some(parent_id),
            commit_sha,
        });
        state.edges.push(PackEdge { from: parent_id, to: child_id, kind: EdgeKind::Child });

        stack.push(identity);
        let result = self.walk_recursive(child_id, &child_manifest, state, stack);
        stack.pop();
        result
    }

    /// Decide where `child` lives on disk and ensure the working tree is
    /// in the expected state: clone if absent, fetch + optional checkout
    /// if present.
    fn resolve_destination(
        &self,
        child: &ChildRef,
        _state: &mut BuildState,
    ) -> Result<PathBuf, TreeError> {
        let dest = self.workspace.join(child.effective_path());
        // M4-D: `ref_override` wins over the parent-declared `child.ref`.
        // Falls back to the declared ref when no override is active.
        let effective_ref = self.ref_override.as_deref().or(child.r#ref.as_deref());
        if dest_has_git_repo(&dest) {
            self.backend.fetch(&dest)?;
            if let Some(r) = effective_ref {
                self.backend.checkout(&dest, r)?;
            }
        } else {
            self.backend.clone(&child.url, &dest, effective_ref)?;
        }
        Ok(dest)
    }
}

/// Best-effort HEAD probe. Returns `None` when the target is not a git
/// repository or the backend refuses — the root of a declarative pack is
/// often a plain directory, so this must not fail the walk.
///
/// Non-`.git` directories short-circuit silently (truly not a git
/// repo). Backend errors on an actual `.git` directory are surfaced as
/// a `tracing::warn!` log line so transient gix failures / ACL-denied
/// `.git` reads do not silently degrade into an empty `commit_sha`
/// without any operator signal. The walker continues with `None` — a
/// best-effort probe is, by construction, allowed to fail.
fn probe_head_sha(backend: &dyn GitBackend, path: &Path) -> Option<String> {
    let dir =
        if path.extension().and_then(|e| e.to_str()).is_some_and(|e| matches!(e, "yaml" | "yml")) {
            path.parent()
                .and_then(Path::parent)
                .map_or_else(|| path.to_path_buf(), Path::to_path_buf)
        } else {
            path.to_path_buf()
        };
    if !dir.join(".git").exists() {
        return None;
    }
    match backend.head_sha(&dir) {
        Ok(s) => Some(s),
        Err(e) => {
            tracing::warn!(
                target: "grex::walker",
                "HEAD probe failed for {}: {e}",
                dir.display()
            );
            None
        }
    }
}

/// Mutable state threaded through the walk. Private to this module so only
/// the walker can grow the graph.
#[derive(Default)]
struct BuildState {
    nodes: Vec<PackNode>,
    edges: Vec<PackEdge>,
}

impl BuildState {
    fn push_node(&mut self, node: PackNode) -> usize {
        let id = node.id;
        self.nodes.push(node);
        id
    }
}

/// Identity string used by the cycle detector for the root pack.
fn pack_identity_for_root(path: &Path) -> String {
    format!("path:{}", path.display())
}

/// Identity string for a child — url+ref so the same repo at two different
/// refs is considered distinct. This matches git semantics and avoids
/// false-positive cycle detections for diamond dependencies on different
/// tags.
fn pack_identity_for_child(child: &ChildRef) -> String {
    let rref = child.r#ref.as_deref().unwrap_or("");
    format!("url:{}@{}", child.url, rref)
}

/// Shallow on-disk check: a `.git` entry (file or dir) signals an existing
/// working tree. We deliberately do not open the repo here — that's the
/// backend's job via `fetch`/`checkout`.
fn dest_has_git_repo(dest: &Path) -> bool {
    dest.join(".git").exists()
}

/// Enforce that the cloned child's pack.yaml name matches what the parent
/// declared. The parent-side expectation is the child entry's
/// [`ChildRef::effective_path`] — the directory name in the workspace.
fn verify_child_name(got: &str, child: &ChildRef, dest: &Path) -> Result<(), TreeError> {
    let expected = child.effective_path();
    if got == expected {
        return Ok(());
    }
    Err(TreeError::PackNameMismatch { got: got.to_string(), expected, path: dest.to_path_buf() })
}

/// Resolve a `depends_on` entry (URL or bare name) against nodes already
/// recorded. Returns the node id on a hit, `None` otherwise.
fn find_node_id_by_name_or_url(nodes: &[PackNode], dep: &str) -> Option<usize> {
    if looks_like_url(dep) {
        nodes.iter().find(|n| n.source_url.as_deref() == Some(dep)).map(|n| n.id)
    } else {
        nodes.iter().find(|n| n.name == dep).map(|n| n.id)
    }
}

/// Run the path-traversal gate on `manifest.children`. Returns the
/// first offending child as a [`TreeError::ChildPathInvalid`] so the
/// walker aborts before any clone of the offending sibling fires.
///
/// Surfacing only the first offender (rather than aggregating) matches
/// the walker's fail-fast posture — the plan-phase
/// [`crate::pack::validate::ChildPathValidator`] still runs against the
/// whole graph post-walk via `validate_graph`, so authors who clear
/// the traversal exploit see the full diagnostic batch on the next
/// invocation.
fn validate_children_paths(manifest: &PackManifest) -> Result<(), TreeError> {
    for child in &manifest.children {
        if let Some(PackValidationError::ChildPathInvalid { child_name, path, reason }) =
            check_child_path(child)
        {
            return Err(TreeError::ChildPathInvalid { child_name, path, reason });
        }
    }
    Ok(())
}

/// Decide whether a `depends_on` entry is a URL rather than a bare name.
/// The rule is intentionally literal — matching the spec's enumeration of
/// accepted forms.
pub(super) fn looks_like_url(s: &str) -> bool {
    s.starts_with("http://")
        || s.starts_with("https://")
        || s.starts_with("ssh://")
        || s.starts_with("git@")
        || s.ends_with(".git")
}
