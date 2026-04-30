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

use rayon::prelude::*;

use crate::git::GitBackend;
use crate::pack::validate::child_path::{
    boundary_fs_reject_reason, boundary_reject_reason, check_one as check_child_path,
    nfc_duplicate_path,
};
use crate::pack::{ChildRef, PackManifest, PackType, PackValidationError, SchemaVersion};

use super::consent::phase2_prune;
use super::dest_class::{aggregate_untracked, classify_dest, DestClass};
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
#[derive(Debug, Clone)]
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
        }
    }
}

/// Outcome of one [`sync_meta`] invocation. Aggregated across every
/// recursion frame: a sub-meta's report is folded into its parent's
/// report at the end of Phase 3.
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
}

impl SyncMetaReport {
    fn merge(&mut self, mut child: SyncMetaReport) {
        self.metas_visited += child.metas_visited;
        self.phase1_classifications.append(&mut child.phase1_classifications);
        self.phase2_pruned.append(&mut child.phase2_pruned);
        self.errors.append(&mut child.errors);
    }
}

/// v1.2.0 Stage 1.g — three-phase per-meta walker entry point.
///
/// `meta_dir` is the on-disk directory containing the meta's
/// `.grex/pack.yaml`. `prune_candidates` is the list of orphan dests
/// (parent-relative) the caller's distributed-lockfile reader determined
/// no longer appear in `manifest.children` — empty until Stage 1.h
/// supplies the read side.
///
/// Discharges Lean theorems W1–W8, V1, C1, C2, F1 via the bridges in
/// `Bridge.lean`. The sequential implementation is a special case of
/// the `sync_disjoint_commutes` axiom (single permit, no interleaving)
/// so no new bridge axiom is required.
///
/// # Errors
///
/// Returns the *first* catastrophic error (manifest parse failure on
/// the supplied `meta_dir`). All recoverable errors land in
/// [`SyncMetaReport::errors`] and the walker continues — fail-loud,
/// not fail-fast.
pub fn sync_meta(
    meta_dir: &Path,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    prune_candidates: &[PathBuf],
) -> Result<SyncMetaReport, TreeError> {
    sync_meta_inner(meta_dir, backend, loader, opts, prune_candidates, /* depth */ 0)
}

fn sync_meta_inner(
    meta_dir: &Path,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    prune_candidates: &[PathBuf],
    depth: usize,
) -> Result<SyncMetaReport, TreeError> {
    let manifest = loader.load(meta_dir)?;
    // v1.2.0 Stage 1.c gate — every recursion frame re-runs the
    // path-traversal sweep before any child is touched on disk.
    validate_children_paths(&manifest)?;

    let mut report = SyncMetaReport { metas_visited: 1, ..SyncMetaReport::default() };

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
    phase3_recurse(&pool, meta_dir, &manifest, backend, loader, opts, depth, &mut report);

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
    };
    match class {
        DestClass::Missing => {
            if let Err(e) = phase1_clone(backend, child, &dest, opts) {
                out.error = Some(e);
            }
        }
        DestClass::PresentDeclared => {
            if let Err(e) = phase1_fetch(backend, child, &dest, opts) {
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

/// Phase 1 clone helper. Acquires the M6 `PackLock` on the prospective
/// dest's parent (`meta_dir`) for the duration of the clone — distinct
/// children clone serially within a meta to keep the scheduler-tier
/// model honest. Sibling parallelism is a 1.j follow-up.
fn phase1_clone(
    backend: &dyn GitBackend,
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
    backend.clone(&child.url, dest, effective_ref)?;
    Ok(())
}

/// Phase 1 fetch helper. Same locking discipline as `phase1_clone`.
fn phase1_fetch(
    backend: &dyn GitBackend,
    child: &ChildRef,
    dest: &Path,
    opts: &SyncMetaOptions,
) -> Result<(), TreeError> {
    backend.fetch(dest)?;
    let effective_ref = opts.ref_override.as_deref().or(child.r#ref.as_deref());
    if let Some(r) = effective_ref {
        backend.checkout(dest, r)?;
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
enum Phase3ChildOutcome {
    Skipped,
    Recursed(SyncMetaReport),
    Failed(TreeError),
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
#[allow(clippy::too_many_arguments)]
fn phase3_recurse(
    pool: &rayon::ThreadPool,
    meta_dir: &Path,
    manifest: &PackManifest,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    depth: usize,
    report: &mut SyncMetaReport,
) {
    if !opts.recurse {
        return;
    }
    let next_depth = depth + 1;
    if let Some(cap) = opts.max_depth {
        if next_depth > cap {
            return;
        }
    }
    let outcomes: Vec<Phase3ChildOutcome> = pool.install(|| {
        manifest
            .children
            .par_iter()
            .map(|child| {
                let dest = meta_dir.join(child.effective_path());
                if !dest.join(".grex").join("pack.yaml").is_file() {
                    return Phase3ChildOutcome::Skipped;
                }
                // Empty `prune_candidates` for the sub-meta — 1.h
                // supplies the sub-meta's distributed lockfile read
                // via the same caller pathway when it lands.
                match sync_meta_inner(&dest, backend, loader, opts, &[], next_depth) {
                    Ok(sub) => Phase3ChildOutcome::Recursed(sub),
                    Err(e) => Phase3ChildOutcome::Failed(e),
                }
            })
            .collect()
    });
    for outcome in outcomes {
        match outcome {
            Phase3ChildOutcome::Skipped => {}
            Phase3ChildOutcome::Recursed(sub) => report.merge(sub),
            Phase3ChildOutcome::Failed(e) => report.errors.push(e),
        }
    }
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
        fn fetch(&self, dest: &Path) -> Result<(), crate::GitError> {
            self.calls.lock().unwrap().push(BackendCall::Fetch { dest: dest.to_path_buf() });
            Ok(())
        }
        fn checkout(&self, dest: &Path, r#ref: &str) -> Result<(), crate::GitError> {
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
        // Note: 1.c's path-segment validator forbids slashes in the
        // `path:` field, so multi-segment nesting is achieved by
        // chaining single-segment children across recursion frames.
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
}
