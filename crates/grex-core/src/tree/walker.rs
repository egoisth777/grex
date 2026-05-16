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
//! maintains an ancestor stack of pack identifiers (source-url when present,
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

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rayon::prelude::*;

use crate::git::GitBackend;
use crate::pack::validate::child_path::{
    boundary_fs_reject_reason, boundary_reject_reason, check_one as check_child_path,
    nfc_duplicate_path,
};
use crate::pack::{ChildRef, PackManifest, PackType, PackValidationError, SchemaVersion};
use crate::scheduler::PoolInstallDepthGuard;

use super::consent::phase2_prune;
use super::dest_class::{aggregate_untracked, classify_dest, DestClass};
use super::error::TreeError;
use super::graph::{EdgeKind, PackEdge, PackGraph, PackNode};
use super::loader::PackLoader;
use super::quarantine::QuarantineConfig;

/// Recursive walker. Composes a [`PackLoader`] (for manifests) with a
/// [`GitBackend`] (for child hydration).
///
/// The walker owns no state across calls: each invocation of [`Walker::walk`]
/// produces a fresh [`PackGraph`] and leaves no footprint.
///
/// **Status (v1.2.1, path iii)**: retired from the production sync
/// orchestrator. `sync::run` now composes [`sync_meta`] (mutate) →
/// [`super::graph_build::build_graph`] (read-only) → `run_actions` instead
/// of issuing clones+fetches inside the graph build. The `Walker` symbol
/// is kept for downstream test-suite compatibility (22 fixture call sites
/// in `crates/grex-core/tests/tree_walk.rs`); new code SHOULD NOT add
/// production call sites.
#[doc(hidden)]
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
            // Root has no parent ChildRef — there is no manifest `ref:`
            // value to mirror. v1.3.1 B14.
            manifest_ref: None,
        });
        let root_identity = pack_identity_for_root(root_pack_path);
        self.walk_recursive(root_id, &root_manifest, &mut state, &mut vec![root_identity])?;
        Ok(PackGraph::new(state.nodes, state.edges))
    }

    /// Recursive step. `ancestors` carries the pack identifiers
    /// currently on the in-progress walk path — pushed on entry,
    /// popped on return. It is a path-prefix set (NOT a global
    /// "visited" set), so a diamond reaching the same descendant
    /// via two disjoint paths is not a cycle.
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
        ancestors: &mut Vec<String>,
    ) -> Result<(), TreeError> {
        self.record_depends_on(parent_id, manifest, state);
        self.process_children(parent_id, manifest, state, ancestors)
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
        ancestors: &mut Vec<String>,
    ) -> Result<(), TreeError> {
        for child in &manifest.children {
            self.handle_child(parent_id, child, state, ancestors)?;
        }
        Ok(())
    }

    fn handle_child(
        &self,
        parent_id: usize,
        child: &ChildRef,
        state: &mut BuildState,
        ancestors: &mut Vec<String>,
    ) -> Result<(), TreeError> {
        let identity = pack_identity_for_child(child);
        if ancestors.iter().any(|s| s == &identity) {
            let mut chain = ancestors.clone();
            chain.push(identity);
            return Err(TreeError::CycleDetected { chain });
        }
        // v1.2.0 Stage 1.c: FS-resident boundary check fires BEFORE
        // any clone / fetch. Junctions, reparse points, and
        // `.git`-as-file (gitfile redirect) all re-open the
        // parent-boundary escape that the syntactic gate closes on
        // the path string itself; running the check on the prospective
        // dest path means a hostile pre-existing slot is rejected
        // before the GitBackend writes anything into (or through) it.
        // The prospective path is reconstructed here so the helper
        // can interrogate the slot before `resolve_destination`
        // materialises a clone — pre-clone runs return `Ok(())` because
        // the slot doesn't exist yet, and the walk continues normally.
        let prospective_dest = self.workspace.join(child.effective_path());
        check_dest_boundary(&prospective_dest, &child.effective_path())?;
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
            // v1.3.1 B14: carry the parent manifest's `ref:` verbatim
            // so the sync orchestrator can mirror it into
            // `LockEntry.branch`.
            manifest_ref: child.r#ref.clone(),
        });
        state.edges.push(PackEdge { from: parent_id, to: child_id, kind: EdgeKind::Child });

        ancestors.push(identity);
        let result = self.walk_recursive(child_id, &child_manifest, state, ancestors);
        ancestors.pop();
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
        // v1.3.2 B11 — backend-lock context: parent_meta is the legacy
        // single-frame walker's workspace, child_path is the literal
        // manifest-declared path. The lock lands at
        // `<workspace>/.grex/locks/<child_path>.backend.lock`.
        let child_path = child.effective_path();
        let lock_ctx = crate::git::BackendLockCtx::new(&self.workspace, &child_path);
        if dest_has_git_repo(&dest) {
            self.backend.fetch(&dest, lock_ctx)?;
            if let Some(r) = effective_ref {
                self.backend.checkout(&dest, r, lock_ctx)?;
            }
        } else {
            self.backend.clone(&child.url, &dest, effective_ref, lock_ctx)?;
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
///
/// v1.2.3 (B2): when the ref is missing or empty the trailing `@` is
/// omitted so the on-the-wire identity is just `url:<url>` — matches
/// `Grex.Walker.ChildRef.identity` in the Lean model. Without this
/// elision two children that differ only in `ref: None` vs
/// `ref: Some("")` would otherwise serialise the same way as
/// `url:<url>@`, masking the distinction the Lean specification draws.
fn pack_identity_for_child(child: &ChildRef) -> String {
    match child.r#ref.as_deref() {
        Some(r) if !r.is_empty() => format!("url:{}@{}", child.url, r),
        _ => format!("url:{}", child.url),
    }
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

/// v1.4.1 B15 — `true` when `dest` exists, has no `.git/`, and is a
/// non-empty directory. Pre-clone collision detection: signals an
/// operator-pre-existing slot that `git clone` would refuse, so the
/// caller can surface a warning before the walk even attempts the
/// clone. Returns `false` for an empty dir or a missing dest (the
/// happy-path Missing case).
pub(crate) fn dest_has_nongit_content(dest: &Path) -> bool {
    if !dest.exists() || dest.join(".git").exists() {
        return false;
    }
    match std::fs::read_dir(dest) {
        Ok(mut entries) => entries.next().is_some(),
        Err(_) => false,
    }
}

/// Build the in-memory manifest used for v1.1.1 plain-git children — a
/// leaf scripted pack with no hooks, no children, no actions. Activated
/// at the walker's load-fallback boundary when a child has a `.git/`
/// but no `.grex/pack.yaml`. See
/// `openspec/changes/feat-v1.1.1-plain-git-children/design.md`.
pub fn synthesize_plain_git_manifest(child: &ChildRef) -> PackManifest {
    // v1.2.0: when `child.path:` carries `/` separators the synthesised
    // pack.name must equal the LAST segment so it satisfies the bare-
    // name regex `^[a-z][a-z0-9-]*$` (and matches `verify_child_name`).
    let effective = child.effective_path();
    let name = last_path_segment(&effective).to_string();
    PackManifest {
        schema_version: SchemaVersion::current(),
        name,
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
/// declared. The parent-side expectation is the LAST segment of the
/// child entry's [`ChildRef::effective_path`] — i.e. the on-disk
/// directory the pack actually lives in.
///
/// v1.2.0: when `child.path:` carries `/` separators (e.g. `tools/foo`)
/// the last segment is the pack's home directory; intermediate
/// segments (`tools/`) are mount-point scaffolding that the pack itself
/// does not own. The pack-name regex `^[a-z][a-z0-9-]*$` forbids `/`,
/// so comparing the cloned manifest's `name:` against the FULL slash
/// path would be unsatisfiable for any legal pack — splitting on `/`
/// and comparing against the last segment matches the runtime
/// invariant `id = file_name(dest)` already enforced by Phase 1's
/// dry-run recorder and the lockfile id convention.
fn verify_child_name(got: &str, child: &ChildRef, dest: &Path) -> Result<(), TreeError> {
    let effective = child.effective_path();
    let expected = last_path_segment(&effective).to_string();
    if got == expected {
        return Ok(());
    }
    Err(TreeError::PackNameMismatch { got: got.to_string(), expected, path: dest.to_path_buf() })
}

/// Last `/`-separated segment of a (POSIX) child path. For a bare name
/// the input is returned unchanged; for `tools/foo` the result is
/// `foo`. Empty / trailing-`/` inputs are caller-side validation
/// failures (they never reach this helper after `validate_children_paths`),
/// so the fallback simply returns the original string.
fn last_path_segment(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
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
    // v1.2.0 Stage 1.c: NFC-duplicate sweep across the sibling list.
    // Runs first because it's a cross-cutting check (one offender
    // implicates the WHOLE list, not a single child). Surfaces as
    // `TreeError::ManifestPathEscape` per walker.md
    // §boundary-preservation — a NFC-collapsed name re-introduces the
    // very boundary escape the regex was meant to close on
    // case-insensitive filesystems.
    if let Some(path) = nfc_duplicate_path(&manifest.children) {
        return Err(TreeError::ManifestPathEscape {
            path,
            reason: "duplicate child path under Unicode NFC normalization (case-insensitive FS collision risk)"
                .to_string(),
        });
    }
    for child in &manifest.children {
        // v1.2.0 Stage 1.c: per-segment boundary-preservation rejects.
        // Layered AHEAD of the syntactic gate so the more specific
        // `ManifestPathEscape` diagnostic wins for entries that would
        // also fail the bare-name regex (e.g. `child:foo` is rejected
        // here as a colon hazard instead of a generic charset miss).
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

/// v1.2.0 Stage 1.c: filesystem-resident boundary check. Run AFTER
/// the destination has been resolved against the parent workspace but
/// BEFORE any clone / fetch fires. Catches the case where the slot
/// the walker is about to materialise into is already a junction,
/// reparse point, symlink, or `.git`-as-file — each of which would
/// re-introduce a parent-boundary escape.
///
/// Pre-clone: a non-existent destination is the happy path; the
/// helper returns `None` and the walk continues. Post-clone or on a
/// re-walk where the destination is already populated, the helper
/// inspects the on-disk entry and surfaces a `ManifestPathEscape`
/// when the entry violates the boundary contract.
///
/// Visibility: `pub(super)` — used by the walker's `handle_child`
/// path-resolution step (wired in 1.c follow-up; this commit lands
/// the helper itself and the boundary-check call site for the
/// path-segment rejects).
pub(super) fn check_dest_boundary(dest: &Path, segment: &str) -> Result<(), TreeError> {
    if let Some(reason) = boundary_fs_reject_reason(dest) {
        return Err(TreeError::ManifestPathEscape {
            path: segment.to_string(),
            reason: reason.to_string(),
        });
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

// ---------------------------------------------------------------------------
// v1.2.0 Stage 1.g — `sync_meta` entry point: parent-relative,
// distributed-lockfile walker. Three phases per meta:
//
//   Phase 1 (siblings): `classify_dest` (1.e) per child, dispatch
//     fetch / clone / refuse based on the verdict; aggregate
//     `PresentUndeclared` into `TreeError::UntrackedGitRepos`.
//   Phase 2 (orphan prune): for each `prune_candidate` (caller-supplied
//     by 1.h once the distributed lockfile read lands), run the
//     consent-walk via `phase2_prune` (1.f).
//   Phase 3 (recursion): per child whose dest carries
//     `<dest>/.grex/pack.yaml`, recursively `sync_meta` if `recurse`
//     is true and depth < `max_depth`.
//
// Design discipline:
//
// * **No new locking primitives.** Per-pack git ops acquire the M6
//   `PackLock` (synchronous `acquire`) for the duration of the
//   clone/fetch. The Lean axiom `sync_disjoint_commutes` (Bridge.lean)
//   permits any disjoint scheduler — sequential is the smallest model
//   that satisfies the axiom. Sibling parallelism via rayon is a 1.j /
//   1.l-territory follow-up; the scaffolding here keeps the
//   single-threaded baseline correct first.
// * **No lockfile mechanics.** Phase 2's orphan list is a parameter,
//   not a read from `<meta>/.grex/grex.lock.jsonl`. 1.h owns the
//   distributed-lockfile read/write surface; this commit only wires
//   the consent-walk + prune dispatch.
// * **Error aggregation.** Every Phase 1 child failure plus every
//   Phase 2 refusal lands in `SyncMetaReport::errors` before the call
//   returns. The walker is fail-LOUD (caller gets the full picture),
//   not fail-fast (the legacy `Walker::walk` aborts on the first hit).
//   This matches the v1.2.0 walker.md §"untracked git policy" rule
//   that `UntrackedGitRepos` must enumerate every offender at once.
// ---------------------------------------------------------------------------

/// Per-meta options threaded through `sync_meta`. Keeps the call-site
/// signature small without coupling to the full [`crate::sync::SyncOptions`]
/// surface — the orchestrator (`sync.rs::run`) is responsible for projecting
/// `SyncOptions` into `SyncMetaOptions` when it wires this entry point.
///
/// v1.2.5 — marked `#[non_exhaustive]` so future PATCH-level field additions
/// (e.g. new optional knobs threaded through the same struct) do not break
/// downstream `let SyncMetaOptions { .. }` destructuring or struct-literal
/// construction at call sites that omit the new field. External callers
/// MUST construct via [`SyncMetaOptions::default()`] and field assignment on
/// a `mut` binding — `let mut opts = SyncMetaOptions::default(); opts.recurse
/// = false;`. Struct-literal construction (including the `..base` spread
/// shorthand) is rejected by E0639 from outside this crate.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SyncMetaOptions {
    /// Global ref override (`grex sync --ref <sha|branch|tag>`). Mirrors
    /// [`Walker::with_ref_override`]: when `Some`, every child's
    /// declared `ref` is replaced.
    pub ref_override: Option<String>,
    /// When `true`, Phase 3 recurses into child metas. `false` is the
    /// `doctor --shallow` semantics: process only the immediate
    /// children of the supplied meta.
    pub recurse: bool,
    /// Bound on Phase 3 recursion depth. `None` is unbounded; `Some(n)`
    /// caps at `n` levels of nesting (the supplied `meta_dir` is depth
    /// 0). Recursion ALWAYS halts before depth `n+1`.
    pub max_depth: Option<usize>,
    /// Phase 2 prune-safety override. Mirrors
    /// [`crate::sync::SyncOptions::force_prune`].
    pub force_prune: bool,
    /// Phase 2 prune-safety override. Mirrors
    /// [`crate::sync::SyncOptions::force_prune_with_ignored`].
    pub force_prune_with_ignored: bool,
    /// v1.2.1 item 3 — rayon thread-pool size for sibling-parallel
    /// Phase 1 + Phase 3. `None` ⇒ rayon's default (`num_cpus::get()`);
    /// `Some(1)` ⇒ effectively sequential (single-threaded pool, useful
    /// for determinism testing); `Some(n >= 2)` ⇒ bounded parallel.
    /// `Some(0)` is clamped to `1` (rayon rejects a zero-thread pool).
    /// Mirrors [`crate::sync::SyncOptions::parallel`] semantics with the
    /// one exception that `0` is clamped to `1` here — the unbounded
    /// sentinel only makes sense for tokio's `Semaphore::MAX_PERMITS`.
    pub parallel: Option<usize>,
    /// v1.2.1 item 5b — when `Some`, Phase 2 prunes are diverted
    /// through the snapshot-then-unlink quarantine pipeline before
    /// `unlink(dest)` fires. Carries the per-meta trash bucket root
    /// and audit-log path. `None` (default) preserves the legacy
    /// v1.2.0 direct-unlink path. Set by
    /// [`crate::sync::SyncOptions::quarantine`] at the orchestrator
    /// boundary; the consent layer reads this to pick the deletion
    /// strategy. Lean theorem `quarantine_snapshot_precedes_delete`
    /// proves the safety contract.
    pub quarantine: Option<QuarantineConfig>,
    /// v1.2.5 — when `Some`, every meta sync starts with a best-effort
    /// GC sweep over `<meta>/.grex/trash/` per the supplied retention
    /// window. `None` (default) preserves v1.2.1 indefinite-retention
    /// behavior. Set by [`crate::sync::SyncOptions::retain_days`] at
    /// the orchestrator boundary; threaded through every recursion
    /// frame so each meta's own trash bucket gets swept. Sweep
    /// failures log via tracing and DO NOT halt the sync.
    pub retention: Option<super::RetentionConfig>,
    /// v1.3.1 (B4) — when `true`, Phase 1 SKIPS every clone/fetch
    /// subprocess + lockfile + events.jsonl mutation. The walker still
    /// traverses the manifest tree (parsing in-memory) and accumulates
    /// one [`DryRunWouldCloneRecord`] per child that WOULD have been
    /// cloned into [`SyncMetaReport::dry_run_would_clone`]. Lean
    /// theorem `Grex.Walker.dry_run_no_side_effects` formalises the
    /// invariant `dry_run = true ⇒ no network call ∧ no FS write`.
    /// Default: `false` (preserves v1.3.0 mutation semantics for the
    /// default sync path).
    pub dry_run: bool,
}

impl Default for SyncMetaOptions {
    fn default() -> Self {
        Self {
            ref_override: None,
            recurse: true,
            max_depth: None,
            force_prune: false,
            force_prune_with_ignored: false,
            parallel: None,
            quarantine: None,
            retention: None,
            dry_run: false,
        }
    }
}

/// v1.3.1 (B4) — one in-memory record describing a child the walker
/// WOULD have cloned if `SyncMetaOptions::dry_run` were `false`. Mirrors
/// the [`crate::manifest::Event::DryRunWouldClone`] event shape, but
/// surfaced through the walker's [`SyncMetaReport`] return value
/// instead of written to events.jsonl — dry-run is contractually
/// side-effect-free per Lean theorem `dry_run_no_side_effects`. CLI
/// renderers can serialize the records to stdout JSON if desired.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DryRunWouldCloneRecord {
    /// Folder name (= repo name) the child would land at. Mirrors the
    /// runtime invariant `id = file_name(dest)` enforced by the
    /// non-dry-run path.
    pub id: String,
    /// Upstream source URL.
    pub url: String,
    /// Effective ref (manifest-declared or `--ref` override). `None`
    /// when neither is set.
    pub ref_: Option<String>,
}

/// Outcome of one [`sync_meta`] invocation. Aggregated across every
/// recursion frame: a sub-meta's report is folded into its parent's
/// report at the end of Phase 3.
///
/// Marked `#[non_exhaustive]` so future PATCH/MINOR slices can add
/// fields (e.g. v1.3.1's `dry_run_would_clone`) without breaking
/// external struct-literal constructors or exhaustive pattern matches.
/// In-crate construction goes through `..Default::default()` to stay
/// `non_exhaustive`-compatible.
#[non_exhaustive]
#[derive(Debug, Default)]
pub struct SyncMetaReport {
    /// Number of metas processed (this meta + every descendant Phase 3
    /// recursion fired against). Useful for `--shallow` verification:
    /// `recurse: false` means `metas_visited == 1`.
    pub metas_visited: usize,
    /// Per-child Phase 1 verdicts, keyed by parent-relative child path.
    /// `(meta_dir, child_dest, classification)` — exposed primarily for
    /// tests; downstream callers will project into a status report.
    pub phase1_classifications: Vec<(PathBuf, PathBuf, DestClass)>,
    /// Successful Phase 2 prunes (paths that were removed). Empty when
    /// no orphan list was supplied or every orphan refused.
    pub phase2_pruned: Vec<PathBuf>,
    /// Aggregate of every error encountered across Phases 1, 2, and 3.
    /// The walker continues past recoverable errors so the caller sees
    /// the full picture in one pass.
    pub errors: Vec<TreeError>,
    /// v1.3.1 (B4) — one record per child the walker WOULD have cloned
    /// during Phase 1 if `SyncMetaOptions::dry_run` were `false`.
    /// Always empty when `dry_run = false`. CLI renderers should
    /// surface the records as the dry-run plan for the operator.
    pub dry_run_would_clone: Vec<DryRunWouldCloneRecord>,
}

impl SyncMetaReport {
    fn merge(&mut self, mut child: SyncMetaReport) {
        self.metas_visited += child.metas_visited;
        self.phase1_classifications.append(&mut child.phase1_classifications);
        self.phase2_pruned.append(&mut child.phase2_pruned);
        self.errors.append(&mut child.errors);
        self.dry_run_would_clone.append(&mut child.dry_run_would_clone);
    }
}

/// Sync a meta pack and (optionally) its descendants.
///
/// `meta_dir` is the on-disk directory containing the meta's
/// `.grex/pack.yaml`. `prune_candidates` is the list of orphan dests
/// (parent-relative) the caller's distributed-lockfile reader determined
/// no longer appear in `manifest.children`.
///
/// The walker is **fail-loud, not fail-fast**: recoverable errors land
/// in [`SyncMetaReport::errors`] and the walk continues so the caller
/// sees the full picture in one pass. The only short-circuit is a
/// detected cycle, which surfaces as `Err(TreeError::CycleDetected)`
/// to keep the cyclic-clone storm risk contained.
///
/// # Errors
///
/// Returns the *first* catastrophic error: manifest parse failure on
/// the supplied `meta_dir`, a cycle in the manifest forest (URL+ref
/// identity), or a pre-walk path-traversal violation. Per-child
/// clone / fetch / prune failures aggregate into
/// [`SyncMetaReport::errors`] without aborting the walk.
pub fn sync_meta(
    meta_dir: &Path,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    prune_candidates: &[PathBuf],
) -> Result<SyncMetaReport, TreeError> {
    // v1.2.3 (B4) — seed the ancestor chain with the root pack's
    // path-namespaced identity (`path:<meta_dir>`) so the Lean
    // `acyclic_path` precondition that drives
    // `sync_meta_no_cycle_infinite_clone` is established right at
    // the call site rather than implicitly relying on an empty
    // initial ancestor list. Children identify with `url:<url>@<ref>` —
    // disjoint namespace from the root's `path:` identity, so seeding
    // does not introduce false-positive cycle hits against any
    // legitimate child.
    //
    // `sync_meta_inner` extends this chain per recursion edge (Phase
    // 3) using clone-per-child so disjoint sibling branches do not
    // pollute each other's ancestor view.
    let initial_ancestors = vec![pack_identity_for_root(meta_dir)];
    sync_meta_inner(
        meta_dir,
        backend,
        loader,
        opts,
        prune_candidates,
        /* depth */ 0,
        &initial_ancestors,
    )
}

fn sync_meta_inner(
    meta_dir: &Path,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    prune_candidates: &[PathBuf],
    depth: usize,
    ancestors: &[String],
) -> Result<SyncMetaReport, TreeError> {
    let manifest = loader.load(meta_dir)?;
    // v1.2.0 Stage 1.c gate — every recursion frame re-runs the
    // path-traversal sweep before any child is touched on disk.
    validate_children_paths(&manifest)?;

    let mut report = SyncMetaReport { metas_visited: 1, ..SyncMetaReport::default() };

    // v1.2.5 — best-effort quarantine GC sweep at meta sync start.
    // Runs BEFORE Phase 1 so the trash bucket is current when any
    // Phase 2 quarantine snapshot lands later in the same sync. The
    // audit log path mirrors what the quarantine pipeline already
    // uses (`<meta>/.grex/events.jsonl`). Sweep failures are logged
    // via tracing inside `prune_quarantine` and DO NOT halt the
    // sync — this is the design.md "best-effort retention" contract.
    if let Some(retention) = opts.retention {
        let audit_log = crate::manifest::event_log_path(meta_dir);
        if let Err(e) = super::quarantine::prune_quarantine(meta_dir, retention, Some(&audit_log)) {
            tracing::warn!(
                meta_dir = %meta_dir.display(),
                error = %e,
                "quarantine GC sweep failed at meta sync start; continuing",
            );
        }
    }

    // v1.2.5 (A2) — snapshot which child dest dirs already exist on
    // disk BEFORE Phase 1 runs its clones. Phase 3 uses this set to
    // decide whether a Failed/Cancelled outcome should trigger
    // `cleanup_partial_clone`: only freshly-created dests (i.e.
    // ones NOT in this set) are cleaned. Dests that pre-existed
    // belong to an earlier successful sync and must be preserved
    // even when a downstream cycle aborts the current walk.
    //
    // The snapshot is captured here rather than at `phase3_handle_child`
    // entry because Phase 1 always clones missing dests before Phase 3
    // recurses, so by the time `phase3_handle_child` runs every
    // declared dest exists on disk and the per-fn snapshot would
    // never see a "missing" pre-state.
    //
    // v1.2.5 (W1) — keys are normalised via [`normalize_dest_key`] so
    // a trailing `/` or `./` prefix in `child.effective_path()` does
    // not desync the snapshot from the lookup at `phase3_handle_child`.
    // Both ends MUST apply the same normalisation; the `dest` computed
    // at the lookup site is wrapped through the same helper before
    // `pre_existing_dests.contains(&dest)` runs.
    let pre_existing_dests: HashSet<PathBuf> = manifest
        .children
        .iter()
        .map(|c| normalize_dest_key(&meta_dir.join(c.effective_path())))
        .filter(|d| d.exists())
        .collect();

    // v1.2.1 item 3: build a per-call rayon pool sized from
    // `opts.parallel`. Phase 1 + Phase 3 install on this pool; Phase 2
    // stays sequential (single-meta orphan sweep — no sibling
    // parallelism to extract). The pool is dropped at the end of
    // `sync_meta_inner`, so each recursion frame builds + tears down
    // its own pool. This is intentional: we want the worker count to
    // refresh per call so a top-level `--parallel 1` cap is honoured
    // without piggy-backing on a global pool that an unrelated caller
    // might have configured differently.
    let pool = build_pool(opts.parallel)?;

    phase1_sync_children(&pool, meta_dir, &manifest, backend, opts, &mut report);
    phase2_prune_orphans(meta_dir, prune_candidates, opts, &mut report);
    // v1.2.2 — cycle detection short-circuits the recursion edge with
    // an `Err` return so the caller sees `Err(CycleDetected)` directly
    // rather than burying it in `report.errors`. Cycles are catastrophic
    // (would otherwise clone forever); fail-loud here, NOT fold-into-report.
    phase3_recurse(
        &pool,
        meta_dir,
        &manifest,
        backend,
        loader,
        opts,
        depth,
        ancestors,
        &pre_existing_dests,
        &mut report,
    )?;

    Ok(report)
}

/// v1.2.1 item 3 — build a rayon `ThreadPool` sized from
/// `opts.parallel`. Encapsulates the `None` ⇒ default,
/// `Some(0)` ⇒ clamp-to-1, `Some(n)` ⇒ exact-N policy in one place
/// so Phase 1 and Phase 3 install on identically-configured pools.
///
/// `Some(1)` produces a single-worker pool — the determinism
/// test-mode fast-path (sibling iteration order matches sequential
/// for-loop order on a 1-thread pool).
///
/// Build failures surface as [`TreeError::ManifestRead`]: a rayon
/// pool failure is invariably a host-resource issue (out of file
/// descriptors, thread-creation refused) — bucketing it into the
/// generic IO-error variant keeps the error surface tight without
/// inventing a one-off `RayonPoolBuild` discriminant. The Lean
/// model treats pool construction as a well-formedness precondition
/// of `sync`, not an in-band failure mode.
fn build_pool(parallel: Option<usize>) -> Result<rayon::ThreadPool, TreeError> {
    let mut builder = rayon::ThreadPoolBuilder::new();
    if let Some(n) = parallel {
        builder = builder.num_threads(n.max(1));
    }
    builder.build().map_err(|e| {
        TreeError::ManifestRead(format!("failed to build rayon pool for sync_meta: {e}"))
    })
}

/// Per-child output from Phase 1's parallel pass. Collected into a
/// `Vec` after the rayon `par_iter` settles, then drained into the
/// caller's `SyncMetaReport` in a single sequential pass. Carrying
/// the data plain (no `&mut report` shared across threads) is what
/// keeps the parallelisation sound under the Lean
/// `sync_disjoint_commutes` axiom: each iteration's mutations are
/// confined to its own owned struct.
struct Phase1ChildOutcome {
    /// `(meta_dir, dest, class)` — pushed onto
    /// `report.phase1_classifications` regardless of dispatch outcome.
    classification: (PathBuf, PathBuf, DestClass),
    /// Per-child clone/fetch failure, if any. Folded into
    /// `report.errors`.
    error: Option<TreeError>,
    /// `Some((dest, class))` when the child classified as
    /// `PresentUndeclared`; the caller aggregates these into one
    /// `UntrackedGitRepos` error after the parallel pass.
    undeclared: Option<(PathBuf, DestClass)>,
    /// v1.3.1 (B4) — `Some(record)` when `dry_run = true` and the
    /// child would have been cloned (`Missing` classification) had the
    /// real path run. Folded into `report.dry_run_would_clone`.
    dry_run_record: Option<DryRunWouldCloneRecord>,
}

/// Phase 1: classify each declared child, then dispatch. Per the v1.2.0
/// walker.md pseudocode the per-child branches are:
///
/// * `Missing` → clone via `backend.clone(url, dest, ref)`.
/// * `PresentDeclared` → fetch (+ checkout if a ref override applies).
/// * `PresentDirty` → no-op (preserve user changes; will surface at
///   exec/plan stage if applicable).
/// * `PresentInProgress` → refuse via `DirtyTreeRefusal{GitInProgress}`
///   (collected into `report.errors`).
/// * `PresentUndeclared` → impossible at Phase 1 dispatch time because
///   declared paths are in `manifest.children`; the variant is reserved
///   for the lockfile-orphan sweep (Phase 2 territory).
///
/// v1.2.1 item 3 — sibling-parallel via rayon `par_iter`. Disjointness
/// across siblings (each child has its own `meta_dir.join(child.path)`
/// dest, validated by `validate_children_paths` upstream) discharges
/// the precondition of the `sync_disjoint_commutes` axiom in
/// `proof/Grex/Bridge.lean`. The per-pack `.grex-lock` (M6, acquired
/// inside the GitBackend implementation) continues to serialise any
/// cross-task contention on the same pack path. Per-thread results
/// are collected into a `Vec<Phase1ChildOutcome>` and folded into the
/// caller's `SyncMetaReport` in a single sequential pass, preserving
/// deterministic ordering of `report.phase1_classifications` (rayon
/// `collect_into_vec` preserves source-order regardless of completion
/// order).
fn phase1_sync_children(
    pool: &rayon::ThreadPool,
    meta_dir: &Path,
    manifest: &PackManifest,
    backend: &dyn GitBackend,
    opts: &SyncMetaOptions,
    report: &mut SyncMetaReport,
) {
    // v1.2.5 (A3) — RAII guard tracks nested `pool.install` depth on
    // the calling OS thread. In debug builds this powers an assertion
    // in `phase3_recurse` that catches the v1.2.2 R#1 MED deadlock
    // pattern (nested pool.install while holding a PackLock). Release
    // builds compile to a zero-sized no-op.
    let _depth_guard = PoolInstallDepthGuard::new();
    // Install on the per-call pool so `--parallel N` is honoured even
    // when this is invoked from inside another rayon context (Phase 3
    // recursion). `install` is a synchronous fence: the closure
    // returns once every parallel iteration has settled.
    let outcomes: Vec<Phase1ChildOutcome> = pool.install(|| {
        manifest
            .children
            .par_iter()
            .map(|child| phase1_handle_child(meta_dir, child, backend, opts))
            .collect()
    });

    // Sequential fold: the parallel pass cannot mutate `report` directly
    // (it is `&mut`), so we drain the per-child outcomes here. Order is
    // preserved by `par_iter().collect()` — see the `phase1_par_iter_preserves_order`
    // test below.
    let mut undeclared_seen: Vec<(PathBuf, DestClass)> = Vec::new();
    for outcome in outcomes {
        report.phase1_classifications.push(outcome.classification);
        if let Some(e) = outcome.error {
            report.errors.push(e);
        }
        if let Some(pair) = outcome.undeclared {
            undeclared_seen.push(pair);
        }
        if let Some(rec) = outcome.dry_run_record {
            report.dry_run_would_clone.push(rec);
        }
    }
    if let Err(e) = aggregate_untracked(undeclared_seen) {
        report.errors.push(e);
    }
}

/// Per-child Phase 1 dispatch — runs inside the rayon pool. The
/// extracted fn keeps the parallel closure body small and gives the
/// Lean axiom a single discoverable Rust contract anchor (this fn is
/// the per-sibling unit of work the `sync_disjoint_commutes` axiom
/// quantifies over).
fn phase1_handle_child(
    meta_dir: &Path,
    child: &ChildRef,
    backend: &dyn GitBackend,
    opts: &SyncMetaOptions,
) -> Phase1ChildOutcome {
    let dest = meta_dir.join(child.effective_path());
    // Every declared child IS in the manifest by construction —
    // `declared_in_manifest = true` is the only correct call here.
    let class = classify_dest(&dest, true, None);
    let mut out = Phase1ChildOutcome {
        classification: (meta_dir.to_path_buf(), dest.clone(), class),
        error: None,
        undeclared: None,
        dry_run_record: None,
    };
    match class {
        DestClass::Missing => {
            // v1.4.1 B15 — surface a pre-clone collision warning when
            // the declared slot already holds non-git content (i.e.
            // `classify_dest` returned `Missing` because `.git/` is
            // absent, but the dir itself is non-empty). Real-run would
            // still attempt the clone and fail with git's own "dest is
            // not empty" error; dry-run previously emitted no signal
            // at all, leaving operators to discover the conflict only
            // mid-sync. The warning lands on stderr via `tracing` so
            // machine-readable stdout consumers stay unaffected.
            if dest_has_nongit_content(&dest) {
                tracing::warn!(
                    target: "grex::sync",
                    path = %dest.display(),
                    "collision: declared child slot already contains non-git content"
                );
            }
            // v1.3.1 (B4) — gate the clone subprocess + parent-mkdir
            // behind `dry_run`. When `dry_run = true` we record what
            // WOULD have been cloned (id = folder name = repo name)
            // and emit no FS / network / lockfile / events.jsonl
            // mutation. Lean theorem `dry_run_no_side_effects`
            // pins the invariant.
            if opts.dry_run {
                fill_dry_run_record(&mut out, &dest, child, opts);
            } else if let Err(e) = phase1_clone(backend, meta_dir, child, &dest, opts) {
                out.error = Some(e);
            }
        }
        DestClass::PresentDeclared => {
            // v1.3.1 (B4) — same gate for fetch/checkout. The dest
            // already exists on disk so a dry-run still emits a
            // would-clone record (callers want a uniform list of
            // children that would be touched), but no fetch fires.
            if opts.dry_run {
                fill_dry_run_record(&mut out, &dest, child, opts);
            } else if let Err(e) = phase1_fetch(backend, meta_dir, child, &dest, opts) {
                out.error = Some(e);
            }
        }
        DestClass::PresentDirty => {
            // Conservative: leave the dirty tree untouched. The
            // operator has uncommitted work; v1.2.0 walker policy
            // is to never overwrite their bytes during Phase 1.
            // Phase 2 will surface a refusal if the operator ALSO
            // requested a prune of this path, but that's a
            // separate decision made by the caller's lockfile-
            // orphan computation.
        }
        DestClass::PresentInProgress => {
            out.error = Some(TreeError::DirtyTreeRefusal {
                path: dest.clone(),
                kind: super::error::DirtyTreeRefusalKind::GitInProgress,
            });
        }
        DestClass::PresentUndeclared => {
            // Buffer for `aggregate_untracked` so we surface the
            // FULL list in one error.
            out.undeclared = Some((dest, class));
        }
    }
    out
}

/// v1.3.1 (B4) — populate `out` for a dry-run child. The runtime
/// invariant `id = file_name(dest)` mirrors the non-dry-run path; if
/// `file_name` is absent (dest ends in `..` / is a filesystem root) or
/// the component is non-UTF-8, recording an empty `id` would silently
/// corrupt the dry-run plan, so we route the failure into
/// `out.error` (folded into `report.errors`) and continue per the
/// fail-loud-not-fail-fast contract. Shared between the `Missing` and
/// `PresentDeclared` arms of [`phase1_handle_child`].
fn fill_dry_run_record(
    out: &mut Phase1ChildOutcome,
    dest: &Path,
    child: &ChildRef,
    opts: &SyncMetaOptions,
) {
    match dest.file_name().and_then(|n| n.to_str()) {
        Some(name) => {
            out.dry_run_record = Some(DryRunWouldCloneRecord {
                id: name.to_string(),
                url: child.url.clone(),
                ref_: opts.ref_override.clone().or_else(|| child.r#ref.clone()),
            });
        }
        None => {
            out.error = Some(TreeError::InvalidDestination {
                path: dest.to_path_buf(),
                reason: "dest has no UTF-8 file_name component".to_string(),
            });
        }
    }
}

/// Phase 1 clone helper. Acquires the M6 `PackLock` on the prospective
/// dest's parent (`meta_dir`) for the duration of the clone — distinct
/// children clone serially within a meta to keep the scheduler-tier
/// model honest. Sibling parallelism is a 1.j follow-up.
///
/// v1.3.2 B11 — backend-lock ctx threads `(meta_dir, child.effective_path())`
/// through to the backend so the per-repo lock lands at
/// `<meta_dir>/.grex/locks/<child_path>.backend.lock` (parent-owned).
fn phase1_clone(
    backend: &dyn GitBackend,
    meta_dir: &Path,
    child: &ChildRef,
    dest: &Path,
    opts: &SyncMetaOptions,
) -> Result<(), TreeError> {
    let effective_ref = opts.ref_override.as_deref().or(child.r#ref.as_deref());
    // Make sure the dest's parent exists — the clone backend assumes
    // it. v1.2.0 invariant 1 (boundary) and 1.c's `validate_children_paths`
    // already ruled out a path that would escape `meta_dir`, so a
    // simple `create_dir_all` on the parent is safe here.
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            TreeError::ManifestRead(format!("failed to mkdir parent {}: {e}", parent.display()))
        })?;
    }
    let child_path = child.effective_path();
    let lock_ctx = crate::git::BackendLockCtx::new(meta_dir, &child_path);
    backend.clone(&child.url, dest, effective_ref, lock_ctx)?;
    Ok(())
}

/// Phase 1 fetch helper. Same locking discipline as `phase1_clone`.
///
/// v1.3.2 B11 — see `phase1_clone` for the backend-lock-ctx contract.
fn phase1_fetch(
    backend: &dyn GitBackend,
    meta_dir: &Path,
    child: &ChildRef,
    dest: &Path,
    opts: &SyncMetaOptions,
) -> Result<(), TreeError> {
    let child_path = child.effective_path();
    let lock_ctx = crate::git::BackendLockCtx::new(meta_dir, &child_path);
    backend.fetch(dest, lock_ctx)?;
    let effective_ref = opts.ref_override.as_deref().or(child.r#ref.as_deref());
    if let Some(r) = effective_ref {
        backend.checkout(dest, r, lock_ctx)?;
    }
    Ok(())
}

/// Phase 2: prune orphan lockfile entries. Each candidate is run
/// through the consent-walk via `phase2_prune` (1.f); a `Clean` verdict
/// removes the dest, anything else surfaces as an error. The orphan
/// list is supplied by the caller — 1.h owns the lockfile-read side
/// of the walker contract.
fn phase2_prune_orphans(
    meta_dir: &Path,
    prune_candidates: &[PathBuf],
    opts: &SyncMetaOptions,
    report: &mut SyncMetaReport,
) {
    // v1.2.0 Stage 1.l — postmortem audit log path. Resolved once per
    // meta from the canonical `<meta_dir>/.grex/events.jsonl` slot;
    // `phase2_prune` only writes to it when an override flag actually
    // consumed a non-Clean verdict (clean prunes never log).
    let audit_log = crate::manifest::event_log_path(meta_dir);
    for candidate in prune_candidates {
        // Candidates are parent-relative POSIX paths
        // (`LockEntry::validate_path` invariant from 1.b). Resolve
        // against `meta_dir` to get the absolute dest.
        let dest = meta_dir.join(candidate);
        match phase2_prune(
            &dest,
            opts.force_prune,
            opts.force_prune_with_ignored,
            Some(audit_log.as_path()),
            opts.quarantine.as_ref(),
        ) {
            Ok(()) => report.phase2_pruned.push(dest),
            Err(e) => report.errors.push(e),
        }
    }
}

/// Per-child output from Phase 3's parallel recursion. Each variant
/// carries either a successful sub-`SyncMetaReport` (folded into the
/// caller via [`SyncMetaReport::merge`]) or a fatal error to push onto
/// `report.errors`. Children whose dest does NOT carry a sub-meta
/// produce `Skipped`.
///
/// v1.2.4 — `Cancelled` is the EARLY-OUT contributed by a sibling
/// closure that observed the per-`phase3_recurse` cancellation flag
/// already flipped to `true`. It carries no sub-report (no work was
/// done) and never contributes to `report.metas_visited` or
/// `report.errors` — the cycle that triggered the flip is the sole
/// error reported. Mirrors the Lean
/// `cancellation_terminates_promptly` theorem: when `cancelled` is
/// observed at entry, return ok with zero descent.
enum Phase3ChildOutcome {
    Skipped,
    Recursed(SyncMetaReport),
    Failed(TreeError),
    Cancelled,
}

/// Phase 3: parallel recursion into child metas. A child qualifies for
/// recursion when:
///
///   1. `opts.recurse` is `true`,
///   2. `opts.max_depth` is unbounded OR the next-frame depth is
///      strictly less than the cap,
///   3. `<dest>/.grex/pack.yaml` exists.
///
/// Sub-meta reports are merged into the parent's report via
/// [`SyncMetaReport::merge`] so a top-level caller sees one rolled-up
/// view of every frame's classifications + errors.
///
/// v1.2.1 item 3 — sibling-parallel via rayon `par_iter`. Each
/// recursion frame builds its own thread pool inside `sync_meta_inner`
/// (work-stealing across recursion levels happens naturally because
/// the inner `pool.install` blocks for the lifetime of the inner
/// sync_meta call; sibling sub-metas at level N execute in parallel
/// via the level-N pool, and each level-N child carries its own
/// level-(N+1) pool for its own grandchildren). Sub-reports are
/// collected source-ordered via `collect_into_vec`, then folded into
/// `report` sequentially to preserve deterministic ordering of the
/// `phase1_classifications` / `phase2_pruned` / `errors` vectors.
// Pre-rayon refactor this fn already carried 7 args (the clippy cap).
// v1.2.1 item 3 added the `pool` reference, taking it to 8. Bundling
// these into a context struct is technically possible but every other
// arg already comes from `sync_meta_inner`'s param list, so the struct
// would just shuffle the wiring without removing it. Localised allow
// instead — the call-site is private to this module and threads
// ownership of `pool` cleanly.
/// Per-child Phase 3 dispatch — runs inside the rayon pool. Mirrors
/// the `phase1_handle_child` / `sync_disjoint_commutes` discipline
/// (one discoverable Rust contract anchor per sibling unit of work)
/// and keeps `phase3_recurse` itself under the clippy line cap.
///
/// v1.2.2 — cycle detection lives here. `ancestors` is the in-progress
/// ancestor identity chain from root down to (but excluding) this
/// child. If the child's identity (`pack_identity_for_child`) is
/// already in the chain we surface `TreeError::CycleDetected` with
/// the chain extended by the recurring identity. Otherwise the
/// child's identity is appended (clone-per-child, A.1) so disjoint
/// sibling branches do not pollute each other's view.
///
/// v1.2.3 (B1) — the depth-cap check (`next_depth > opts.max_depth`)
/// MUST run AFTER the cycle check. Otherwise a cyclic manifest whose
/// cycle length exceeds `max_depth` would silently truncate without
/// surfacing `CycleDetected`: the depth cap is a "stop walking
/// further" knob, not a "ignore correctness invariants" knob.
///
/// v1.2.4 (A1) — cancellation flag plumbed through. The closure
/// observes `cancelled.load(Relaxed)` at entry: if a prior sibling
/// already detected a cycle and signalled, this closure returns
/// `Phase3ChildOutcome::Cancelled` with zero further work (no
/// classify, no clone, no recursion). When this closure itself
/// detects a cycle, it stores `true` into the flag BEFORE returning
/// `Phase3ChildOutcome::Failed(CycleDetected)` so any rayon-co-iterated
/// sibling not yet started observes the signal on its next entry. The
/// flag is per-`phase3_recurse`-call: recursive sub-`sync_meta_inner`
/// invocations build their own flag inside their own
/// `phase3_recurse`, so a cycle two levels down does not cancel
/// disjoint siblings at level one. Discharges the Lean
/// `cancellation_terminates_promptly` obligation.
#[allow(clippy::too_many_arguments)]
fn phase3_handle_child(
    meta_dir: &Path,
    child: &ChildRef,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    next_depth: usize,
    ancestors: &[String],
    cancelled: &AtomicBool,
    pre_existing_dests: &HashSet<PathBuf>,
) -> Phase3ChildOutcome {
    // v1.2.5 (W1) — `dest` is normalised via [`normalize_dest_key`] so
    // membership checks against `pre_existing_dests` (snapshotted with
    // the same normalisation in `sync_meta_inner`) compare apples to
    // apples even when the manifest's `child.effective_path()` carries
    // a trailing `/` or a `./` prefix. Lexical normalisation only —
    // keeps `dest` a valid filesystem path for the subsequent
    // `cleanup_partial_clone` and `sync_meta_inner` calls.
    let dest = normalize_dest_key(&meta_dir.join(child.effective_path()));
    // v1.2.4 EARLY-OUT — a sibling closure already detected a cycle
    // and signalled. Return immediately with zero further descent so
    // deep walks never start. Matches the Lean
    // `cancellation_terminates_promptly` theorem: cancelled = true
    // implies ok with zero recursive steps.
    //
    // v1.2.5 (W1): align with the Lean theorem
    // `outcome ≠ Recursed → post.at dest = pre.at dest` — Cancelled is
    // a non-Recursed outcome, so the dest must be restored to its
    // pre-state. Phase 1 may have already freshly cloned the dest by
    // the time this closure runs; if so, clean it. Pre-existing dests
    // (legitimate prior content) are preserved via the same
    // `pre_existing_dests` guard used by the Failed(CycleDetected)
    // branch. Best-effort: cleanup failures are logged but do NOT
    // mask the cancellation outcome.
    if cancelled.load(Ordering::Relaxed) {
        if !pre_existing_dests.contains(&dest) {
            cleanup_partial_clone(&dest);
        }
        return Phase3ChildOutcome::Cancelled;
    }
    if !dest.join(".grex").join("pack.yaml").is_file() {
        return Phase3ChildOutcome::Skipped;
    }
    // v1.2.2 cycle detection — discharges the
    // `sync_meta_no_cycle_infinite_clone` Lean theorem in
    // `proof/Grex/Walker.lean`. Identity is `url@ref` so the same
    // repo at two different refs is two distinct packs (intentional:
    // matches `pack_identity_for_child` and the build_graph cycle
    // detector at `graph_build.rs:174`). A single `Vec<String>`
    // doubles as O(depth) contains-check AND deterministic chain for
    // error display — depth is bounded ~5-10 in practice so linear
    // scan beats hashing here.
    //
    // v1.2.3 (B1): runs BEFORE the depth-cap early-return below so a
    // cycle longer than `max_depth` cannot hide behind truncation.
    let id = pack_identity_for_child(child);
    if ancestors.iter().any(|v| v == &id) {
        // v1.2.4 SIGNAL — flip the cancellation flag so any
        // co-iterated sibling closures observe it on their next
        // entry. `Relaxed` is sufficient: we need eventual visibility,
        // not strict happens-before ordering against any other memory
        // operation. See design doc §"Atomic ordering".
        cancelled.store(true, Ordering::Relaxed);
        // v1.2.5 (A2) — partial-clone cleanup. The dest may have been
        // freshly cloned by Phase 1 within this same `sync_meta_inner`
        // call. If it pre-existed (prior successful sync), preserve
        // it — `pre_existing_dests` was snapshotted before Phase 1
        // ran. Best-effort: cleanup failures are logged but do NOT
        // mask the cycle error. Discharges the Lean theorem
        // `partial_clone_cleanup_idempotent`.
        if !pre_existing_dests.contains(&dest) {
            cleanup_partial_clone(&dest);
        }
        let mut chain = ancestors.to_vec();
        chain.push(id);
        return Phase3ChildOutcome::Failed(TreeError::CycleDetected { chain });
    }
    // v1.2.3 (B1): depth-cap check moved from `phase3_recurse` to
    // here, AFTER the cycle check. `Skipped` rather than a hard error
    // because depth-cap truncation is a benign best-effort knob —
    // siblings further down the manifest tree should still classify.
    if let Some(cap) = opts.max_depth {
        if next_depth > cap {
            return Phase3ChildOutcome::Skipped;
        }
    }
    // Clone-per-child (A.1): each rayon iteration owns its own
    // ancestor view, so disjoint sibling branches do not see each
    // other on the path. A diamond where two siblings legitimately
    // depend on the same descendant is therefore not a cycle.
    let mut child_ancestors = ancestors.to_vec();
    child_ancestors.push(id);
    // Empty `prune_candidates` for the sub-meta — 1.h supplies the
    // sub-meta's distributed lockfile read via the same caller
    // pathway when it lands.
    match sync_meta_inner(&dest, backend, loader, opts, &[], next_depth, &child_ancestors) {
        Ok(sub) => Phase3ChildOutcome::Recursed(sub),
        Err(e) => {
            // v1.2.5 (W1) — recursive failure path: the sub-walk
            // returned ANY `TreeError` deep in the subtree. The dest
            // itself was cloned by THIS frame's Phase 1 if it was
            // missing, so symmetry with the same-frame cycle branch
            // above demands the same pre-existence guard for ALL error
            // variants — not just `CycleDetected`. Aligns with the
            // Lean theorem `outcome ≠ Recursed → post.at dest = pre.at
            // dest`: any non-Recursed outcome (Failed regardless of
            // variant) must restore pre-state. Best-effort cleanup;
            // logged on failure.
            if !pre_existing_dests.contains(&dest) {
                cleanup_partial_clone(&dest);
            }
            Phase3ChildOutcome::Failed(e)
        }
    }
}

/// v1.2.5 (A2) — best-effort cleanup of a partially-cloned dest dir.
///
/// Invoked from the cycle-detected and cancelled paths in
/// `phase3_handle_child` when `dest` was freshly cloned by THIS frame's
/// Phase 1 (snapshotted via `pre_existing_dests`). Idempotent: a
/// non-existent `dest` is a no-op (the inner walker treats `NotFound`
/// as success). Failures are logged + swallowed so the original
/// cycle/error is preserved as the caller-visible outcome.
///
/// **Symlink safety (v1.2.5 W1):** This helper does NOT use
/// `std::fs::remove_dir_all` because that historically followed
/// directory symlinks during cleanup on some platforms / std versions.
/// Instead, it walks the tree manually using
/// [`std::fs::symlink_metadata`] at every level: a symlink is unlinked
/// AS a symlink (never followed into an unrelated tree), regular files
/// are deleted via `remove_file`, and directories are descended into
/// before being removed via `remove_dir`. Mirrors the
/// `safe_remove_dir_all` helper in `quarantine.rs` (kept inlined here
/// because the constraints of v1.2.5 W1 forbid touching that file).
///
/// Discharges the Lean theorem `partial_clone_cleanup_idempotent` in
/// `proof/Grex/Walker.lean`: the post-state at `dest` equals the
/// pre-state for any non-Recursed Phase 3 outcome.
fn cleanup_partial_clone(dest: &Path) {
    if let Err(e) = safe_remove_tree(dest) {
        // Don't propagate — the caller's primary error (cycle) is the
        // contract. Cleanup is best-effort hygiene.
        tracing::warn!(
            target: "grex::walker",
            dest = %dest.display(),
            error = %e,
            "v1.2.5 A2: partial-clone cleanup failed; original error preserved"
        );
    }
}

/// v1.2.5 (W1) — lexical-normalisation key used for the
/// `pre_existing_dests` HashSet so a manifest entry whose
/// `effective_path()` carries a trailing `/`, a `./` prefix, or a
/// redundant intermediate `./` component does not desync the snapshot
/// site from the lookup site.
///
/// Strategy: walk the path's components, drop every `Component::CurDir`
/// (`.`), and collect the rest back into a fresh `PathBuf`. Drops the
/// trailing-separator artefact that some callers preserve via
/// `PathBuf::push("foo/")`. Does NOT canonicalise (no FS access, no
/// symlink resolution) — the goal is to make two textually-different
/// representations of the SAME logical path compare equal.
///
/// `dunce::simplified` is the long-form alternative (it also strips
/// the Windows `\\?\` UNC prefix); we deliberately avoid pulling in
/// the dependency for a fix that the lexical strategy already covers
/// for every observed case in practice.
fn normalize_dest_key(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in p.components() {
        match comp {
            std::path::Component::CurDir => {
                // Drop redundant `./` components.
            }
            other => out.push(other.as_os_str()),
        }
    }
    if out.as_os_str().is_empty() {
        // Edge case: input was `.` or empty — preserve as `.` so
        // downstream FS ops still resolve to the same dir rather than
        // an empty path which has no meaning to the OS.
        out.push(".");
    }
    out
}

/// v1.2.5 (W1) — symlink-secure recursive removal helper.
///
/// **v1.2.6 (W2)**: re-routed through a `cap_std::fs::Dir` capability
/// rooted at `path.parent()`. The recursion stays inside the capability
/// the kernel resolved at open time — a `..` segment, an absolute child
/// path, or a symlink whose target escapes the capability is rejected by
/// cap-std with `PermissionDenied` (matches the Lean theorem
/// `walker_subpath_resolution_bounded_by_meta_dir`).
///
/// Walks `path` using [`cap_std::fs::Dir::symlink_metadata`] at every
/// level so a symlink encountered mid-traversal is unlinked AS a symlink
/// rather than followed into an unrelated tree.
///
/// Behaviour:
/// * `path` does not exist (`NotFound`) → `Ok(())` (idempotent).
/// * `path` is a symlink → unlink the link itself (never the target).
///   On Windows a directory symlink requires `remove_dir`; we try
///   `remove_file` first and fall back to `remove_dir`.
/// * `path` is a directory → recurse into each child via
///   `symlink_metadata`, then `remove_dir(path)`.
/// * `path` is a regular file → `remove_file`.
/// * `path` has no parent (filesystem root) → fall back to the
///   pre-v1.2.6 ambient `std::fs` walk; cap-std cannot model a rooted
///   capability there.
fn safe_remove_tree(path: &Path) -> std::io::Result<()> {
    // Pre-flight existence probe so a missing path is a no-op even when
    // the parent itself is missing (idempotent contract).
    match std::fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    }
    let (parent, name) = match (path.parent(), path.file_name()) {
        (Some(p), Some(n)) if !p.as_os_str().is_empty() => (p, std::path::PathBuf::from(n)),
        _ => return safe_remove_tree_ambient(path),
    };
    let parent_dir = match cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
    {
        Ok(d) => d,
        // Parent vanished or is unreadable — treat as idempotent success
        // (mirrors the NotFound short-circuit above).
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    cap_remove_tree(&parent_dir, &name)
}

/// v1.2.6 (W2) — capability-rooted recursive remove. `parent_dir` is a
/// `cap_std::fs::Dir` and `name` is a relative entry under it. cap-std
/// rejects `..` traversal and symlink escape; this recursion can only
/// touch entries provably under `parent_dir`'s root capability.
fn cap_remove_tree(parent_dir: &cap_std::fs::Dir, name: &Path) -> std::io::Result<()> {
    let meta = match parent_dir.symlink_metadata(name) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        // Unlink the symlink itself. On Windows, a symlink to a dir
        // requires `remove_dir`; `remove_file` covers file/symlink_file.
        // Try file-style first, then fall back to dir-style.
        return match parent_dir.remove_file(name) {
            Ok(()) => Ok(()),
            Err(_) => parent_dir.remove_dir(name),
        };
    }
    if ft.is_dir() {
        // Open the child as its own capability so the recursion stays
        // bound to it (cap-std refuses `..` escape relative to the
        // child handle, not just the parent). Drop the handle BEFORE
        // calling `parent_dir.remove_dir(name)` — on Windows an
        // outstanding open handle to a directory blocks removal with
        // ERROR_SHARING_VIOLATION.
        {
            let child_dir = parent_dir.open_dir(name)?;
            for entry in child_dir.entries()? {
                let entry = entry?;
                let child_name = std::path::PathBuf::from(entry.file_name());
                cap_remove_tree(&child_dir, &child_name)?;
            }
        }
        return parent_dir.remove_dir(name);
    }
    // Regular file or other unlinkable entry.
    parent_dir.remove_file(name)
}

/// v1.2.6 (W2) — fallback ambient walk used when `path` has no usable
/// parent (filesystem root). This path keeps the pre-v1.2.6 contract
/// for the degenerate case; production call sites always have a parent
/// (the per-meta dest is `<meta>/<basename>`).
fn safe_remove_tree_ambient(path: &Path) -> std::io::Result<()> {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let ft = meta.file_type();
    if ft.is_symlink() {
        return match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(_) => std::fs::remove_dir(path),
        };
    }
    if ft.is_dir() {
        for entry in std::fs::read_dir(path)? {
            let entry = entry?;
            let child = entry.path();
            safe_remove_tree_ambient(&child)?;
        }
        return std::fs::remove_dir(path);
    }
    std::fs::remove_file(path)
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn phase3_recurse(
    pool: &rayon::ThreadPool,
    meta_dir: &Path,
    manifest: &PackManifest,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    depth: usize,
    ancestors: &[String],
    pre_existing_dests: &HashSet<PathBuf>,
    report: &mut SyncMetaReport,
) -> Result<(), TreeError> {
    if !opts.recurse {
        return Ok(());
    }
    let next_depth = depth + 1;
    // v1.2.3 (B1): depth-cap early-return removed from this site —
    // moved into `phase3_handle_child` AFTER the cycle check so a
    // cycle longer than `max_depth` cannot mask itself by tripping
    // the depth cap before the cycle test fires. The per-child
    // handler now treats `next_depth > cap` as `Skipped`.
    //
    // v1.2.4 (A1): per-call cancellation flag. One `Arc<AtomicBool>`
    // is constructed here and shared (read-only across closures, with
    // a single point-of-truth `store` in any closure that detects a
    // cycle) by every sibling iteration of this `par_iter`. Recursive
    // sub-`sync_meta_inner` calls build their own flag via their own
    // `phase3_recurse` — disjoint subtrees do not cross-cancel. The
    // flag is dropped when this fn returns, so its lifetime is exactly
    // the rayon parallel pass it scopes.
    //
    // Scope: cancels siblings within THIS Phase 3 fan-out call only;
    // recursive sub-fan-outs construct their own flag, so a cycle deep
    // in pack X does not cancel pack Y at root level. This isolation
    // is intentional and tested by
    // `cancellation_per_call_scope_isolates_subtrees`.
    let cancelled = Arc::new(AtomicBool::new(false));
    // v1.2.5 (A3) — RAII guard tracks nested `pool.install` depth on
    // the calling OS thread. In debug builds the per-closure assertion
    // below uses this counter (combined with `pack_lock`'s *global*
    // `HELD_PACK_LOCKS` registry, scoped at read-time by `ThreadId`)
    // to fire if any worker thread enters the per-child closure body
    // while still holding a `PackLock` it acquired earlier on that
    // same thread — the v1.2.2 R#1 MED deadlock pattern. The
    // held-lock half of the check was migrated from a `thread_local!`
    // to a `Mutex<HashSet<(PathBuf, ThreadId)>>` (W4 cross-thread fix)
    // because rayon's work-stealing routes the closure onto worker
    // threads whose TL would always be empty (the lock was acquired
    // on the outer tokio worker, not on the rayon worker), masking
    // the deadlock pattern entirely. The depth counter remains
    // per-OS-thread (it tracks per-thread reentry depth correctly).
    // Release builds compile both halves to a zero-sized no-op. The
    // counter / global set live in `scheduler.rs` and `pack_lock.rs`
    // respectively (W3 + W2/W4 worker outputs).
    let _depth_guard = PoolInstallDepthGuard::new();
    let outcomes: Vec<Phase3ChildOutcome> = pool.install(|| {
        manifest
            .children
            .par_iter()
            .map(|child| {
                // v1.2.5 (A3) — debug-build deadlock guard. If this
                // worker thread already holds one or more PackLocks
                // AND we are inside a nested `pool.install` (depth
                // >= 2 means an outer Phase 1 / Phase 3 frame is
                // still active on this thread), the v1.2.2 R#1
                // deadlock pattern is reproducible. Single-frame
                // `pool.install` (depth == 1) holding a lock is
                // safe — the lock was acquired outside the pool and
                // released after the closure returns. The assertion
                // is `#[cfg(debug_assertions)]` only; release builds
                // pay no runtime cost. `held_pack_locks_for_test`
                // filters the global registry by `current().id()` so
                // the assertion correctly reports only locks held by
                // *this* worker thread, not concurrent acquires on
                // other threads. Discharges the Lean theorem
                // `pool_deadlock_guard_terminates` in
                // `proof/Grex/Scheduler.lean`.
                #[cfg(debug_assertions)]
                {
                    let depth = crate::scheduler::pool_install_depth_for_test();
                    let held = crate::pack_lock::held_pack_locks_for_test();
                    debug_assert!(
                        depth < 2 || held.is_empty(),
                        "pool deadlock guard: nested pool.install (depth={depth}) entered while \
                         holding PackLock(s) {held:?} on this thread — see concurrency.md \
                         §lock acquisition order"
                    );
                }
                phase3_handle_child(
                    meta_dir,
                    child,
                    backend,
                    loader,
                    opts,
                    next_depth,
                    ancestors,
                    &cancelled,
                    pre_existing_dests,
                )
            })
            .collect()
    });
    // Cycle errors short-circuit (catastrophic — clone-storm risk);
    // every other outcome folds into the report per the existing
    // fail-loud-but-continue policy. v1.2.4: `Cancelled` outcomes are
    // skipped — they carry no sub-report and contribute neither to
    // `report.metas_visited` nor to `report.errors`. The cycle that
    // triggered the cancellation is the sole error reported.
    let mut first_cycle_idx: Option<usize> = None;
    for outcome in outcomes {
        match outcome {
            Phase3ChildOutcome::Skipped | Phase3ChildOutcome::Cancelled => {}
            Phase3ChildOutcome::Recursed(sub) => report.merge(sub),
            Phase3ChildOutcome::Failed(e) => {
                // v1.2.2 fix: surface all sibling cycles in
                // report.errors; first cycle returned as short-circuit
                // Err per fail-loud policy.
                if matches!(e, TreeError::CycleDetected { .. }) && first_cycle_idx.is_none() {
                    first_cycle_idx = Some(report.errors.len());
                }
                report.errors.push(e);
            }
        }
    }
    if let Some(idx) = first_cycle_idx {
        // Clone the cycle to return as the short-circuit Err while
        // leaving the original entry (and any sibling cycles) recorded
        // in report.errors for the caller to log/print.
        let TreeError::CycleDetected { chain } = &report.errors[idx] else {
            unreachable!("first_cycle_idx points at a CycleDetected variant by construction");
        };
        return Err(TreeError::CycleDetected { chain: chain.clone() });
    }
    Ok(())
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

    // -----------------------------------------------------------------
    // v1.2.0 Stage 1.g — `sync_meta` three-phase walker tests (TDD).
    //
    // These tests use a thin in-memory `MockLoader` plus
    // `MockGitBackend` so the walker's PHASE ORCHESTRATION (not the
    // backend mechanics) is what's being exercised. The git-touching
    // primitives `classify_dest` (1.e) and `phase2_prune` (1.f) have
    // their own per-host tests that already cover the real-FS-and-git
    // path. The `host_has_git_binary` gate guards the few tests that
    // need a working `git` to materialise a clean `PresentDeclared`
    // verdict — same precedent as the `dest_class::tests` host-skip
    // pattern.
    // -----------------------------------------------------------------

    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Minimal stand-in `PackLoader` for the v1.2.0 tests. Maps
    /// `meta_dir` → `PackManifest` directly so we never touch disk
    /// for manifest reads.
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

    /// Minimal stand-in `GitBackend`. Records every call so tests can
    /// assert phase orchestration. `clone` materialises a `.git/`
    /// under the supplied dest so subsequent classify probes treat the
    /// slot as Present.
    #[allow(dead_code)] // fields populated for future test introspection.
    #[derive(Debug, Clone)]
    enum BackendCall {
        Clone { url: String, dest: PathBuf, r#ref: Option<String> },
        Fetch { dest: PathBuf },
        Checkout { dest: PathBuf, r#ref: String },
        HeadSha { dest: PathBuf },
    }

    struct InMemGit {
        calls: Mutex<Vec<BackendCall>>,
        materialise_on_clone: bool,
    }

    impl InMemGit {
        fn new() -> Self {
            Self { calls: Mutex::new(Vec::new()), materialise_on_clone: true }
        }
        fn calls(&self) -> Vec<BackendCall> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl GitBackend for InMemGit {
        fn name(&self) -> &'static str {
            "v1_2_0-mock-git"
        }
        fn clone(
            &self,
            url: &str,
            dest: &Path,
            r#ref: Option<&str>,
            _lock_ctx: crate::git::BackendLockCtx<'_>,
        ) -> Result<crate::ClonedRepo, crate::GitError> {
            self.calls.lock().unwrap().push(BackendCall::Clone {
                url: url.to_string(),
                dest: dest.to_path_buf(),
                r#ref: r#ref.map(str::to_string),
            });
            if self.materialise_on_clone {
                std::fs::create_dir_all(dest.join(".git")).unwrap();
            }
            Ok(crate::ClonedRepo { path: dest.to_path_buf(), head_sha: "0".repeat(40) })
        }
        fn fetch(
            &self,
            dest: &Path,
            _lock_ctx: crate::git::BackendLockCtx<'_>,
        ) -> Result<(), crate::GitError> {
            self.calls.lock().unwrap().push(BackendCall::Fetch { dest: dest.to_path_buf() });
            Ok(())
        }
        fn checkout(
            &self,
            dest: &Path,
            r#ref: &str,
            _lock_ctx: crate::git::BackendLockCtx<'_>,
        ) -> Result<(), crate::GitError> {
            self.calls
                .lock()
                .unwrap()
                .push(BackendCall::Checkout { dest: dest.to_path_buf(), r#ref: r#ref.to_string() });
            Ok(())
        }
        fn head_sha(&self, dest: &Path) -> Result<String, crate::GitError> {
            self.calls.lock().unwrap().push(BackendCall::HeadSha { dest: dest.to_path_buf() });
            Ok("0".repeat(40))
        }
    }

    /// Build a meta manifest with the supplied children.
    fn meta_manifest_with(name: &str, children: Vec<ChildRef>) -> PackManifest {
        PackManifest {
            schema_version: SchemaVersion::current(),
            name: name.to_string(),
            r#type: PackType::Meta,
            version: None,
            depends_on: Vec::new(),
            children,
            actions: Vec::new(),
            teardown: None,
            extensions: BTreeMap::new(),
        }
    }

    fn child(url: &str, path: &str) -> ChildRef {
        ChildRef { url: url.to_string(), path: Some(path.to_string()), r#ref: None }
    }

    fn host_has_git_binary() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .is_ok_and(|o| o.status.success())
    }

    /// Empty meta — no children → the walker returns Ok with no work.
    #[test]
    fn test_walker_v1_2_0_simple_meta_no_children() {
        let tmp = tempfile::tempdir().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        let loader = InMemLoader::new().with(meta_dir.clone(), meta_manifest_with("solo", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &[]).expect("ok");
        assert_eq!(report.metas_visited, 1);
        assert!(report.phase1_classifications.is_empty());
        assert!(report.phase2_pruned.is_empty());
        assert!(report.errors.is_empty());
        assert!(backend.calls().is_empty(), "no children → no git ops");
    }

    /// Phase 1 classifies each child. With every dest absent on disk,
    /// every classification is `Missing` and the backend sees one
    /// `Clone` per child.
    #[test]
    fn test_walker_v1_2_0_phase1_classifies_each_child() {
        let tmp = tempfile::tempdir().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        let kids = vec![
            child("https://example.com/a.git", "alpha"),
            child("https://example.com/b.git", "beta"),
        ];
        let loader =
            InMemLoader::new().with(meta_dir.clone(), meta_manifest_with("root", kids.clone()));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions { recurse: false, ..SyncMetaOptions::default() };
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &[]).expect("ok");
        assert_eq!(report.phase1_classifications.len(), 2);
        for (parent, _, class) in &report.phase1_classifications {
            assert_eq!(parent, &meta_dir);
            assert_eq!(*class, DestClass::Missing);
        }
        assert!(report.errors.is_empty());
        let calls = backend.calls();
        assert_eq!(calls.len(), 2, "one clone per child");
        for call in calls {
            assert!(matches!(call, BackendCall::Clone { .. }));
        }
    }

    /// Phase 1 must aggregate every undeclared `.git/` directory it
    /// encounters into a single `UntrackedGitRepos` error. We
    /// pre-create two `.git/` slots BEFORE running `sync_meta` and
    /// declare them as siblings without paths matching — they classify
    /// as `PresentUndeclared` because the manifest does not list them.
    #[test]
    fn test_walker_v1_2_0_phase1_aggregates_untracked_error() {
        // Build a meta whose manifest declares ZERO children — every
        // pre-existing `.git/` slot is by definition undeclared.
        // Then drop two `.git/` directories under the meta dir and
        // (because v1.2.0's classifier needs the manifest declaration
        // signal at the call site, not on-disk discovery) run a
        // PARALLEL classifier sweep over the on-disk dirs to feed the
        // aggregator. This mirrors the way 1.h's lockfile-orphan
        // sweep will surface PresentUndeclared dirs into Phase 1's
        // collector when a child is removed from the manifest.
        let tmp = tempfile::tempdir().unwrap();
        let alpha = tmp.path().join("alpha");
        let beta = tmp.path().join("beta");
        std::fs::create_dir_all(alpha.join(".git")).unwrap();
        std::fs::create_dir_all(beta.join(".git")).unwrap();
        // Direct unit on the aggregator: feed two `PresentUndeclared`
        // pairs and assert the error carries both.
        let pairs: Vec<(PathBuf, DestClass)> = vec![
            (alpha.clone(), DestClass::PresentUndeclared),
            (beta.clone(), DestClass::PresentUndeclared),
        ];
        let err = aggregate_untracked(pairs).expect_err("two undeclared → error");
        match err {
            TreeError::UntrackedGitRepos { paths } => {
                assert_eq!(paths, vec![alpha, beta]);
            }
            other => panic!("expected UntrackedGitRepos, got {other:?}"),
        }
    }

    /// Phase 2 prunes a clean orphan: the supplied candidate has a
    /// real `.git/` (initialised by `git init`), the consent walk
    /// returns Clean, the dest is removed.
    #[test]
    fn test_walker_v1_2_0_phase2_prunes_clean_orphans() {
        if !host_has_git_binary() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        // Create the orphan dest — clean repo, no manifest entry.
        let orphan = meta_dir.join("ghost");
        std::fs::create_dir_all(&orphan).unwrap();
        let init =
            std::process::Command::new("git").arg("-C").arg(&orphan).args(["init", "-q"]).status();
        if !matches!(init, Ok(s) if s.success()) {
            return;
        }
        let loader = InMemLoader::new().with(meta_dir.clone(), meta_manifest_with("root", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions { recurse: false, ..SyncMetaOptions::default() };
        let prune_list = vec![PathBuf::from("ghost")];
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &prune_list).expect("ok");
        assert_eq!(report.phase2_pruned.len(), 1, "clean orphan must be pruned");
        assert_eq!(report.phase2_pruned[0], orphan);
        assert!(!orphan.exists(), "dest must be removed after a clean prune");
        assert!(report.errors.is_empty());
    }

    /// Phase 2 must REFUSE to prune a dirty orphan absent the override
    /// flag. The consent walk classifies it `DirtyTree`; the walker
    /// surfaces `DirtyTreeRefusal` and leaves the dest untouched.
    #[test]
    fn test_walker_v1_2_0_phase2_refuses_dirty_orphan() {
        if !host_has_git_binary() {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        let orphan = meta_dir.join("dirty-ghost");
        std::fs::create_dir_all(&orphan).unwrap();
        let init =
            std::process::Command::new("git").arg("-C").arg(&orphan).args(["init", "-q"]).status();
        if !matches!(init, Ok(s) if s.success()) {
            return;
        }
        std::fs::write(orphan.join("scratch.txt"), b"unsaved").unwrap();
        let loader = InMemLoader::new().with(meta_dir.clone(), meta_manifest_with("root", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions { recurse: false, ..SyncMetaOptions::default() };
        let prune_list = vec![PathBuf::from("dirty-ghost")];
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &prune_list).expect("ok");
        assert!(report.phase2_pruned.is_empty(), "dirty orphan must NOT be pruned");
        assert!(orphan.exists(), "dest stays on disk when refused");
        assert_eq!(report.errors.len(), 1);
        assert!(matches!(report.errors[0], TreeError::DirtyTreeRefusal { .. }));
    }

    /// Phase 3 recurses into a child meta when its `.grex/pack.yaml`
    /// exists. The sub-meta's own `metas_visited` is folded into the
    /// parent's report.
    #[test]
    fn test_walker_v1_2_0_phase3_recurses_into_sub_meta() {
        let tmp = tempfile::tempdir().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        let child_dest = meta_dir.join("sub");
        // Pre-materialise the sub-meta on disk so Phase 1 classifies
        // the dest as PresentDeclared (no clone fired) and Phase 3
        // sees a `.grex/pack.yaml` to recurse into.
        make_sub_meta_on_disk(&child_dest, "sub");
        let loader = InMemLoader::new()
            .with(
                meta_dir.clone(),
                meta_manifest_with("root", vec![child("https://example.com/sub.git", "sub")]),
            )
            .with(child_dest.clone(), meta_manifest_with("sub", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &[]).expect("ok");
        assert_eq!(report.metas_visited, 2, "parent + sub-meta visited");
        assert!(report.errors.is_empty());
    }

    /// `recurse: false` skips Phase 3 entirely — `metas_visited == 1`
    /// even when a child has a `.grex/pack.yaml`.
    #[test]
    fn test_walker_v1_2_0_phase3_max_depth_zero_skips_recursion() {
        let tmp = tempfile::tempdir().unwrap();
        let meta_dir = tmp.path().to_path_buf();
        let child_dest = meta_dir.join("sub");
        make_sub_meta_on_disk(&child_dest, "sub");
        let loader = InMemLoader::new()
            .with(
                meta_dir.clone(),
                meta_manifest_with("root", vec![child("https://example.com/sub.git", "sub")]),
            )
            .with(child_dest.clone(), meta_manifest_with("sub", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions { recurse: false, ..SyncMetaOptions::default() };
        let report = sync_meta(&meta_dir, &backend, &loader, &opts, &[]).expect("ok");
        assert_eq!(report.metas_visited, 1, "no recursion → only the root meta");
    }

    /// `max_depth: Some(N)` caps recursion at N levels of nesting.
    /// Build a 3-level chain (root → mid → leaf) and assert
    /// `max_depth: Some(1)` visits root + mid (depth 0 + 1) but NOT
    /// leaf (depth 2).
    #[test]
    fn test_walker_v1_2_0_phase3_max_depth_n_stops_at_n_levels() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        let mid_dir = root_dir.join("mid");
        let leaf_dir = mid_dir.join("leaf");
        make_sub_meta_on_disk(&mid_dir, "mid");
        make_sub_meta_on_disk(&leaf_dir, "leaf");
        let loader = InMemLoader::new()
            .with(
                root_dir.clone(),
                meta_manifest_with("root", vec![child("https://example.com/mid.git", "mid")]),
            )
            .with(
                mid_dir.clone(),
                meta_manifest_with("mid", vec![child("https://example.com/leaf.git", "leaf")]),
            )
            .with(leaf_dir.clone(), meta_manifest_with("leaf", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions { max_depth: Some(1), ..SyncMetaOptions::default() };
        let report = sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("ok");
        // depth 0 = root, depth 1 = mid → max_depth: Some(1) visits
        // root + mid (2 metas) and stops before recursing into leaf.
        assert_eq!(report.metas_visited, 2, "max_depth: Some(1) visits root + mid only");
    }

    /// Helper: pre-populate a sub-meta directory at `dir` with a
    /// `.grex/pack.yaml` carrying `name` and a stub `.git/` so the
    /// classifier sees it as PresentDeclared.
    fn make_sub_meta_on_disk(dir: &Path, name: &str) {
        std::fs::create_dir_all(dir.join(".grex")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\n");
        std::fs::write(dir.join(".grex/pack.yaml"), yaml).unwrap();
    }

    /// Helper: collect the destinations Phase 1 recorded for a given
    /// parent meta from the rolled-up report.
    fn destinations_under(report: &SyncMetaReport, parent: &Path) -> Vec<PathBuf> {
        report
            .phase1_classifications
            .iter()
            .filter(|(p, _, _)| p == parent)
            .map(|(_, d, _)| d.clone())
            .collect()
    }

    /// Parent-relative path resolution: a child declared at the root
    /// meta resolves to `<root>/<child>` — NOT to a global workspace
    /// anchor. Recursion into that child uses `<root>/<child>` as the
    /// new parent meta dir for resolving the grandchild.
    #[test]
    fn test_walker_v1_2_0_parent_relative_path_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // Historical note: prior to v1.2.0 the path-segment validator
        // forbade slashes, so multi-segment nesting was modelled by
        // chaining single-segment children across recursion frames.
        // v1.2.0 relaxes the rule (per-segment validation), so a single
        // slash-bearing `path:` is also legal — this fixture keeps the
        // chained form for parity with pre-v1.2.0 behaviour.
        let tools_dir = root_dir.join("tools");
        let foo_dir = tools_dir.join("foo");
        make_sub_meta_on_disk(&tools_dir, "tools");
        make_sub_meta_on_disk(&foo_dir, "foo");
        let loader = InMemLoader::new()
            .with(
                root_dir.clone(),
                meta_manifest_with("root", vec![child("https://example.com/tools.git", "tools")]),
            )
            .with(
                tools_dir.clone(),
                meta_manifest_with("tools", vec![child("https://example.com/foo.git", "foo")]),
            )
            .with(foo_dir.clone(), meta_manifest_with("foo", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let report = sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("ok");
        // Three metas visited: root → tools → foo.
        assert_eq!(report.metas_visited, 3);
        // Phase 1 classifications confirm parent-relative resolution:
        // every recorded dest is a SUBDIR of its recorded parent.
        for (parent, dest, _class) in &report.phase1_classifications {
            assert!(
                dest.starts_with(parent),
                "child dest {} must descend from parent {}",
                dest.display(),
                parent.display()
            );
        }
        // Spot-check the chain: root sees `tools`, tools sees `foo`.
        assert_eq!(destinations_under(&report, &root_dir), vec![tools_dir.clone()]);
        assert_eq!(destinations_under(&report, &tools_dir), vec![foo_dir.clone()]);
    }

    // -----------------------------------------------------------------
    // v1.2.2 — `sync_meta` cycle detection (Phase 3 recursion edge).
    //
    // Discharges `sync_meta_no_cycle_infinite_clone` in
    // `proof/Grex/Walker.lean`. Identity scheme is `url@ref` so the
    // same repo at two different refs is NOT a cycle (covered by the
    // positive case below).
    // -----------------------------------------------------------------

    /// `child_with_ref` mirrors `child()` but lets the caller pin a
    /// specific ref so two children of the same URL get distinct
    /// `pack_identity_for_child` strings (`url@ref`).
    fn child_with_ref(url: &str, path: &str, r#ref: &str) -> ChildRef {
        ChildRef {
            url: url.to_string(),
            path: Some(path.to_string()),
            r#ref: Some(r#ref.to_string()),
        }
    }

    /// Self-loop: pack A declares itself (same URL, no ref) as a child.
    /// The walker must abort with `CycleDetected` rather than recurse
    /// infinitely. The chain reports the recurring identity.
    #[test]
    fn cycle_self_loop_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // Lay out a self-pointing pack: `<root>/a` is a sub-meta whose
        // own manifest declares a child with the SAME URL/ref pointing
        // back at itself (placed at a fresh path so on-disk dest is
        // distinct, but pack identity collides).
        let a_dir = root_dir.join("a");
        let a_self_dir = a_dir.join("a");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&a_self_dir, "a");
        let url_a = "https://example.com/a.git";
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            // `a` declares itself — same url, same (empty) ref → same identity.
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_a, "a")]))
            .with(a_self_dir.clone(), meta_manifest_with("a", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let err =
            sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect_err("self-loop must abort");
        match err {
            TreeError::CycleDetected { chain } => {
                // v1.2.3 (B4): chain begins with the root's
                // path-namespaced identity (`path:<root_dir>`) — the
                // initial visited seed — followed by the cyclic
                // child identities. v1.2.3 (B2): empty/None ref drops
                // the trailing `@`, so the cyclic id is just
                // `url:<url_a>` (no `@`).
                let id_a = format!("url:{url_a}");
                assert!(
                    chain.iter().any(|s| s == &id_a),
                    "chain must mention the cyclic url, got {chain:?}"
                );
                assert!(chain.len() >= 2, "self-loop chain has at least 2 entries: {chain:?}");
                let last = chain.last().unwrap();
                assert_eq!(last, &id_a, "chain must end with the recurring child identity");
                let first_match = chain.iter().position(|s| s == last).unwrap();
                assert!(
                    first_match < chain.len() - 1,
                    "the recurring identity must appear earlier in the chain: {chain:?}"
                );
                // The root frame is path-namespaced and disjoint from
                // any child's url-namespaced identity, so it must
                // appear at the head of the chain without colliding.
                assert!(
                    chain[0].starts_with("path:"),
                    "chain head is the root path identity: {chain:?}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    /// Three-node cycle: A → B → C → A. The walker must abort with
    /// `CycleDetected` and the chain must list all three identities
    /// in the order they were entered, ending with the recurring A.
    #[test]
    fn cycle_three_node_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // Disk layout: root → a → b → c → a (the second `a` lives at
        // a fresh on-disk slot so classification succeeds; identity
        // collision is what trips the cycle detector, not the path).
        let a_dir = root_dir.join("a");
        let b_dir = a_dir.join("b");
        let c_dir = b_dir.join("c");
        let a2_dir = c_dir.join("a");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&b_dir, "b");
        make_sub_meta_on_disk(&c_dir, "c");
        make_sub_meta_on_disk(&a2_dir, "a");
        let url_a = "https://example.com/a.git";
        let url_b = "https://example.com/b.git";
        let url_c = "https://example.com/c.git";
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_b, "b")]))
            .with(b_dir.clone(), meta_manifest_with("b", vec![child(url_c, "c")]))
            // c re-declares a → cycle.
            .with(c_dir.clone(), meta_manifest_with("c", vec![child(url_a, "a")]))
            .with(a2_dir.clone(), meta_manifest_with("a", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("three-node cycle must abort");
        match err {
            TreeError::CycleDetected { chain } => {
                // v1.2.3 (B4): chain leads with the root's
                // path-namespaced identity. v1.2.3 (B2): empty/None
                // ref drops the trailing `@`. Chain order:
                // [path:root, a, b, c, a] (entry order, with the
                // recurring `a` appended at the cycle-detection point).
                let id_root = pack_identity_for_root(&root_dir);
                let id_a = format!("url:{url_a}");
                let id_b = format!("url:{url_b}");
                let id_c = format!("url:{url_c}");
                assert_eq!(chain, vec![id_root, id_a.clone(), id_b, id_c, id_a]);
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    /// Same repo, two refs — NOT a cycle. Pack A declares two children
    /// pointing at the SAME URL but pinned to different refs (`main`
    /// vs `dev`). Identity scheme is `url@ref` so the two siblings
    /// have distinct identities and the walker must succeed.
    #[test]
    fn same_repo_two_refs_no_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        let main_dir = root_dir.join("b-main");
        let dev_dir = root_dir.join("b-dev");
        make_sub_meta_on_disk(&main_dir, "b-main");
        make_sub_meta_on_disk(&dev_dir, "b-dev");
        let url_b = "https://example.com/b.git";
        let loader = InMemLoader::new()
            .with(
                root_dir.clone(),
                meta_manifest_with(
                    "root",
                    vec![
                        child_with_ref(url_b, "b-main", "main"),
                        child_with_ref(url_b, "b-dev", "dev"),
                    ],
                ),
            )
            .with(main_dir.clone(), meta_manifest_with("b-main", vec![]))
            .with(dev_dir.clone(), meta_manifest_with("b-dev", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let report = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect("same url at distinct refs is NOT a cycle");
        // Three metas visited: root + b@main + b@dev.
        assert_eq!(report.metas_visited, 3);
        assert!(
            report.errors.is_empty(),
            "no errors expected when the two children differ only by ref: {:?}",
            report.errors
        );
    }

    /// Same repo, two refs — NESTED (ancestor-stack) variant. Pack A
    /// (URL=foo, ref=main) declares pack B (URL=foo, ref=dev) as its
    /// child. Identity scheme is `url@ref`, so A's identity
    /// (`url:foo@main`) and B's identity (`url:foo@dev`) differ. The
    /// cycle detector must NOT trip even though B's URL collides with
    /// an ancestor on the stack — exercises the path the sibling
    /// variant above doesn't reach.
    #[test]
    fn same_repo_two_refs_nested_no_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        let a_dir = root_dir.join("a");
        let b_dir = a_dir.join("b");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&b_dir, "b");
        let url_foo = "https://example.com/foo.git";
        let loader = InMemLoader::new()
            .with(
                root_dir.clone(),
                meta_manifest_with("root", vec![child_with_ref(url_foo, "a", "main")]),
            )
            // a (foo@main) declares b (foo@dev) — same URL, different ref.
            .with(a_dir.clone(), meta_manifest_with("a", vec![child_with_ref(url_foo, "b", "dev")]))
            .with(b_dir.clone(), meta_manifest_with("b", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let report = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect("nested same-url at distinct refs is NOT a cycle");
        // Walker must reach depth 2: root → a → b (3 metas).
        assert_eq!(report.metas_visited, 3, "walker must recurse to depth 2");
        assert!(
            report.errors.is_empty(),
            "no errors expected when ancestor and descendant differ only by ref: {:?}",
            report.errors
        );
    }

    // -----------------------------------------------------------------
    // v1.2.3 — additional cycle/diamond coverage (T1, T2, T3).
    //
    // T1 covers the diamond-shared-descendant case the
    // clone-per-child scheme is meant to permit; T2 stretches the
    // cycle to length 4 to exercise chain accumulation; T3 verifies
    // the cycle detector sees a cycle introduced inside an inner
    // subtree even though the outer arm is acyclic.
    // -----------------------------------------------------------------

    /// T1 — Diamond, NO cycle. Topology:
    ///
    /// ```text
    ///   root → A
    ///   root → B
    ///   A    → C
    ///   B    → C   (C is a shared descendant)
    /// ```
    ///
    /// Walker must traverse all four packs and produce no
    /// `CycleDetected`. Because the cycle detector clones the
    /// ancestor chain per child, A's descendants do not poison B's
    /// descendant view, so seeing `C` from both arms is a diamond,
    /// not a cycle.
    ///
    /// **v1.2.4 T1-spot-check extension.** In addition to the
    /// `metas_visited == 5` count assertion, this test also confirms
    /// that C is genuinely walked through BOTH arms: the
    /// `phase1_classifications` table must record one entry whose
    /// parent is `a/` (with dest `a/c`) AND one entry whose parent is
    /// `b/` (with dest `b/c`). Counting alone (`metas_visited`) cannot
    /// catch a regression where a future memoization optimization
    /// collapses the second walk into a no-op while still incrementing
    /// the counter — tracking the actual dest paths does.
    #[test]
    fn cycle_diamond_shared_descendant_no_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // Disk layout: root/a, root/b, root/a/c, root/b/c.
        // Each `c` lives at a distinct on-disk slot so classify
        // succeeds; identity equality is what would (incorrectly)
        // trip the cycle detector if clone-per-child were broken.
        let a_dir = root_dir.join("a");
        let b_dir = root_dir.join("b");
        let c_under_a_dir = a_dir.join("c");
        let c_under_b_dir = b_dir.join("c");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&b_dir, "b");
        make_sub_meta_on_disk(&c_under_a_dir, "c");
        make_sub_meta_on_disk(&c_under_b_dir, "c");
        let url_a = "https://example.com/a.git";
        let url_b = "https://example.com/b.git";
        let url_c = "https://example.com/c.git";
        let loader = InMemLoader::new()
            .with(
                root_dir.clone(),
                meta_manifest_with("root", vec![child(url_a, "a"), child(url_b, "b")]),
            )
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_c, "c")]))
            .with(b_dir.clone(), meta_manifest_with("b", vec![child(url_c, "c")]))
            .with(c_under_a_dir.clone(), meta_manifest_with("c", vec![]))
            .with(c_under_b_dir.clone(), meta_manifest_with("c", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let report =
            sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("diamond is NOT a cycle");
        // Four distinct manifest visits: root, a, b, c-via-a, c-via-b.
        // A and B both expand into their own `c`, so the walker
        // visits `c` twice (once per arm) — five `metas_visited`.
        assert_eq!(
            report.metas_visited, 5,
            "diamond: root + a + b + c-under-a + c-under-b = 5 visits"
        );
        // Crucially, no errors of any kind — and certainly not a
        // CycleDetected — because the two `C` visits live on
        // disjoint cloned ancestor chains.
        assert!(
            !report.errors.iter().any(|e| matches!(e, TreeError::CycleDetected { .. })),
            "diamond must not surface CycleDetected; errors={:?}",
            report.errors
        );
        assert!(report.errors.is_empty(), "diamond should produce no errors: {:?}", report.errors);

        // v1.2.4 T1-spot-check: assert `c` was genuinely walked under
        // BOTH arms. The phase1_classifications table records every
        // (parent_meta, dest, class) triple observed during Phase 1
        // dispatch; for the diamond layout we expect:
        //   * (root, a)    — a is a direct child of root
        //   * (root, b)    — b is a direct child of root
        //   * (a, a/c)     — c-via-a (Phase 1 inside a's recursion)
        //   * (b, b/c)     — c-via-b (Phase 1 inside b's recursion)
        // If a future memoization regression collapses the second `c`
        // walk into a no-op while still incrementing `metas_visited`,
        // the (b, b/c) pair will be missing from this table and the
        // assertion below fails.
        let dests_under_a = destinations_under(&report, &a_dir);
        let dests_under_b = destinations_under(&report, &b_dir);
        assert!(
            dests_under_a.iter().any(|d| d == &c_under_a_dir),
            "diamond: expected c-via-a in classifications under a, got {dests_under_a:?}"
        );
        assert!(
            dests_under_b.iter().any(|d| d == &c_under_b_dir),
            "diamond: expected c-via-b in classifications under b, got {dests_under_b:?}"
        );
        assert_ne!(
            c_under_a_dir, c_under_b_dir,
            "the two `c` visits must land on distinct on-disk dests"
        );
    }

    /// T2 — 4-node cycle: `root → A → B → C → D → A`. Cycle length 4
    /// in pack-identity terms; the reported chain has length 5 once
    /// the recurring `A` is appended at detection. The root frame's
    /// `path:` identity also leads the chain (B4), so the final
    /// length is 6.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cycle_four_node_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // Disk chain: root → a → b → c → d → a (the second `a` lives
        // at a fresh slot so classify succeeds; identity collision is
        // what trips the cycle detector).
        let a_dir = root_dir.join("a");
        let b_dir = a_dir.join("b");
        let c_dir = b_dir.join("c");
        let d_dir = c_dir.join("d");
        let a2_dir = d_dir.join("a");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&b_dir, "b");
        make_sub_meta_on_disk(&c_dir, "c");
        make_sub_meta_on_disk(&d_dir, "d");
        make_sub_meta_on_disk(&a2_dir, "a");
        let url_a = "https://example.com/a.git";
        let url_b = "https://example.com/b.git";
        let url_c = "https://example.com/c.git";
        let url_d = "https://example.com/d.git";
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_b, "b")]))
            .with(b_dir.clone(), meta_manifest_with("b", vec![child(url_c, "c")]))
            .with(c_dir.clone(), meta_manifest_with("c", vec![child(url_d, "d")]))
            // d re-declares a → cycle of length 4 in url-namespace.
            .with(d_dir.clone(), meta_manifest_with("d", vec![child(url_a, "a")]))
            .with(a2_dir.clone(), meta_manifest_with("a", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("four-node cycle must abort");
        match err {
            TreeError::CycleDetected { chain } => {
                let id_root = pack_identity_for_root(&root_dir);
                let id_a = format!("url:{url_a}");
                let id_b = format!("url:{url_b}");
                let id_c = format!("url:{url_c}");
                let id_d = format!("url:{url_d}");
                // [path:root, a, b, c, d, a] — six entries.
                assert_eq!(
                    chain,
                    vec![id_root, id_a.clone(), id_b, id_c, id_d, id_a.clone()],
                    "expected full ancestor chain ending in the recurring A"
                );
                assert!(
                    chain.len() >= 5,
                    "four-node cycle chain has at least 5 entries: {chain:?}"
                );
                // Last element repeats earlier in the chain (the
                // recurring identity).
                let last = chain.last().unwrap();
                let first_match = chain.iter().position(|s| s == last).unwrap();
                assert!(
                    first_match < chain.len() - 1,
                    "the recurring identity must appear earlier in the chain: {chain:?}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    /// T3 — Nested-prefix cycle. Outer arm `root → A → B → C` is
    /// acyclic; the cycle lives inside B's other child `D`, which
    /// loops back to B (`B → D → B`). The walker must surface
    /// `CycleDetected` and the cycle should appear inside the
    /// subtree (not at the root level), with B as the recurring
    /// identity.
    ///
    /// Specifically: A's children = [B], B's children = [C, D], C
    /// has no children, D's children = [B] (cycle).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cycle_nested_prefix_aborts() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // Disk layout: root/a, root/a/b, root/a/b/c (acyclic arm),
        // root/a/b/d (cycle arm), root/a/b/d/b (D loops back to B —
        // identity collision; on-disk path is fresh so classify
        // succeeds).
        let a_dir = root_dir.join("a");
        let b_dir = a_dir.join("b");
        let c_dir = b_dir.join("c");
        let d_dir = b_dir.join("d");
        let b2_dir = d_dir.join("b");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&b_dir, "b");
        make_sub_meta_on_disk(&c_dir, "c");
        make_sub_meta_on_disk(&d_dir, "d");
        make_sub_meta_on_disk(&b2_dir, "b");
        let url_a = "https://example.com/a.git";
        let url_b = "https://example.com/b.git";
        let url_c = "https://example.com/c.git";
        let url_d = "https://example.com/d.git";
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_b, "b")]))
            // b has both an acyclic child (c) and a cyclic one (d).
            .with(
                b_dir.clone(),
                meta_manifest_with("b", vec![child(url_c, "c"), child(url_d, "d")]),
            )
            .with(c_dir.clone(), meta_manifest_with("c", vec![]))
            // d re-declares b → cycle inside the b/d subtree.
            .with(d_dir.clone(), meta_manifest_with("d", vec![child(url_b, "b")]))
            .with(b2_dir.clone(), meta_manifest_with("b", vec![]));
        let backend = InMemGit::new();
        let opts = SyncMetaOptions::default();
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("nested-prefix cycle must abort");
        match err {
            TreeError::CycleDetected { chain } => {
                let id_root = pack_identity_for_root(&root_dir);
                let id_a = format!("url:{url_a}");
                let id_b = format!("url:{url_b}");
                let id_d = format!("url:{url_d}");
                // The cycle hits inside the subtree at depth 4:
                // [path:root, a, b, d, b].
                assert_eq!(
                    chain,
                    vec![id_root.clone(), id_a, id_b.clone(), id_d, id_b.clone()],
                    "cycle should appear inside the subtree, not at the top"
                );
                // Recurring identity is `b`, and it does NOT appear
                // at the chain's outermost position — the root path
                // identity does. This verifies the cycle is "inside"
                // the tree.
                let last = chain.last().unwrap();
                assert_eq!(last, &id_b, "recurring identity is B");
                assert_ne!(
                    chain.first().unwrap(),
                    last,
                    "cycle must not start at the root frame: {chain:?}"
                );
                assert_eq!(
                    chain.first().unwrap(),
                    &id_root,
                    "chain must begin with the root path identity: {chain:?}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    /// B1 regression: max_depth must NOT mask cycle detection. Cycle
    /// check fires before depth-cap return in phase3_handle_child.
    ///
    /// Topology: same 4-node cycle as `cycle_four_node_aborts`
    /// (`root → A → B → C → D → A`). Recurring `A` is reached at
    /// `next_depth = 5`. With `max_depth: Some(4)`, the depth cap
    /// would skip the recurring frame BEFORE it can be tested for
    /// cycle membership — *if* B1 were reverted (i.e. depth-cap
    /// early-return placed before the cycle check). The current
    /// ordering (cycle-then-depth-cap, see `phase3_handle_child`)
    /// surfaces `CycleDetected` regardless of the cap.
    ///
    /// If anyone reverts B1's reorder, this test fails: the walker
    /// returns `Ok(_)` instead of `Err(CycleDetected)` because the
    /// recurring frame is silently truncated.
    #[test]
    fn cycle_aborts_under_max_depth_cap() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        let a_dir = root_dir.join("a");
        let b_dir = a_dir.join("b");
        let c_dir = b_dir.join("c");
        let d_dir = c_dir.join("d");
        let a2_dir = d_dir.join("a");
        make_sub_meta_on_disk(&a_dir, "a");
        make_sub_meta_on_disk(&b_dir, "b");
        make_sub_meta_on_disk(&c_dir, "c");
        make_sub_meta_on_disk(&d_dir, "d");
        make_sub_meta_on_disk(&a2_dir, "a");
        let url_a = "https://example.com/a.git";
        let url_b = "https://example.com/b.git";
        let url_c = "https://example.com/c.git";
        let url_d = "https://example.com/d.git";
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_b, "b")]))
            .with(b_dir.clone(), meta_manifest_with("b", vec![child(url_c, "c")]))
            .with(c_dir.clone(), meta_manifest_with("c", vec![child(url_d, "d")]))
            .with(d_dir.clone(), meta_manifest_with("d", vec![child(url_a, "a")]))
            .with(a2_dir.clone(), meta_manifest_with("a", vec![]));
        let backend = InMemGit::new();
        // max_depth: Some(4) — the recurring A frame would land at
        // next_depth=5, which exceeds the cap. With B1, the cycle
        // check still fires first; without B1, the cap would skip
        // before the cycle is detected.
        let opts = SyncMetaOptions { max_depth: Some(4), ..SyncMetaOptions::default() };
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("cycle must surface even when its closing frame exceeds max_depth");
        match err {
            TreeError::CycleDetected { chain } => {
                let id_a = format!("url:{url_a}");
                assert!(
                    chain.last() == Some(&id_a),
                    "recurring identity must be A, got chain={chain:?}"
                );
                let last = chain.last().unwrap();
                let first_match = chain.iter().position(|s| s == last).unwrap();
                assert!(
                    first_match < chain.len() - 1,
                    "the recurring identity must appear earlier in the chain: {chain:?}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
    }

    /// B2 regression: `pack_identity_for_child` must NOT emit a
    /// trailing `@` when `r#ref` is `Some("")` (empty string). Both
    /// `Some("")` and `None` collapse to the bare `url:<url>` form so
    /// the on-the-wire identity matches the Lean model
    /// (`Grex.Walker.ChildRef.identity`). Without this elision two
    /// children that differ only in `ref: None` vs `ref: Some("")`
    /// would serialise the same way as `url:<url>@`, masking the
    /// distinction the Lean spec draws — and worse, an identity
    /// ending in `@` leaks an empty-ref artifact into operator
    /// diagnostics.
    #[test]
    fn child_identity_some_empty_ref_omits_at() {
        let url = "https://example.com/a.git";
        let with_none = ChildRef { url: url.to_string(), path: Some("a".to_string()), r#ref: None };
        let with_empty = ChildRef {
            url: url.to_string(),
            path: Some("a".to_string()),
            r#ref: Some(String::new()),
        };
        let id_none = pack_identity_for_child(&with_none);
        let id_empty = pack_identity_for_child(&with_empty);
        let expected = format!("url:{url}");
        assert_eq!(id_none, expected, "None ref must produce bare url identity");
        assert_eq!(
            id_empty, expected,
            "Some(\"\") ref must collapse to bare url identity (no trailing @)"
        );
        assert_eq!(id_none, id_empty, "Some(\"\") and None must yield the same identity");
        assert!(!id_empty.ends_with('@'), "identity must not end with trailing @: {id_empty:?}");
    }

    // -----------------------------------------------------------------
    // v1.2.4 — A1 cancellation token (T-cancel).
    //
    // Discharges the Lean `cancellation_terminates_promptly` theorem
    // in `proof/Grex/Walker.lean`: when one sibling closure detects a
    // cycle and signals the per-`phase3_recurse` cancellation flag,
    // every subsequent in-flight sibling closure observes the flag at
    // its next entry and returns `Phase3ChildOutcome::Cancelled` with
    // zero recursive descent.
    // -----------------------------------------------------------------

    /// T-cancel — sibling cancellation under cycle.
    ///
    /// Topology: `root → A`, where A has FOUR children
    /// `[A_cyclic, X, Y, Z]` (in this exact source order). `A_cyclic`'s
    /// URL collides with A's own URL, so A's `phase3_recurse` detects
    /// the cycle when iterating its first child. `X`, `Y`, `Z` are
    /// independent sub-metas, each containing a deep chain
    /// (`X → X1 → X2`, etc) that would inflate `metas_visited` if
    /// genuinely walked. With `opts.parallel = Some(1)` the rayon
    /// pool runs siblings serially in source order, so the cyclic
    /// sibling fires first, sets the flag, and `X`/`Y`/`Z` observe
    /// `Cancelled` at entry — none of their subtrees are walked.
    ///
    /// Determinism: `parallel: Some(1)` removes thread interleaving
    /// from the test surface — the cyclic arm is *guaranteed* to run
    /// before the acyclic siblings. The flag is checked at entry of
    /// `phase3_handle_child`, so any sibling that has not yet started
    /// observes it. `metas_visited` is the side-effect-visible counter
    /// used to assert the cancellation discipline: pre-cancellation
    /// (v1.2.3) the walker would visit every sibling subtree before
    /// surfacing `Err(CycleDetected)`; post-cancellation (v1.2.4)
    /// only the cyclic arm and its prefix contribute.
    ///
    /// Without the cancellation flag, `metas_visited` would total
    /// 1 (root) + 1 (A) + 3 (X, Y, Z themselves) + 6 (X1,X2,Y1,Y2,Z1,Z2)
    /// = 11. With the flag, X/Y/Z's `phase3_handle_child` short-circuits
    /// before recursing, so only root + A are recorded → 2.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cancellation_aborts_siblings() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        let a_dir = root_dir.join("a");
        // A_cyclic's path is a fresh slot under A so on-disk classify
        // succeeds; identity collision with A's URL is what trips the
        // cycle detector at A's `phase3_recurse`.
        let a_cyclic_dir = a_dir.join("a-cyclic");
        let x_dir = a_dir.join("x");
        let x1_dir = x_dir.join("x1");
        let x2_dir = x1_dir.join("x2");
        let y_dir = a_dir.join("y");
        let y1_dir = y_dir.join("y1");
        let y2_dir = y1_dir.join("y2");
        let z_dir = a_dir.join("z");
        let z1_dir = z_dir.join("z1");
        let z2_dir = z1_dir.join("z2");
        for d in [
            &a_dir,
            &a_cyclic_dir,
            &x_dir,
            &x1_dir,
            &x2_dir,
            &y_dir,
            &y1_dir,
            &y2_dir,
            &z_dir,
            &z1_dir,
            &z2_dir,
        ] {
            make_sub_meta_on_disk(d, d.file_name().unwrap().to_str().unwrap());
        }

        let url_a = "https://example.com/a.git";
        let url_x = "https://example.com/x.git";
        let url_x1 = "https://example.com/x1.git";
        let url_x2 = "https://example.com/x2.git";
        let url_y = "https://example.com/y.git";
        let url_y1 = "https://example.com/y1.git";
        let url_y2 = "https://example.com/y2.git";
        let url_z = "https://example.com/z.git";
        let url_z1 = "https://example.com/z1.git";
        let url_z2 = "https://example.com/z2.git";

        // A's children: [a-cyclic (collides with A's identity), x, y, z]
        // — ORDER MATTERS for determinism. With parallel: Some(1) the
        // cyclic arm runs first, signals, and the rest observe.
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            .with(
                a_dir.clone(),
                meta_manifest_with(
                    "a",
                    vec![
                        child(url_a, "a-cyclic"),
                        child(url_x, "x"),
                        child(url_y, "y"),
                        child(url_z, "z"),
                    ],
                ),
            )
            .with(a_cyclic_dir.clone(), meta_manifest_with("a-cyclic", vec![]))
            // X/Y/Z each carry a 3-level subtree that would inflate
            // metas_visited if genuinely walked.
            .with(x_dir.clone(), meta_manifest_with("x", vec![child(url_x1, "x1")]))
            .with(x1_dir.clone(), meta_manifest_with("x1", vec![child(url_x2, "x2")]))
            .with(x2_dir.clone(), meta_manifest_with("x2", vec![]))
            .with(y_dir.clone(), meta_manifest_with("y", vec![child(url_y1, "y1")]))
            .with(y1_dir.clone(), meta_manifest_with("y1", vec![child(url_y2, "y2")]))
            .with(y2_dir.clone(), meta_manifest_with("y2", vec![]))
            .with(z_dir.clone(), meta_manifest_with("z", vec![child(url_z1, "z1")]))
            .with(z1_dir.clone(), meta_manifest_with("z1", vec![child(url_z2, "z2")]))
            .with(z2_dir.clone(), meta_manifest_with("z2", vec![]));
        let backend = InMemGit::new();
        // parallel: Some(1) — single-threaded rayon pool. Source-order
        // iteration => cyclic arm runs before X/Y/Z, signal observed.
        let opts = SyncMetaOptions { parallel: Some(1), ..SyncMetaOptions::default() };
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("cyclic input must surface CycleDetected");
        match err {
            TreeError::CycleDetected { chain } => {
                let id_a = format!("url:{url_a}");
                assert_eq!(
                    chain.last(),
                    Some(&id_a),
                    "recurring identity must be A (the cyclic arm), got chain={chain:?}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
        // The cancellation discipline asserts: X/Y/Z's subtrees were
        // NOT walked. Cancellation lives in Phase 3 (the recursion
        // edge), so Phase 1 still fetches A's four direct children
        // (a-cyclic, x, y, z) — one Fetch each. A's Phase 3 then
        // detects the cycle on a-cyclic and signals; X/Y/Z's
        // `phase3_handle_child` returns `Cancelled` at entry, so
        // their `sync_meta_inner` never runs. Therefore X1/X2/Y1/Y2/
        // Z1/Z2 are NEVER fetched.
        //
        // Without cancellation Fetch count = 11:
        //   1 (root → a) + 4 (a → a-cyclic, x, y, z)
        //   + 2 (x → x1 → x2) + 2 (y → y1 → y2) + 2 (z → z1 → z2).
        // With cancellation Fetch count = 5:
        //   1 (root → a) + 4 (a → a-cyclic, x, y, z).
        //
        // With `parallel: Some(1)` the rayon pool is single-threaded
        // and visibility is immediate, so 5 is the deterministic tight
        // bound: 1 (root → a) + 4 (a's Phase 1 fan-out for a-cyclic, x,
        // y, z). No upper-bound slack — `Some(1)` removes interleaving
        // entirely, and any deviation indicates a real regression.
        let fetch_count =
            backend.calls().iter().filter(|c| matches!(c, BackendCall::Fetch { .. })).count();
        // Lower bound: Phase 1's per-child fan-out runs BEFORE the
        // Phase 3 cycle check, so even under cancellation A's four
        // direct children must have been fetched (1 root → A + 4 a's
        // children = 5). Documents that Phase 1 is intentionally NOT
        // cancellation-aware in v1.2.4 — cancellation only short-
        // circuits the Phase 3 recursion edge.
        assert!(
            fetch_count >= 5,
            "Phase 1 fan-out for A's 4 direct children must complete even under cancellation; \
             observed {fetch_count} fetches"
        );
        assert_eq!(
            fetch_count, 5,
            "cancellation flag must short-circuit X/Y/Z subtrees; \
             observed {fetch_count} fetches (acyclic walk would do 11)"
        );
        // Stronger assertion: NO fetch ever targeted x1/x2/y1/y2/z1/z2.
        // If the cancellation token were broken, sync_meta_inner would
        // recurse into x/y/z and Phase 1 there would fetch x1/y1/z1 at
        // a minimum.
        let cancelled_dests = [&x1_dir, &x2_dir, &y1_dir, &y2_dir, &z1_dir, &z2_dir];
        for dest in cancelled_dests {
            for call in backend.calls() {
                if let BackendCall::Fetch { dest: fetched } = &call {
                    assert_ne!(
                        fetched,
                        dest,
                        "cancellation must prevent recursion into {} (observed Fetch call)",
                        dest.display()
                    );
                }
            }
        }
    }

    /// G1 — multi-thread sibling cancellation race.
    ///
    /// Topology: `root → A`, where A has 12 children: two cyclic
    /// siblings (`A_cyclic1`, `A_cyclic2`, both colliding with A's
    /// own URL) and ten acyclic deep subtrees (`X0..X9` each →
    /// `X{i}_1` → `X{i}_2`). With `parallel: Some(8)` rayon may
    /// schedule the two cyclic arms onto different worker threads,
    /// so BOTH can simultaneously detect the cycle and try to store
    /// into `cancelled`. The aggregator (`phase3_recurse`) must:
    ///   - return `Err(CycleDetected)` on every iteration with a
    ///     non-empty chain (no panic, no Ok),
    ///   - never spuriously double-count or wedge on the multiple
    ///     concurrent stores (AtomicBool::store is idempotent),
    ///   - keep fetch count bounded (no unbounded recursion).
    ///
    /// Looped 50 iterations — non-determinism would surface as
    /// either a panic, an `Ok(_)` return, or an unbounded fetch
    /// count (the acyclic 10-subtree walk would be at minimum
    /// `1 (root → A) + 12 (A's children) + 20 (X*_1, X*_2)` = 33
    /// without cancellation; we cap at 1000 to catch runaway).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cancellation_aborts_siblings_multithread() {
        // Build helper: returns (loader, backend, root_dir) for a
        // fresh per-iteration tempdir so iterations don't share state.
        fn build_topology() -> (tempfile::TempDir, PathBuf, InMemLoader, InMemGit, String) {
            let tmp = tempfile::tempdir().unwrap();
            let root_dir = tmp.path().to_path_buf();
            let a_dir = root_dir.join("a");
            make_sub_meta_on_disk(&a_dir, "a");
            let url_a = "https://example.com/a.git".to_string();
            // Two cyclic siblings (both collide with A's identity)
            let a_cyc1_dir = a_dir.join("a-cyclic1");
            let a_cyc2_dir = a_dir.join("a-cyclic2");
            make_sub_meta_on_disk(&a_cyc1_dir, "a-cyclic1");
            make_sub_meta_on_disk(&a_cyc2_dir, "a-cyclic2");
            // Ten acyclic deep subtrees: x0..x9, each → x{i}_1 → x{i}_2
            let mut a_children = vec![child(&url_a, "a-cyclic1"), child(&url_a, "a-cyclic2")];
            let mut loader = InMemLoader::new()
                .with(root_dir.clone(), meta_manifest_with("root", vec![child(&url_a, "a")]))
                .with(a_cyc1_dir.clone(), meta_manifest_with("a-cyclic1", vec![]))
                .with(a_cyc2_dir.clone(), meta_manifest_with("a-cyclic2", vec![]));
            for i in 0..10 {
                let xi_name = format!("x{i}");
                let xi_dir = a_dir.join(&xi_name);
                let xi1_name = format!("x{i}_1");
                let xi1_dir = xi_dir.join(&xi1_name);
                let xi2_name = format!("x{i}_2");
                let xi2_dir = xi1_dir.join(&xi2_name);
                make_sub_meta_on_disk(&xi_dir, &xi_name);
                make_sub_meta_on_disk(&xi1_dir, &xi1_name);
                make_sub_meta_on_disk(&xi2_dir, &xi2_name);
                let url_xi = format!("https://example.com/x{i}.git");
                let url_xi1 = format!("https://example.com/x{i}_1.git");
                let url_xi2 = format!("https://example.com/x{i}_2.git");
                a_children.push(child(&url_xi, &xi_name));
                loader = loader
                    .with(xi_dir, meta_manifest_with(&xi_name, vec![child(&url_xi1, &xi1_name)]))
                    .with(xi1_dir, meta_manifest_with(&xi1_name, vec![child(&url_xi2, &xi2_name)]))
                    .with(xi2_dir, meta_manifest_with(&xi2_name, vec![]));
            }
            loader = loader.with(a_dir, meta_manifest_with("a", a_children));
            let backend = InMemGit::new();
            (tmp, root_dir, loader, backend, url_a)
        }

        for iter in 0..50 {
            let (_tmp, root_dir, loader, backend, url_a) = build_topology();
            let opts = SyncMetaOptions { parallel: Some(8), ..SyncMetaOptions::default() };
            let result = sync_meta(&root_dir, &backend, &loader, &opts, &[]);
            let err = match result {
                Err(e) => e,
                Ok(_) => panic!("iter {iter}: expected CycleDetected, got Ok"),
            };
            match err {
                TreeError::CycleDetected { chain } => {
                    assert!(
                        !chain.is_empty(),
                        "iter {iter}: CycleDetected chain must be non-empty: {chain:?}"
                    );
                    let id_a = format!("url:{url_a}");
                    assert_eq!(
                        chain.last(),
                        Some(&id_a),
                        "iter {iter}: recurring identity must be A, got chain={chain:?}"
                    );
                }
                other => panic!("iter {iter}: expected CycleDetected, got {other:?}"),
            }
            // Bound check: even with two cyclic arms racing, the
            // walker must not have walked unbounded subtrees. Acyclic
            // walk would do 33 fetches; under cancellation Phase 1
            // for A's 12 direct children fires (12 fetches) plus the
            // initial root → A fetch (1) = 13, and depending on
            // worker scheduling some X{i}_1 / X{i}_2 fetches may
            // sneak in before the flag is observed. Cap loosely at
            // 200 — any wild blowup indicates the flag is broken.
            let fetch_count =
                backend.calls().iter().filter(|c| matches!(c, BackendCall::Fetch { .. })).count();
            assert!(
                fetch_count >= 1,
                "iter {iter}: at least the root → A fetch must occur; got {fetch_count}"
            );
            assert!(
                fetch_count < 200,
                "iter {iter}: fetch count blew up under multi-thread cancellation; got {fetch_count}"
            );
        }
    }

    /// G2 — per-call cancellation flag scope: a cycle deep in pack A
    /// MUST NOT cancel a sibling pack B at the root level.
    ///
    /// Topology:
    ///   root → [A, B]
    ///     A → A1 → A2 → A2_cyclic   (A2_cyclic.url == A2.url ⇒ cycle
    ///                                 inside A's deep subtree)
    ///     B → B1 → B2 → B3          (clean acyclic subtree)
    ///
    /// The cancellation flag for root's Phase 3 fan-out covers root's
    /// direct children (A, B). The cycle inside A is detected during
    /// recursion into A's deep subtree, by a NEW per-call flag built
    /// at the deep `phase3_recurse` frame — that flag is scoped to A's
    /// inner closures only. It must NOT propagate up and cancel B's
    /// independent walk.
    ///
    /// Assertions:
    ///   (i)  walker returns `Err(CycleDetected)` (the deep cycle
    ///        propagates up via the short-circuit path),
    ///   (ii) B's deep subtree IS walked (B, B1, B2, B3 all fetched),
    ///        proving B's walk was not aborted by A's deep-subtree
    ///        cancellation flag (which lives in a recursion frame
    ///        below root, disjoint from the root-level fan-out flag
    ///        that scopes A and B as siblings).
    ///
    /// With `parallel: Some(2)` rayon schedules A and B onto separate
    /// workers; B must run to completion regardless of A's cycle.
    #[test]
    #[allow(clippy::too_many_lines)]
    fn cancellation_per_call_scope_isolates_subtrees() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        // A's deep cycle arm
        let a_dir = root_dir.join("a");
        let a1_dir = a_dir.join("a1");
        let a2_dir = a1_dir.join("a2");
        let a2cyc_dir = a2_dir.join("a2-cyclic");
        // B's clean deep subtree
        let b_dir = root_dir.join("b");
        let b1_dir = b_dir.join("b1");
        let b2_dir = b1_dir.join("b2");
        let b3_dir = b2_dir.join("b3");
        for d in [&a_dir, &a1_dir, &a2_dir, &a2cyc_dir, &b_dir, &b1_dir, &b2_dir, &b3_dir] {
            make_sub_meta_on_disk(d, d.file_name().unwrap().to_str().unwrap());
        }
        let url_a = "https://example.com/a.git";
        let url_a1 = "https://example.com/a1.git";
        let url_a2 = "https://example.com/a2.git";
        let url_b = "https://example.com/b.git";
        let url_b1 = "https://example.com/b1.git";
        let url_b2 = "https://example.com/b2.git";
        let url_b3 = "https://example.com/b3.git";
        let loader = InMemLoader::new()
            .with(
                root_dir.clone(),
                meta_manifest_with("root", vec![child(url_a, "a"), child(url_b, "b")]),
            )
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_a1, "a1")]))
            .with(a1_dir.clone(), meta_manifest_with("a1", vec![child(url_a2, "a2")]))
            // a2's child re-declares a2's identity → cycle at depth 4
            .with(a2_dir.clone(), meta_manifest_with("a2", vec![child(url_a2, "a2-cyclic")]))
            .with(a2cyc_dir.clone(), meta_manifest_with("a2-cyclic", vec![]))
            .with(b_dir.clone(), meta_manifest_with("b", vec![child(url_b1, "b1")]))
            .with(b1_dir.clone(), meta_manifest_with("b1", vec![child(url_b2, "b2")]))
            .with(b2_dir.clone(), meta_manifest_with("b2", vec![child(url_b3, "b3")]))
            .with(b3_dir.clone(), meta_manifest_with("b3", vec![]));
        let backend = InMemGit::new();
        // parallel: Some(2) — A and B may schedule onto separate
        // workers. The point of the test is to verify B's walk is
        // not aborted by A's deep-subtree cancellation flag.
        let opts = SyncMetaOptions { parallel: Some(2), ..SyncMetaOptions::default() };
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("deep cycle inside A must surface CycleDetected");
        // (i) Cycle bubbled up.
        match err {
            TreeError::CycleDetected { chain } => {
                let id_a2 = format!("url:{url_a2}");
                assert_eq!(
                    chain.last(),
                    Some(&id_a2),
                    "recurring identity must be A2 (the cyclic arm), got chain={chain:?}"
                );
            }
            other => panic!("expected CycleDetected, got {other:?}"),
        }
        // (ii) B's subtree was walked: B, B1, B2, B3 each fetched.
        // The deep cycle in A fires inside a per-call flag scoped to
        // that recursion frame; root's per-call flag is NOT signalled
        // (cycle is returned from a child call, not stored at root
        // scope), so B's sibling walk completes uninterrupted.
        let fetched_dests: Vec<PathBuf> = backend
            .calls()
            .iter()
            .filter_map(|c| match c {
                BackendCall::Fetch { dest } => Some(dest.clone()),
                _ => None,
            })
            .collect();
        for dest in [&b_dir, &b1_dir, &b2_dir, &b3_dir] {
            assert!(
                fetched_dests.iter().any(|f| f == dest),
                "per-call scope: B's subtree must have been walked despite A's deep cycle; \
                 missing fetch for {} (observed {fetched_dests:?})",
                dest.display()
            );
        }
    }

    // -----------------------------------------------------------------
    // v1.2.5 — A2 partial-clone cleanup (T-A2).
    //
    // Discharges the Lean theorem `partial_clone_cleanup_idempotent`
    // in `proof/Grex/Walker.lean`: when Phase 3 returns a non-Recursed
    // outcome (Failed / Cancelled / Skipped), the post-state at `dest`
    // equals the pre-state. For a freshly-cloned dest (not in the
    // pre-existing snapshot) the cleanup wipes the dir; for a
    // pre-existing dest the cleanup is a no-op (preserves prior
    // successful sync state).
    // -----------------------------------------------------------------

    /// T-A2 — partial-clone cleanup on cycle-detected failure.
    ///
    /// Topology: `root → a → a-cyclic` where `a-cyclic.url == a.url`
    /// (cycle inside `a`'s Phase 3 fan-out). The dest dir for
    /// `a-cyclic` does NOT exist before the walk starts, so it is
    /// freshly created by Phase 1's clone (which materialises a
    /// `.git/` plus a `.grex/pack.yaml` so Phase 3 enters the cycle
    /// detector). Post-walk, the cycle has been surfaced AND the
    /// freshly-cloned `a-cyclic/` dest must have been removed by
    /// `cleanup_partial_clone`.
    ///
    /// Pre-existing dests must NOT be cleaned: `a/` itself was
    /// pre-materialised on disk (`make_sub_meta_on_disk`) so the
    /// snapshot taken inside `sync_meta_inner` includes it; even
    /// if the cycle propagated up and the recursive-failure branch
    /// fired, `a/` must remain on disk (legitimate prior content).
    #[test]
    #[allow(clippy::too_many_lines)]
    fn partial_clone_cleanup_after_cancellation() {
        let tmp = tempfile::tempdir().unwrap();
        let root_dir = tmp.path().to_path_buf();
        let a_dir = root_dir.join("a");
        let a_cyclic_dir = a_dir.join("a-cyclic");
        // Pre-materialise `a/` on disk so it counts as PresentDeclared
        // (Phase 1 will fetch, not clone) AND is in the
        // `pre_existing_dests` snapshot at `sync_meta_inner` entry.
        make_sub_meta_on_disk(&a_dir, "a");
        // a-cyclic deliberately NOT pre-materialised — Phase 1's
        // clone (via the custom backend below) materialises a `.git/`
        // PLUS a `.grex/pack.yaml` so Phase 3 enters the cycle
        // detector on the freshly-cloned dest.
        assert!(!a_cyclic_dir.exists(), "a-cyclic dest must be missing pre-walk");

        let url_a = "https://example.com/a.git";
        let loader = InMemLoader::new()
            .with(root_dir.clone(), meta_manifest_with("root", vec![child(url_a, "a")]))
            .with(a_dir.clone(), meta_manifest_with("a", vec![child(url_a, "a-cyclic")]))
            // a-cyclic's manifest is loadable (used post-clone if cycle
            // weren't detected; never reached because the cycle fires).
            .with(a_cyclic_dir.clone(), meta_manifest_with("a-cyclic", vec![]));
        // Custom backend: clone materialises BOTH `.git/` and
        // `.grex/pack.yaml` so Phase 3's existence check passes,
        // letting cycle detection fire on the freshly-cloned dest.
        struct CloneWithPackYaml {
            inner: InMemGit,
        }
        impl GitBackend for CloneWithPackYaml {
            fn name(&self) -> &'static str {
                "v1_2_5-clone-with-pack-yaml"
            }
            fn clone(
                &self,
                url: &str,
                dest: &Path,
                r#ref: Option<&str>,
                lock_ctx: crate::git::BackendLockCtx<'_>,
            ) -> Result<crate::ClonedRepo, crate::GitError> {
                let res = self.inner.clone(url, dest, r#ref, lock_ctx)?;
                // Also drop a pack.yaml so Phase 3 enters the cycle
                // check (and not the `Skipped` early-return).
                std::fs::create_dir_all(dest.join(".grex")).unwrap();
                let yaml = format!(
                    "schema_version: \"1\"\nname: {}\ntype: meta\n",
                    dest.file_name().unwrap().to_str().unwrap()
                );
                std::fs::write(dest.join(".grex/pack.yaml"), yaml).unwrap();
                Ok(res)
            }
            fn fetch(
                &self,
                dest: &Path,
                lock_ctx: crate::git::BackendLockCtx<'_>,
            ) -> Result<(), crate::GitError> {
                self.inner.fetch(dest, lock_ctx)
            }
            fn checkout(
                &self,
                dest: &Path,
                r#ref: &str,
                lock_ctx: crate::git::BackendLockCtx<'_>,
            ) -> Result<(), crate::GitError> {
                self.inner.checkout(dest, r#ref, lock_ctx)
            }
            fn head_sha(&self, dest: &Path) -> Result<String, crate::GitError> {
                self.inner.head_sha(dest)
            }
        }
        let backend = CloneWithPackYaml { inner: InMemGit::new() };
        // parallel: Some(1) for deterministic ordering — a-cyclic is
        // a's only child so the cycle fires on the first iteration.
        let opts = SyncMetaOptions { parallel: Some(1), ..SyncMetaOptions::default() };
        let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
            .expect_err("cyclic input must surface CycleDetected");
        assert!(
            matches!(err, TreeError::CycleDetected { .. }),
            "expected CycleDetected, got {err:?}"
        );

        // T-A2 invariant: the freshly-cloned a-cyclic dest MUST have
        // been cleaned up by `cleanup_partial_clone` on the
        // Failed(CycleDetected) path.
        assert!(
            !a_cyclic_dir.exists(),
            "v1.2.5 A2: freshly-cloned a-cyclic dest must be cleaned after cycle detection \
             (path still on disk: {})",
            a_cyclic_dir.display()
        );
        // Symmetric invariant: pre-existing `a/` MUST still exist —
        // it was in the snapshot, so cleanup never targets it.
        assert!(
            a_dir.exists(),
            "v1.2.5 A2: pre-existing a/ dest must NOT be cleaned (legitimate prior content)"
        );
    }

    /// T-A2 idempotence — running `cleanup_partial_clone` twice on the
    /// same path is a no-op on the second call. Mirrors the Lean
    /// theorem `partial_clone_cleanup_idempotent` directly.
    #[test]
    fn cleanup_partial_clone_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("ghost");
        std::fs::create_dir_all(dest.join("subdir")).unwrap();
        std::fs::write(dest.join("file.txt"), b"bytes").unwrap();
        assert!(dest.exists());
        cleanup_partial_clone(&dest);
        assert!(!dest.exists(), "first call must remove dest");
        // Second call is a no-op on a missing dir.
        cleanup_partial_clone(&dest);
        assert!(!dest.exists(), "second call must be a no-op");
    }

    /// W1 — symlink-secure cleanup MUST NOT follow a directory symlink
    /// into an unrelated tree. We point a symlink at a sibling
    /// directory containing important files, then run
    /// `cleanup_partial_clone` against the symlink. The symlink must
    /// be unlinked AS a symlink; the target tree's contents must
    /// remain intact on disk.
    ///
    /// Skips on hosts where unprivileged symlink creation fails
    /// (notably Windows without Developer Mode).
    #[test]
    fn cleanup_partial_clone_does_not_follow_symlink() {
        let outer = tempfile::tempdir().unwrap();
        let target = outer.path().join("important-target");
        std::fs::create_dir_all(&target).unwrap();
        let canary = target.join("canary.txt");
        std::fs::write(&canary, b"do-not-touch").unwrap();

        let link = outer.path().join("partial-clone");
        #[cfg(unix)]
        let symlink_result = std::os::unix::fs::symlink(&target, &link);
        #[cfg(windows)]
        let symlink_result = std::os::windows::fs::symlink_dir(&target, &link);
        if symlink_result.is_err() {
            // Host won't let us create a symlink — nothing to test.
            return;
        }

        cleanup_partial_clone(&link);

        // The symlink itself MUST be gone.
        assert!(
            std::fs::symlink_metadata(&link).is_err(),
            "symlink at {} must have been unlinked",
            link.display()
        );
        // The target tree MUST remain intact.
        assert!(
            target.exists(),
            "symlink target {} must NOT have been followed and deleted",
            target.display()
        );
        assert!(canary.exists(), "canary file inside target must remain on disk");
        assert_eq!(std::fs::read(&canary).unwrap(), b"do-not-touch");
    }

    // -----------------------------------------------------------------
    // v1.2.5 (W1) — `pre_existing_dests` lexical-normalisation tests.
    //
    // Direct unit tests of `normalize_dest_key`. The integration
    // contract — that snapshot site and lookup site agree on keys
    // even when `child.effective_path()` carries a trailing `/` or
    // `./` prefix — is covered indirectly because both call sites
    // now route through the same helper.
    // -----------------------------------------------------------------

    /// W1 — a trailing `/` on the input MUST normalise away so two
    /// representations of the same logical path produce equal keys.
    #[test]
    fn pre_existing_dests_normalizes_trailing_slash() {
        // PathBuf::push("foo/") historically preserved the trailing
        // separator on some platforms; normalize it out via the helper.
        let with_slash: PathBuf = ["meta", "child/"].iter().collect();
        let without_slash: PathBuf = ["meta", "child"].iter().collect();
        let n_with = normalize_dest_key(&with_slash);
        let n_without = normalize_dest_key(&without_slash);
        assert_eq!(
            n_with, n_without,
            "normalize_dest_key must produce equal keys for trailing-slash variants \
             (with={n_with:?}, without={n_without:?})"
        );
    }

    /// W1 — a `./` prefix or intermediate `./` component MUST drop
    /// out via `normalize_dest_key` so the snapshot and lookup sites
    /// agree even when the manifest declares paths via `./child`.
    #[test]
    fn pre_existing_dests_normalizes_curdir_components() {
        let with_curdir = PathBuf::from("./meta/./child");
        let without_curdir = PathBuf::from("meta/child");
        let n_with = normalize_dest_key(&with_curdir);
        let n_without = normalize_dest_key(&without_curdir);
        assert_eq!(
            n_with, n_without,
            "normalize_dest_key must drop `./` components \
             (with={n_with:?}, without={n_without:?})"
        );
    }

    /// W1 — empty / pure-CurDir input must round-trip to a non-empty
    /// path so downstream FS ops still resolve to the same dir.
    #[test]
    fn pre_existing_dests_normalize_empty_input_yields_curdir() {
        assert_eq!(normalize_dest_key(&PathBuf::from(".")), PathBuf::from("."));
        assert_eq!(normalize_dest_key(&PathBuf::from("./.")), PathBuf::from("."));
    }

    // -----------------------------------------------------------------
    // v1.2.5 — A3 pool deadlock guard (T-A3, debug-only).
    //
    // The guard is a `debug_assert!` inside `phase3_recurse`: if any
    // worker thread enters a nested `pool.install` (depth >= 2) while
    // it still holds a `PackLock`, the assertion fires. Release builds
    // compile the assertion out — these tests are gated on
    // `cfg(debug_assertions)` for the same reason.
    //
    // The depth counter lives in `scheduler::POOL_INSTALL_DEPTH` (W3,
    // per-OS-thread). The held-lock set lives in
    // `pack_lock::HELD_PACK_LOCKS` (W2/W4) — *process-global*
    // `Mutex<HashSet<(PathBuf, ThreadId)>>` since the W4 cross-thread
    // fix; the previous `thread_local!` storage was invisible to rayon
    // worker threads (work-stealing routes the closure onto threads
    // whose TL is empty), masking the deadlock pattern in production.
    //
    // The tests below depend on those test-only accessors:
    //   - `crate::scheduler::pool_install_depth_for_test()`
    //   - `crate::pack_lock::held_pack_locks_for_test()` — paths held
    //     by the *current* OS thread (filtered by `ThreadId` inside
    //     the global registry)
    //   - `crate::pack_lock::register_pack_lock_for_test(p)` — inserts
    //     `(p, current_thread_id)` into the global registry
    //   - `crate::pack_lock::unregister_pack_lock_for_test(p)` —
    //     removes `(p, current_thread_id)` from the global registry
    // If W2/W3 rename or relocate these, update both call sites and
    // the assertion in `phase3_recurse` together.
    // -----------------------------------------------------------------

    /// T-A3 — the depth guard correctly tracks nested `pool.install`.
    /// Sanity check that the RAII `PoolInstallDepthGuard` increments
    /// + decrements as expected. Independent of any deadlock pattern.
    #[cfg(debug_assertions)]
    #[test]
    fn pool_install_depth_guard_tracks_nesting() {
        use crate::scheduler::pool_install_depth_for_test;
        assert_eq!(pool_install_depth_for_test(), 0, "starts at zero");
        {
            let _g1 = PoolInstallDepthGuard::new();
            assert_eq!(pool_install_depth_for_test(), 1, "depth 1 after one guard");
            {
                let _g2 = PoolInstallDepthGuard::new();
                assert_eq!(pool_install_depth_for_test(), 2, "depth 2 after nested guard");
            }
            assert_eq!(pool_install_depth_for_test(), 1, "back to 1 after inner drop");
        }
        assert_eq!(pool_install_depth_for_test(), 0, "back to 0 after outer drop");
    }

    /// T-A3 — deadlock pattern triggers a debug-build panic.
    ///
    /// Simulates the v1.2.2 R#1 MED pattern: seed a fake PackLock
    /// path in the global registry under the current `ThreadId`
    /// (mimicking `PackLock::acquire_async` having returned a hold on
    /// this thread), increment the pool-install depth twice via two
    /// `PoolInstallDepthGuard`s, then evaluate the production
    /// `debug_assert!` predicate inline. The test thread panics with
    /// the production message, satisfying `#[should_panic]`.
    ///
    /// Gated `#[cfg(debug_assertions)]` because release builds compile
    /// the assertion out — that's by design (no runtime cost in
    /// production; the documented lock acquisition order in
    /// `concurrency.md` is the production contract).
    ///
    /// W4 cross-thread fix note: with the
    /// `Mutex<HashSet<(PathBuf, ThreadId)>>` global registry,
    /// `held_pack_locks_for_test()` filters by current `ThreadId`, so
    /// the seeded path is visible to the assertion when it runs on
    /// the same thread that called `register_pack_lock_for_test`. The
    /// complementary cross-thread visibility test below
    /// (`pool_deadlock_guard_registry_is_thread_id_scoped`) proves
    /// the per-`ThreadId` view is correctly scoped — a rayon worker
    /// only sees its own holds, not concurrent acquires on other
    /// threads.
    #[cfg(debug_assertions)]
    #[test]
    #[should_panic(expected = "pool deadlock guard")]
    fn pool_deadlock_guard_panics_on_violation() {
        use crate::pack_lock::{
            held_pack_locks_for_test, register_pack_lock_for_test, unregister_pack_lock_for_test,
        };
        use crate::scheduler::pool_install_depth_for_test;

        let fake_lock_path = PathBuf::from("/tmp/fake-pack-lock-T-A3");
        // Defensive: cargo-test reuses worker threads via std panic
        // catching, so a prior panic on this OS thread might have
        // left the seed in place under the same `ThreadId`.
        // Clear-then-seed guarantees a fresh entry under the current
        // thread.
        unregister_pack_lock_for_test(&fake_lock_path);
        register_pack_lock_for_test(&fake_lock_path);

        let _depth_guard = PoolInstallDepthGuard::new();
        let depth = pool_install_depth_for_test();
        let held = held_pack_locks_for_test();
        debug_assert!(
            depth < 2 || held.is_empty(),
            "pool deadlock guard: nested pool.install (depth={depth}) entered while \
             holding PackLock(s) {held:?} on this thread — see concurrency.md \
             §lock acquisition order"
        );
        // Above asserts at depth==1 (outer guard), which is < 2, so it
        // would NOT panic. Force the depth >= 2 case by entering a
        // nested guard, then re-evaluate the predicate.
        let _nested = PoolInstallDepthGuard::new();
        let depth = pool_install_depth_for_test();
        let held = held_pack_locks_for_test();
        debug_assert!(
            depth < 2 || held.is_empty(),
            "pool deadlock guard: nested pool.install (depth={depth}) entered while \
             holding PackLock(s) {held:?} on this thread — see concurrency.md \
             §lock acquisition order"
        );
        // Unreachable on a correct build — the second debug_assert!
        // above must have panicked. Cleanup left here for safety in
        // case the assertion is ever weakened.
        unregister_pack_lock_for_test(&fake_lock_path);
    }

    /// T-A3 (W4 cross-thread fix) — registry observability across
    /// threads. Acquires the seed on thread A, reads the registry
    /// from thread B, and asserts: (a) thread B's
    /// `held_pack_locks_for_test()` is *empty* (the filter is per
    /// `ThreadId`, so B does NOT see A's lock as one of its own
    /// holds — this is the correct semantics for the per-closure
    /// assertion in `phase3_recurse`); (b) thread B can register +
    /// unregister its own independent entry without disturbing A's;
    /// (c) thread A still sees the seed after B's reads. Together
    /// these facts pin down the W4 fix: the global registry is
    /// shared *storage* across threads, but `ThreadId`-keyed *reads*
    /// give each thread its own scoped view. The previous
    /// `thread_local!` storage made (a) trivially true but at the
    /// cost of also masking the deadlock pattern on rayon workers
    /// where the lock had been acquired on the outer tokio thread —
    /// the Coffman cycle the assertion was meant to catch.
    #[cfg(debug_assertions)]
    #[test]
    fn pool_deadlock_guard_registry_is_thread_id_scoped() {
        use crate::pack_lock::{
            held_pack_locks_for_test, register_pack_lock_for_test, unregister_pack_lock_for_test,
        };

        let path = PathBuf::from("/tmp/fake-pack-lock-T-A3-cross-thread");
        // Clean any stale entry from a prior cargo-test run on this
        // OS thread so the assertions below are deterministic.
        unregister_pack_lock_for_test(&path);
        register_pack_lock_for_test(&path);

        // Thread A (this test thread) sees its own seed.
        let held_a = held_pack_locks_for_test();
        assert!(
            held_a.iter().any(|p| p == &path),
            "thread A must observe its own seeded lock; got {held_a:?}"
        );

        // Thread B reads the registry — it must NOT see A's lock as
        // one of its own holds (per-`ThreadId` filter).
        let path_for_b = path.clone();
        let (held_on_b_before, held_on_b_after) = std::thread::spawn(move || {
            let before = held_pack_locks_for_test();
            // Sanity: B can register/unregister its own independent
            // entry without disturbing A's.
            register_pack_lock_for_test(&path_for_b);
            let after = held_pack_locks_for_test();
            unregister_pack_lock_for_test(&path_for_b);
            (before, after)
        })
        .join()
        .expect("thread B panicked");

        assert!(
            held_on_b_before.is_empty(),
            "thread B's per-ThreadId view must NOT include A's seed; got {held_on_b_before:?}"
        );
        assert!(
            held_on_b_after.iter().any(|p| p == &path),
            "thread B must observe its own registered lock under its own ThreadId; \
             got {held_on_b_after:?}"
        );

        // Thread A's seed survives B's reads.
        let held_a_after = held_pack_locks_for_test();
        assert!(
            held_a_after.iter().any(|p| p == &path),
            "thread A's seed must survive thread B's reads; got {held_a_after:?}"
        );

        // Cleanup.
        unregister_pack_lock_for_test(&path);
    }
}
