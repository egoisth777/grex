//! Read-only [`PackGraph`] construction (v1.2.1 path iii).
//!
//! Extracted from [`super::walker::Walker::walk`] for the v1.2.1 sync
//! orchestrator refactor. Where the legacy [`super::walker::Walker`]
//! interleaved disk MUTATION (clone / fetch) with graph BUILD,
//! [`build_graph`] is strictly READ-ONLY: it walks the manifest tree, loads
//! each child's `pack.yaml` from disk (or synthesises a plain-git leaf when
//! the dest carries `.git/` but no `.grex/pack.yaml`), records cycles, and
//! produces the [`PackGraph`] consumed downstream by `sync::run_actions`.
//!
//! The mutation half (clone / fetch / lockfile / TOCTOU prune) is owned by
//! [`super::walker::sync_meta`], which the orchestrator calls BEFORE
//! `build_graph` so that every dest the build pass needs to read is already
//! materialised on disk.
//!
//! # Resolution model
//!
//! * **Parent-relative.** `dest = parent_meta.join(child.effective_path())`.
//!   Each recursion frame anchors against ITS parent meta's directory —
//!   matching the v1.2.0 walker semantics that `sync_meta` already
//!   implements. There is no global workspace anchor below the root.
//! * **Workspace anchor (root only).** The supplied `workspace` argument is
//!   the canonical path under which the root meta lives. Whether the caller
//!   passed a directory or a `.yaml` file, the canonical workspace is used
//!   verbatim as the root meta's on-disk location.
//!
//! # What this fn does NOT do
//!
//! * No `git fetch` / `git clone` — `sync_meta` already ran.
//! * No prune dispatch — `sync_meta` already swept Phase 2 candidates.
//! * No `--force-prune` engagement — read-only by construction.
//! * No `BoundedDir` open — TOCTOU dirfd binding belongs to the mutating
//!   half (`sync_meta`'s prune path), not the read-build pass.
//!
//! # Non-goals (v1.2.1)
//!
//! * Sibling-parallel build. The walk is sequential — graph build is cheap
//!   relative to git ops, and the Lean `sync_disjoint_commutes` axiom is
//!   trivially preserved by a sequential reader.

use std::collections::BTreeMap;
use std::path::Path;

use crate::git::GitBackend;
use crate::pack::{ChildRef, PackManifest, PackType, PackValidationError, SchemaVersion};

use super::error::TreeError;
use super::graph::{EdgeKind, PackEdge, PackGraph, PackNode};
use super::loader::PackLoader;
use super::walker::{check_dest_boundary, dest_has_git_repo, looks_like_url};

/// Read-only pack-tree walker that produces a [`PackGraph`].
///
/// Inputs are intentionally narrow: a workspace anchor (canonical path of
/// the root meta on disk), a [`PackLoader`] for manifests, and a
/// [`GitBackend`] used solely for the best-effort `head_sha` probe per
/// node. The backend is NEVER asked to clone or fetch — this fn is
/// strictly READ-ONLY.
///
/// `ref_override` is threaded through for parity with the legacy walker's
/// `with_ref_override` knob: it does not affect graph shape (we don't
/// mutate disk here), but it IS recorded against children whose
/// downstream lockfile/probe needs it. Currently unused inside
/// `build_graph` — kept in the signature so the orchestrator's wiring
/// matches the legacy `Walker::with_ref_override` surface 1:1, easing
/// future extensions without a signature break.
///
/// # Errors
///
/// Returns [`TreeError`] on any loader, child-name-mismatch, cycle, or
/// path-traversal failure encountered while reading the on-disk tree.
/// Fails-fast on the first error — matches [`super::walker::Walker::walk`]'s
/// behaviour so callers see the same diagnostic surface.
pub fn build_graph(
    workspace: &Path,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    ref_override: Option<&str>,
) -> Result<PackGraph, TreeError> {
    let _ = ref_override; // currently unused by the reader; see fn doc
    let root_manifest = loader.load(workspace)?;
    validate_children_paths(&root_manifest)?;
    let root_commit_sha = probe_head_sha(backend, workspace);
    let mut state = BuildState::default();
    let root_id = state.push_node(PackNode {
        id: 0,
        name: root_manifest.name.clone(),
        path: workspace.to_path_buf(),
        source_url: None,
        manifest: root_manifest.clone(),
        parent: None,
        commit_sha: root_commit_sha,
        synthetic: false,
    });
    let root_identity = pack_identity_for_root(workspace);
    walk_recursive(
        backend,
        loader,
        root_id,
        workspace,
        &root_manifest,
        &mut state,
        &mut vec![root_identity],
    )?;
    Ok(PackGraph::new(state.nodes, state.edges))
}

// ---------------------------------------------------------------------------
// Internal helpers — narrow analogues of the private bits inside
// [`super::walker`]. Kept separate from the legacy `Walker` impl so the
// read-only contract is enforced by construction (no `&mut backend` reach
// past `head_sha`, no `loader.load` outside the build pass).
// ---------------------------------------------------------------------------

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

fn walk_recursive(
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    parent_id: usize,
    parent_meta: &Path,
    manifest: &PackManifest,
    state: &mut BuildState,
    ancestors: &mut Vec<String>,
) -> Result<(), TreeError> {
    record_depends_on(parent_id, manifest, state);
    process_children(backend, loader, parent_id, parent_meta, manifest, state, ancestors)
}

fn record_depends_on(parent_id: usize, manifest: &PackManifest, state: &mut BuildState) {
    for dep in &manifest.depends_on {
        if let Some(to) = find_node_id_by_name_or_url(&state.nodes, dep) {
            state.edges.push(PackEdge { from: parent_id, to, kind: EdgeKind::DependsOn });
        }
    }
}

fn process_children(
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    parent_id: usize,
    parent_meta: &Path,
    manifest: &PackManifest,
    state: &mut BuildState,
    ancestors: &mut Vec<String>,
) -> Result<(), TreeError> {
    for child in &manifest.children {
        handle_child(backend, loader, parent_id, parent_meta, child, state, ancestors)?;
    }
    Ok(())
}

fn handle_child(
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    parent_id: usize,
    parent_meta: &Path,
    child: &ChildRef,
    state: &mut BuildState,
    ancestors: &mut Vec<String>,
) -> Result<(), TreeError> {
    // `ancestors` is the in-progress identity path from the root down
    // to (but excluding) this child's parent — a path-prefix set, NOT
    // a global "visited" set. A diamond reaching the same descendant
    // via two disjoint paths is therefore not a cycle (the shared
    // descendant never appears on either arm's ancestor chain).
    let identity = pack_identity_for_child(child);
    if ancestors.iter().any(|s| s == &identity) {
        let mut chain = ancestors.clone();
        chain.push(identity);
        return Err(TreeError::CycleDetected { chain });
    }

    // Parent-relative resolution: each child anchors against ITS parent
    // meta's directory, NOT a global workspace root. Matches the v1.2.0
    // `sync_meta` placement so the build pass reads from exactly where
    // the mutating pass wrote.
    let dest = parent_meta.join(child.effective_path());

    // Re-run the FS-resident boundary check (junction / reparse / `.git`-as-file).
    // This is read-only — no side effect — but rejects a hostile pre-existing
    // entry that `sync_meta` may have classified as `PresentDirty` and left in
    // place. Mirrors `Walker::handle_child`'s pre-clone check.
    check_dest_boundary(&dest, &child.effective_path())?;

    // Load child manifest — fall back to plain-git synthesis if dest has
    // `.git/` but no `.grex/pack.yaml`. Matches `Walker::handle_child`'s
    // load-fallback contract (v1.1.1 plain-git children).
    let (child_manifest, is_synthetic) = match loader.load(&dest) {
        Ok(m) => (m, false),
        Err(TreeError::ManifestNotFound(_)) if dest_has_git_repo(&dest) => {
            (synthesize_plain_git_manifest(child), true)
        }
        Err(e) => return Err(e),
    };
    verify_child_name(&child_manifest.name, child, &dest)?;
    validate_children_paths(&child_manifest)?;

    let commit_sha = probe_head_sha(backend, &dest);
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

    ancestors.push(identity);
    let result =
        walk_recursive(backend, loader, child_id, &dest, &child_manifest, state, ancestors);
    ancestors.pop();
    result
}

/// Best-effort HEAD probe — same contract as
/// [`super::walker`]'s private `probe_head_sha`. Reproduced here (rather
/// than re-exported) so this module stays self-contained — the read-only
/// build pass should be readable without bouncing to the mutating walker.
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
                target: "grex::graph_build",
                "HEAD probe failed for {}: {e}",
                dir.display()
            );
            None
        }
    }
}

fn pack_identity_for_root(path: &Path) -> String {
    format!("path:{}", path.display())
}

fn pack_identity_for_child(child: &ChildRef) -> String {
    // v1.2.3 (B2): mirror `walker.rs::pack_identity_for_child` — drop
    // the trailing `@` on empty/missing ref so the build_graph cycle
    // detector and the sync_meta cycle detector produce identical
    // identity strings (otherwise a manifest authored against one
    // surface could trip a cycle the other surface fails to see).
    match child.r#ref.as_deref() {
        Some(r) if !r.is_empty() => format!("url:{}@{}", child.url, r),
        _ => format!("url:{}", child.url),
    }
}

fn verify_child_name(got: &str, child: &ChildRef, dest: &Path) -> Result<(), TreeError> {
    let expected = child.effective_path();
    if got == expected {
        return Ok(());
    }
    Err(TreeError::PackNameMismatch { got: got.to_string(), expected, path: dest.to_path_buf() })
}

fn find_node_id_by_name_or_url(nodes: &[PackNode], dep: &str) -> Option<usize> {
    if looks_like_url(dep) {
        nodes.iter().find(|n| n.source_url.as_deref() == Some(dep)).map(|n| n.id)
    } else {
        nodes.iter().find(|n| n.name == dep).map(|n| n.id)
    }
}

fn synthesize_plain_git_manifest(child: &ChildRef) -> PackManifest {
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

/// Replicates `super::walker::validate_children_paths` for the read pass.
/// The legacy fn is `pub(super)`-visible to its module only; rather than
/// widen its visibility for a single read-only caller we mirror the
/// minimal subset here. Kept byte-equivalent in semantics — any future
/// drift between the two should be flagged by the parallel test suite
/// in `crates/grex-core/tests/tree_walk.rs`.
fn validate_children_paths(manifest: &PackManifest) -> Result<(), TreeError> {
    use crate::pack::validate::child_path::{
        boundary_reject_reason, check_one as check_child_path, nfc_duplicate_path,
    };
    if let Some(path) = nfc_duplicate_path(&manifest.children) {
        return Err(TreeError::ManifestPathEscape {
            path,
            reason: "duplicate child path under Unicode NFC normalization (case-insensitive FS collision risk)"
                .to_string(),
        });
    }
    for child in &manifest.children {
        let segment = child.path.as_deref().map_or_else(|| child.effective_path(), str::to_string);
        if let Some(reason) = boundary_reject_reason(&segment) {
            return Err(TreeError::ManifestPathEscape {
                path: segment,
                reason: reason.to_string(),
            });
        }
        let Some(err) = check_child_path(child) else { continue };
        match err {
            PackValidationError::ChildPathInvalid { child_name, path, reason } => {
                return Err(TreeError::ChildPathInvalid { child_name, path, reason });
            }
            other @ (PackValidationError::DuplicateSymlinkDst { .. }
            | PackValidationError::GraphCycle { .. }
            | PackValidationError::DependsOnUnsatisfied { .. }
            | PackValidationError::ChildPathDuplicate { .. }) => {
                tracing::error!(
                    target: "grex::graph_build",
                    "check_child_path returned unexpected variant: {other:?}",
                );
                debug_assert!(false, "check_child_path returned unexpected variant: {other:?}");
            }
        }
    }
    Ok(())
}
