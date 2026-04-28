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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::git::GitBackend;
use crate::pack::validate::child_path::check_one as check_child_path;
use crate::pack::{ChildRef, PackManifest, PackType, PackValidationError, SchemaVersion};

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
            synthetic: false,
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
        // v1.1.1 plain-git children: when the destination has no
        // `.grex/pack.yaml` but does carry a `.git/`, synthesize a
        // leaf scripted-no-hooks manifest in-memory rather than
        // aborting. See
        // `openspec/changes/feat-v1.1.1-plain-git-children/design.md`
        // §"Synthesis algorithm".
        let (child_manifest, is_synthetic) = match self.loader.load(&dest) {
            Ok(m) => (m, false),
            Err(TreeError::ManifestNotFound(_)) if dest_has_git_repo(&dest) => {
                (synthesize_plain_git_manifest(child), true)
            }
            Err(e) => return Err(e),
        };
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
            synthetic: is_synthetic,
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
///
/// # Symlink safety
///
/// `dest` itself MUST NOT be a symlink. If it is, this function returns
/// `false` regardless of whether the symlink target carries a `.git`
/// entry. This refusal closes a synthesis-redirection attack: a parent
/// pack declaring `path: code` against a workspace where the user
/// happens to have `<workspace>/code -> $HOME` would otherwise let the
/// walker treat `$HOME/.git` as a "plain-git child" and operate on an
/// unrelated tree. The check uses [`std::fs::symlink_metadata`] so the
/// link itself — not its target — is interrogated.
pub fn dest_has_git_repo(dest: &Path) -> bool {
    // Reject symlinked destinations outright. `symlink_metadata` does
    // NOT follow the link, so a broken or path-traversing symlink is
    // treated as untrusted regardless of its target.
    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if meta.file_type().is_symlink() {
            return false;
        }
    }
    dest.join(".git").exists()
}

/// Build the in-memory manifest used for v1.1.1 plain-git children — a
/// leaf scripted pack with no hooks, no children, no actions. Activated
/// at the walker's load-fallback boundary when a child has a `.git/`
/// but no `.grex/pack.yaml`. See
/// `openspec/changes/feat-v1.1.1-plain-git-children/design.md`.
pub fn synthesize_plain_git_manifest(child: &ChildRef) -> PackManifest {
    PackManifest {
        schema_version: SchemaVersion::current(),
        name: child.effective_path(),
        r#type: PackType::Scripted,
        version: None,
        depends_on: Vec::new(),
        children: Vec::new(),
        actions: Vec::new(),
        teardown: None,
        extensions: BTreeMap::new(),
    }
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
///
/// `check_child_path` is documented to return only the
/// `ChildPathInvalid` variant, but we `match` exhaustively so any
/// future variant the helper grows surfaces as a compile-time
/// failure here rather than as a silently swallowed `Some(other)`.
fn validate_children_paths(manifest: &PackManifest) -> Result<(), TreeError> {
    for child in &manifest.children {
        let Some(err) = check_child_path(child) else { continue };
        match err {
            PackValidationError::ChildPathInvalid { child_name, path, reason } => {
                return Err(TreeError::ChildPathInvalid { child_name, path, reason });
            }
            other @ (PackValidationError::DuplicateSymlinkDst { .. }
            | PackValidationError::GraphCycle { .. }
            | PackValidationError::DependsOnUnsatisfied { .. }
            | PackValidationError::ChildPathDuplicate { .. }) => {
                // `check_child_path` is contracted to only emit
                // `ChildPathInvalid`. Any other variant indicates the
                // helper has drifted out of sync with this caller —
                // surface loudly rather than silently swallowing it.
                tracing::error!(
                    target: "grex::walker",
                    "check_child_path returned unexpected variant: {other:?}",
                );
                debug_assert!(false, "check_child_path returned unexpected variant: {other:?}");
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Direct unit test of the synthesis helper — name must equal the
    /// child's `effective_path()`, type must be `Scripted`, and every
    /// list field must be empty.
    #[test]
    fn synthesize_plain_git_manifest_yields_leaf_scripted_pack() {
        let child = ChildRef {
            url: "https://example.com/algo-leet.git".to_string(),
            path: None,
            r#ref: None,
        };
        let manifest = synthesize_plain_git_manifest(&child);
        assert_eq!(manifest.name, child.effective_path());
        assert_eq!(manifest.name, "algo-leet");
        assert_eq!(manifest.r#type, PackType::Scripted);
        assert_eq!(manifest.schema_version.as_str(), "1");
        assert!(manifest.depends_on.is_empty());
        assert!(manifest.children.is_empty());
        assert!(manifest.actions.is_empty());
        assert!(manifest.teardown.is_none());
        assert!(manifest.extensions.is_empty());
        assert!(manifest.version.is_none());
    }

    /// Explicit `path:` override wins over the URL-derived bare name —
    /// confirms the synthesised manifest's `name` mirrors what the
    /// parent declared, so `verify_child_name` passes by construction.
    #[test]
    fn synthesize_plain_git_manifest_honours_explicit_path() {
        let child = ChildRef {
            url: "https://example.com/some-repo.git".to_string(),
            path: Some("custom-name".to_string()),
            r#ref: None,
        };
        let manifest = synthesize_plain_git_manifest(&child);
        assert_eq!(manifest.name, "custom-name");
    }

    /// `dest_has_git_repo` MUST refuse a symlinked destination — even
    /// when the symlink target carries a real `.git/` directory.
    /// Otherwise a malicious parent pack could redirect synthesis to
    /// fetch into `$HOME` (or any sibling repo) by relying on a
    /// pre-existing symlink in the workspace.
    #[test]
    fn dest_has_git_repo_rejects_symlinked_dest() {
        // Skip on platforms where unprivileged symlink creation fails
        // (notably Windows without Developer Mode). Failing the symlink
        // call is itself proof the attack vector is closed for that
        // host, so the rest of the test is moot.
        let outer = tempfile::tempdir().unwrap();
        let real = outer.path().join("real-repo");
        std::fs::create_dir_all(real.join(".git")).unwrap();
        let link = outer.path().join("via-link");

        #[cfg(unix)]
        let symlink_result = std::os::unix::fs::symlink(&real, &link);
        #[cfg(windows)]
        let symlink_result = std::os::windows::fs::symlink_dir(&real, &link);

        if symlink_result.is_err() {
            // Host won't let us create a symlink — nothing to test.
            return;
        }

        // Sanity: following the symlink would reveal `.git`.
        assert!(link.join(".git").exists(), "symlink target should expose .git through traversal");
        // But `dest_has_git_repo` must refuse it.
        assert!(
            !dest_has_git_repo(&link),
            "dest_has_git_repo must refuse a symlinked destination even when target has .git"
        );
        // Real (non-symlinked) sibling still passes — we haven't
        // accidentally broken the happy path.
        assert!(dest_has_git_repo(&real));
    }
}
