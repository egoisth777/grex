//! Sync orchestrator — M3 Stage B slice 6.
//!
//! Glues the building blocks shipped in slices 1–5b into a single runnable
//! pipeline:
//!
//! 1. Walk a pack tree via [`Walker`] + [`FsPackLoader`] + a `GitBackend`.
//! 2. Run plan-phase validators (manifest-level + graph-level).
//! 3. Execute every action via a pluggable [`ActionExecutor`]
//!    ([`PlanExecutor`] for dry-run, [`FsExecutor`] for wet-run).
//! 4. Record each step as an [`Event::Sync`] entry in the pack-root's
//!    `.grex/grex.jsonl` event log.
//!
//! # Traversal order
//!
//! Nodes are executed in **depth-first post-order**: children fully install
//! before their parent. Rationale: parent packs commonly `require:` artifacts
//! created by children (e.g. a parent symlink whose `src` lives inside a
//! child). Running the root last matches the overlay-style dotfile-install
//! intent authors expect, and it matches how `walker.walk` is structured
//! (children are hydrated before the recursion returns).
//!
//! # Decoupling
//!
//! The CLI crate drives this module through a thin `run()` entry point;
//! [`SyncOptions`] is `#[non_exhaustive]` so new knobs (parallelism, filter
//! expressions, ref overrides) can land in later milestones without breaking
//! CLI callers. Errors aggregate into [`SyncError`] with a small, stable
//! variant set.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use thiserror::Error;

use crate::execute::{
    ActionExecutor, ExecCtx, ExecError, ExecResult, ExecStep, FsExecutor, PlanExecutor, Platform,
    StepKind,
};
use crate::fs::{ManifestLock, ScopedLock};
use crate::git::GixBackend;
use crate::lockfile::{
    compute_actions_hash, read_lockfile, write_lockfile, LockEntry, LockfileError,
};
use crate::manifest::{append_event, read_all, Event, ACTION_ERROR_SUMMARY_MAX, SCHEMA_VERSION};
use crate::pack::{Action, PackValidationError};
use crate::plugin::Registry;
use crate::tree::{FsPackLoader, PackGraph, PackNode, TreeError, Walker};
use crate::vars::VarEnv;

/// Inputs to [`run`].
///
/// Fields are public-writable so call sites can construct with struct
/// literals. Adding new knobs is still non-breaking: callers who use the
/// [`SyncOptions::default`] pattern + setters (or `..SyncOptions::default()`
/// on struct updates) will not need changes. The struct is deliberately
/// *not* marked `#[non_exhaustive]` — that would force a named-setter API
/// without adding real decoupling for an in-repo orchestrator type.
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// When `true`, use [`PlanExecutor`] (no filesystem mutations).
    pub dry_run: bool,
    /// When `false`, skip plan-phase validators (manifest + graph). Debug
    /// escape hatch; production callers should leave this `true`.
    pub validate: bool,
    /// Override workspace directory. `None` → `<pack_root>/.grex/workspace`.
    pub workspace: Option<PathBuf>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self { dry_run: false, validate: true, workspace: None }
    }
}

impl SyncOptions {
    /// Default options: wet-run, validators enabled, default workspace path.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

/// One executed (or planned) action step in a sync run.
///
/// Marked `#[non_exhaustive]` so new observability fields (timestamps,
/// plugin provenance) can land without breaking library consumers who
/// destructure the struct.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SyncStep {
    /// Name of the pack that owned the action.
    pub pack: String,
    /// 0-based index into the pack's top-level `actions` vector.
    pub action_idx: usize,
    /// The [`ExecStep`] record emitted by the executor.
    pub exec_step: ExecStep,
}

/// Outcome of a [`run`] invocation.
///
/// On fail-fast termination, `halted` carries the error that stopped the
/// sync; every completed step up to that point is still in `steps` so
/// callers can render a partial transcript.
///
/// Marked `#[non_exhaustive]` so new report-level fields (run id, metrics)
/// can land without breaking library consumers who destructure the struct.
#[non_exhaustive]
#[derive(Debug)]
pub struct SyncReport {
    /// Fully-walked pack graph (present even on halted runs).
    pub graph: PackGraph,
    /// Steps produced by the executor, in execution order.
    pub steps: Vec<SyncStep>,
    /// `Some(e)` if execution stopped before all actions ran.
    pub halted: Option<SyncError>,
    /// Non-fatal manifest-append warnings (one per failed event append).
    /// Kept as a separate field because spec marks event-log write failures
    /// as non-aborting.
    pub event_log_warnings: Vec<String>,
    /// `Some(r)` when the pre-run teardown scan found orphaned backup
    /// files or dangling [`Event::ActionStarted`] records from a prior
    /// crashed run. Informational only — the report is still returned and
    /// the sync proceeds. CLI renderers should surface a warning so the
    /// operator can decide whether to run a future `grex doctor` verb.
    pub pre_run_recovery: Option<RecoveryReport>,
}

/// Rich context attached to a [`SyncError::Halted`] variant.
///
/// Packages the pack + action position together with the underlying
/// executor error and an optional human-readable recovery hint. Marked
/// `#[non_exhaustive]` so future fields (step transcript, timestamp) can
/// land without breaking `match` arms or struct destructures.
#[non_exhaustive]
#[derive(Debug)]
pub struct HaltedContext {
    /// Name of the pack that owned the halted action.
    pub pack: String,
    /// 0-based index into the pack's top-level `actions` vector.
    pub action_idx: usize,
    /// Short action kind tag (e.g. `"symlink"`, `"exec"`).
    pub action_name: String,
    /// Underlying executor error.
    pub error: ExecError,
    /// Optional next-step suggestion for the operator. `None` when no
    /// generic hint applies — the executor error's own `Display` already
    /// tells the story.
    pub recovery_hint: Option<String>,
}

/// Error taxonomy surfaced by [`run`].
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum SyncError {
    /// The pack-tree walker failed (loader error, git error, cycle, …).
    #[error("tree walk failed: {0}")]
    Tree(#[from] TreeError),
    /// One or more plan-phase validators flagged the graph.
    #[error("validation failed: {errors:?}")]
    Validation {
        /// Aggregated errors from manifest-level + graph-level validators.
        errors: Vec<PackValidationError>,
    },
    /// An action executor returned an error.
    ///
    /// Retained for backward compatibility; new call sites should prefer
    /// [`SyncError::Halted`] which carries full pack + action context.
    /// Kept non-deprecated because [`From<ExecError>`] still materialises
    /// the variant for non-sync-loop callers (e.g. ad-hoc helpers).
    #[error("action execution failed: {0}")]
    Exec(#[from] ExecError),
    /// Action execution halted; full context (pack, action index, error,
    /// optional recovery hint) lives in [`HaltedContext`]. This is the
    /// variant the sync driver emits — [`SyncError::Exec`] is only
    /// surfaced by ancillary code paths.
    #[error(
        "sync halted at pack `{}` action #{} ({}): {}",
        .0.pack, .0.action_idx, .0.action_name, .0.error
    )]
    Halted(Box<HaltedContext>),
    /// Another `grex` process (or thread) already holds the workspace-level
    /// lock. The running sync refused to start to avoid racing two concurrent
    /// walkers into the same workspace. If the lock file at `lock_path` is
    /// stale (no other grex is actually running), remove it by hand.
    #[error(
        "workspace `{workspace}` is locked by another grex process (remove {lock_path:?} if stale)"
    )]
    WorkspaceBusy {
        /// Resolved workspace directory that the current run tried to lock.
        workspace: PathBuf,
        /// Sidecar lock file that is currently held.
        lock_path: PathBuf,
    },
    /// Reading or parsing the resolved-state lockfile failed. Surfaced as
    /// its own variant (rather than folded into `Validation`) because a
    /// corrupt / unreadable lockfile is an I/O or schema fault, not a
    /// dependency-satisfaction fault. Resolution is operator-level
    /// (restore a backup, delete the file, re-sync), not author-level.
    #[error("lockfile `{path}` failed to load: {source}")]
    Lockfile {
        /// Lockfile path that failed to load.
        path: PathBuf,
        /// Underlying lockfile error.
        #[source]
        source: LockfileError,
    },
}

impl Clone for SyncError {
    fn clone(&self) -> Self {
        // `TreeError` / `ExecError` do not implement `Clone` (they wrap
        // `std::io::Error`-adjacent values). Halts carry only a display
        // rendering in the report; we re-materialise via a synthetic
        // `Validation` variant so `SyncReport` can be `Clone`-safe for
        // observability tooling without widening the taxonomy.
        match self {
            Self::Tree(e) => Self::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<tree>".into(),
                    required: e.to_string(),
                }],
            },
            Self::Validation { errors } => Self::Validation { errors: errors.clone() },
            Self::Exec(e) => Self::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<exec>".into(),
                    required: e.to_string(),
                }],
            },
            Self::Halted(ctx) => Self::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: ctx.pack.clone(),
                    required: format!(
                        "action #{} ({}): {}",
                        ctx.action_idx, ctx.action_name, ctx.error
                    ),
                }],
            },
            Self::WorkspaceBusy { workspace, lock_path } => {
                Self::WorkspaceBusy { workspace: workspace.clone(), lock_path: lock_path.clone() }
            }
            Self::Lockfile { path, source } => Self::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<lockfile>".into(),
                    required: format!("{}: {source}", path.display()),
                }],
            },
        }
    }
}

/// Run a full sync over the pack tree rooted at `pack_root`.
///
/// Resolution rules:
/// * If `pack_root` is a directory the walker looks for
///   `<pack_root>/.grex/pack.yaml`.
/// * If `pack_root` ends in `.yaml` / `.yml` it is loaded verbatim.
/// * Workspace defaults to `<pack_root>/.grex/workspace` when `opts.workspace`
///   is `None`.
///
/// # Errors
///
/// Returns the first error that halts the pipeline — see [`SyncError`] for
/// the taxonomy.
pub fn run(pack_root: &Path, opts: &SyncOptions) -> Result<SyncReport, SyncError> {
    let workspace = resolve_workspace(pack_root, opts.workspace.as_deref());
    ensure_workspace_dir(&workspace)?;
    let (mut ws_lock, ws_lock_path) = open_workspace_lock(&workspace)?;
    let _ws_guard = match ws_lock.try_acquire() {
        Ok(Some(g)) => g,
        Ok(None) => {
            return Err(SyncError::WorkspaceBusy {
                workspace: workspace.clone(),
                lock_path: ws_lock_path,
            });
        }
        Err(e) => return Err(workspace_lock_err(&ws_lock_path, &e.to_string())),
    };

    let graph = walk_and_validate(pack_root, &workspace, opts.validate)?;

    let event_log = event_log_path(pack_root);
    let lock_path = event_lock_path(&event_log);
    let vars = VarEnv::from_os();
    let order = post_order(&graph);

    let pre_run_recovery =
        scan_recovery(&pack_root_dir(pack_root), &event_log).ok().filter(|r| !r.is_empty());
    let lockfile_path = lockfile_path(pack_root);
    let prior_lock = load_prior_lock(&lockfile_path)?;
    let registry = Arc::new(Registry::bootstrap());

    let mut report = SyncReport {
        graph,
        steps: Vec::new(),
        halted: None,
        event_log_warnings: Vec::new(),
        pre_run_recovery,
    };

    let mut next_lock = prior_lock.clone();
    run_actions(
        &mut report,
        &order,
        &vars,
        &workspace,
        &event_log,
        &lock_path,
        opts.dry_run,
        &prior_lock,
        &mut next_lock,
        &registry,
    );

    persist_lockfile_if_clean(&mut report, &lockfile_path, &next_lock, opts.dry_run);
    Ok(report)
}

/// Walk the pack tree rooted at `pack_root`, optionally running the
/// plan-phase validators. Extracted so [`run`] stays under the
/// workspace's 50-LOC per-function lint threshold.
fn walk_and_validate(
    pack_root: &Path,
    workspace: &Path,
    validate: bool,
) -> Result<PackGraph, SyncError> {
    let loader = FsPackLoader::new();
    let backend = GixBackend::new();
    let walker = Walker::new(&loader, &backend, workspace.to_path_buf());
    let graph = walker.walk(pack_root)?;
    if validate {
        validate_graph(&graph)?;
    }
    Ok(graph)
}

/// Load the prior lockfile (`grex.lock.jsonl`). Missing file yields an
/// empty map; parse errors are fatal since writes are atomic and a torn
/// lockfile therefore indicates real corruption that must be resolved
/// before a fresh sync is safe. Parse/IO failures surface as
/// [`SyncError::Lockfile`] — this is an I/O / schema fault, not a
/// dependency-satisfaction fault, so it gets its own taxonomy slot.
fn load_prior_lock(
    lockfile_path: &Path,
) -> Result<std::collections::HashMap<String, LockEntry>, SyncError> {
    read_lockfile(lockfile_path)
        .map_err(|source| SyncError::Lockfile { path: lockfile_path.to_path_buf(), source })
}

/// Persist `next_lock` atomically to `lockfile_path` whenever this was
/// not a dry-run. On a halt the map has already had the halted pack's
/// entry removed (see `run_actions`), so persisting now preserves every
/// *successful* pack's fresh entry while guaranteeing absence of an
/// entry for the halted pack — next sync sees no prior hash there and
/// re-executes from scratch (route (b) halt-state gating). Write errors
/// surface as non-fatal warnings on the report.
fn persist_lockfile_if_clean(
    report: &mut SyncReport,
    lockfile_path: &Path,
    next_lock: &std::collections::HashMap<String, LockEntry>,
    dry_run: bool,
) {
    if dry_run {
        return;
    }
    if let Err(e) = write_lockfile(lockfile_path, next_lock) {
        tracing::warn!(target: "grex::sync", "lockfile write failed: {e}");
        report.event_log_warnings.push(format!("{}: {e}", lockfile_path.display()));
    }
}

/// Canonical location of the resolved-state lockfile
/// (`<pack_root>/.grex/grex.lock.jsonl`). Colocated with the event log
/// so both audit artifacts live under a single `.grex/` sidecar.
fn lockfile_path(pack_root: &Path) -> PathBuf {
    pack_root_dir(pack_root).join(".grex").join("grex.lock.jsonl")
}

/// Create the workspace directory if it does not yet exist.
fn ensure_workspace_dir(workspace: &Path) -> Result<(), SyncError> {
    if !workspace.exists() {
        std::fs::create_dir_all(workspace).map_err(|e| SyncError::Validation {
            errors: vec![PackValidationError::DependsOnUnsatisfied {
                pack: "<workspace>".into(),
                required: format!("{}: {e}", workspace.display()),
            }],
        })?;
    }
    Ok(())
}

/// Open (but do not acquire) the workspace-level lock file.
fn open_workspace_lock(workspace: &Path) -> Result<(ScopedLock, PathBuf), SyncError> {
    let ws_lock_path = workspace_lock_path(workspace);
    let ws_lock = ScopedLock::open(&ws_lock_path)
        .map_err(|e| workspace_lock_err(&ws_lock_path, &e.to_string()))?;
    Ok((ws_lock, ws_lock_path))
}

/// Build a `Validation` error describing a workspace-lock failure.
fn workspace_lock_err(ws_lock_path: &Path, reason: &str) -> SyncError {
    SyncError::Validation {
        errors: vec![PackValidationError::DependsOnUnsatisfied {
            pack: "<workspace-lock>".into(),
            required: format!("{}: {reason}", ws_lock_path.display()),
        }],
    }
}

/// Compute the default workspace path when `override_` is absent.
fn resolve_workspace(pack_root: &Path, override_: Option<&Path>) -> PathBuf {
    if let Some(p) = override_ {
        return p.to_path_buf();
    }
    let anchor = pack_root_dir(pack_root);
    anchor.join(".grex").join("workspace")
}

/// If `pack_root` points at a yaml file, use its parent; otherwise use it.
fn pack_root_dir(pack_root: &Path) -> PathBuf {
    let is_yaml = matches!(pack_root.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"));
    if is_yaml {
        pack_root
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf)
    } else {
        pack_root.to_path_buf()
    }
}

/// Compute the `.grex/grex.jsonl` path next to the pack root.
fn event_log_path(pack_root: &Path) -> PathBuf {
    pack_root_dir(pack_root).join(".grex").join("grex.jsonl")
}

/// Compute the sidecar lock path next to the event log. One canonical slot
/// per pack root — cooperating grex procs serialize through this file.
fn event_lock_path(event_log: &Path) -> PathBuf {
    event_log.parent().map_or_else(|| PathBuf::from(".grex.lock"), |p| p.join(".grex.lock"))
}

/// Compute the sidecar lock path for the workspace itself. Lives at
/// `<workspace>/.grex.sync.lock` — the workspace dir is already created by
/// the `run()` prologue, so the lock sidecar lands beside the child clones.
fn workspace_lock_path(workspace: &Path) -> PathBuf {
    workspace.join(".grex.sync.lock")
}

/// Aggregate manifest-level + graph-level validators and return their output.
fn validate_graph(graph: &PackGraph) -> Result<(), SyncError> {
    let mut errors: Vec<PackValidationError> = Vec::new();
    for node in graph.nodes() {
        if let Err(mut e) = node.manifest.validate_plan() {
            errors.append(&mut e);
        }
    }
    if let Err(mut e) = graph.validate() {
        errors.append(&mut e);
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(SyncError::Validation { errors })
    }
}

/// Depth-first post-order traversal of the graph starting from root.
///
/// Children fully precede their parent in the returned vector so downstream
/// executors install leaves first and the root last.
fn post_order(graph: &PackGraph) -> Vec<usize> {
    let mut out = Vec::with_capacity(graph.nodes().len());
    visit_post(graph, 0, &mut out);
    out
}

fn visit_post(graph: &PackGraph, id: usize, out: &mut Vec<usize>) {
    // Collect child ids first to avoid borrow conflicts with graph iteration.
    let kids: Vec<usize> = graph.children_of(id).map(|n| n.id).collect();
    for k in kids {
        visit_post(graph, k, out);
    }
    out.push(id);
}

/// Drive every action for every node; abort on the first [`ExecError`].
///
/// Each action is bracketed by three manifest events:
/// 1. [`Event::ActionStarted`] — appended **before** `execute` returns.
/// 2. [`Event::ActionCompleted`] — appended on `Ok(step)`.
/// 3. [`Event::ActionHalted`] — appended on `Err(e)` before returning.
///
/// All three writes go through the same [`ManifestLock`]-wrapped path
/// ([`append_manifest_event`]) and failures are recorded as non-fatal
/// warnings so the executor's outcome always dominates. The third append
/// (`ActionHalted`) lets a future `grex doctor` correlate crash recovery
/// with the exact action that halted.
#[allow(clippy::too_many_arguments)]
fn run_actions(
    report: &mut SyncReport,
    order: &[usize],
    vars: &VarEnv,
    workspace: &Path,
    event_log: &Path,
    lock_path: &Path,
    dry_run: bool,
    prior_lock: &std::collections::HashMap<String, LockEntry>,
    next_lock: &mut std::collections::HashMap<String, LockEntry>,
    registry: &Arc<Registry>,
) {
    let plan = PlanExecutor::with_registry(registry.clone());
    let fs = FsExecutor::with_registry(registry.clone());
    for &id in order {
        let Some(node) = report.graph.node(id) else { continue };
        // Clone the data we need so report is borrow-free inside the loop.
        let pack_name = node.name.clone();
        let pack_path = node.path.clone();
        let actions = node.manifest.actions.clone();
        // Skip-on-hash (M4-B S1): if the prior lockfile entry's
        // `actions_hash` matches the freshly-computed hash over the
        // current action list + commit sha, emit a single
        // `ExecResult::Skipped` step for the pack and move on. The
        // force-flag override lives in M4-D; S1 assumes force is absent.
        if try_skip_pack(report, &pack_name, &pack_path, &actions, prior_lock, next_lock, dry_run) {
            continue;
        }
        let pack_halted = run_pack_actions(
            report, vars, workspace, event_log, lock_path, dry_run, &plan, &fs, &pack_name,
            &pack_path, &actions,
        );
        if pack_halted {
            // Route (b) halt-state gating: drop any prior entry for the
            // halted pack so the next sync sees no prior hash and
            // re-executes from scratch. Successful packs in this same
            // run keep their freshly-upserted entries, and packs we did
            // not reach keep their prior entries untouched.
            next_lock.remove(&pack_name);
            return;
        }
        // Successful pack — record a fresh lockfile entry so the next
        // run's skip-on-hash test can succeed. Commit SHA plumbing is a
        // follow-up (the walker does not yet expose a resolved SHA per
        // node); empty string keeps the hash stable for now. TODO(M4):
        // thread real SHA through `PackNode`.
        let actions_hash = compute_actions_hash(&actions, "");
        upsert_lock_entry(next_lock, &pack_name, &actions_hash);
    }
}

/// Per-pack execution loop. Returns `true` when the sync must halt.
#[allow(clippy::too_many_arguments)]
fn run_pack_actions(
    report: &mut SyncReport,
    vars: &VarEnv,
    workspace: &Path,
    event_log: &Path,
    lock_path: &Path,
    dry_run: bool,
    plan: &PlanExecutor,
    fs: &FsExecutor,
    pack_name: &str,
    pack_path: &Path,
    actions: &[Action],
) -> bool {
    for (idx, action) in actions.iter().enumerate() {
        let ctx = ExecCtx::new(vars, pack_path, workspace).with_platform(Platform::current());
        let action_tag = action_kind_tag(action);
        append_manifest_event(
            event_log,
            lock_path,
            &Event::ActionStarted {
                ts: Utc::now(),
                pack: pack_name.to_string(),
                action_idx: idx,
                action_name: action_tag.to_string(),
            },
            &mut report.event_log_warnings,
        );
        let step_result =
            if dry_run { plan.execute(action, &ctx) } else { fs.execute(action, &ctx) };
        if !record_action_outcome(
            report,
            event_log,
            lock_path,
            pack_name,
            idx,
            action_tag,
            step_result,
        ) {
            return true;
        }
    }
    false
}

/// Decide whether `pack_name` can be short-circuited via a lockfile
/// hash match. When the prior hash matches the freshly-computed hash,
/// emit a single [`ExecResult::Skipped`] step and carry the prior
/// lockfile entry forward unchanged. Returns `true` when the pack was
/// skipped.
#[allow(clippy::too_many_arguments)]
fn try_skip_pack(
    report: &mut SyncReport,
    pack_name: &str,
    pack_path: &Path,
    actions: &[Action],
    prior_lock: &std::collections::HashMap<String, LockEntry>,
    next_lock: &mut std::collections::HashMap<String, LockEntry>,
    dry_run: bool,
) -> bool {
    if dry_run {
        // Dry runs must always produce the planned-step transcript so
        // authors can see what `sync` *would* do; skip-on-hash only
        // applies to wet-run side-effect avoidance.
        return false;
    }
    let Some(prior) = prior_lock.get(pack_name) else {
        return false;
    };
    let hash = compute_actions_hash(actions, "");
    if prior.actions_hash != hash {
        return false;
    }
    let skipped_step = ExecStep {
        action_name: Cow::Borrowed("pack"),
        result: ExecResult::Skipped {
            pack_path: pack_path.to_path_buf(),
            actions_hash: hash.clone(),
        },
        // W4 landed `StepKind::PackSkipped` as the dedicated pack-level
        // short-circuit detail; we use it here instead of the prior
        // `Require { Satisfied, Skip }` proxy so renderers and consumers
        // can match on a single, purpose-built variant.
        details: StepKind::PackSkipped { actions_hash: hash },
    };
    report.steps.push(SyncStep {
        pack: pack_name.to_string(),
        action_idx: 0,
        exec_step: skipped_step,
    });
    // Carry the prior entry forward so the next-lock snapshot stays
    // consistent with what's on disk.
    next_lock.insert(pack_name.to_string(), prior.clone());
    true
}

/// Insert or update a lockfile entry for `pack_name` with `actions_hash`.
/// Preserves the prior entry's commit SHA / branch when present; falls
/// back to empty strings for a first-ever install. The real commit-SHA
/// thread-through lands with the walker work in a follow-up.
fn upsert_lock_entry(
    next_lock: &mut std::collections::HashMap<String, LockEntry>,
    pack_name: &str,
    actions_hash: &str,
) {
    let installed_at = Utc::now();
    let entry = next_lock.get(pack_name).map_or_else(
        || LockEntry {
            id: pack_name.to_string(),
            sha: String::new(),
            branch: String::new(),
            installed_at,
            actions_hash: actions_hash.to_string(),
            schema_version: "1".to_string(),
        },
        |prev| LockEntry { installed_at, actions_hash: actions_hash.to_string(), ..prev.clone() },
    );
    next_lock.insert(pack_name.to_string(), entry);
}

/// Record one action outcome into `report` + event log. Returns `false`
/// when the run must halt (on error); `true` otherwise.
fn record_action_outcome(
    report: &mut SyncReport,
    event_log: &Path,
    lock_path: &Path,
    pack_name: &str,
    idx: usize,
    action_tag: &'static str,
    step_result: Result<ExecStep, ExecError>,
) -> bool {
    match step_result {
        Ok(step) => {
            record_action_ok(report, event_log, lock_path, pack_name, idx, step);
            true
        }
        Err(e) => {
            record_action_err(report, event_log, lock_path, pack_name, idx, action_tag, e);
            false
        }
    }
}

/// Success-path bookkeeping: emit legacy `Sync` summary + `ActionCompleted`
/// audit event, then push the step onto the report.
fn record_action_ok(
    report: &mut SyncReport,
    event_log: &Path,
    lock_path: &Path,
    pack_name: &str,
    idx: usize,
    step: ExecStep,
) {
    append_step_event(event_log, lock_path, pack_name, &step, &mut report.event_log_warnings);
    append_manifest_event(
        event_log,
        lock_path,
        &Event::ActionCompleted {
            ts: Utc::now(),
            pack: pack_name.to_string(),
            action_idx: idx,
            result_summary: format!("{:?}", step.result),
        },
        &mut report.event_log_warnings,
    );
    report.steps.push(SyncStep { pack: pack_name.to_string(), action_idx: idx, exec_step: step });
}

/// Halt-path bookkeeping: emit `ActionHalted` audit event, then stash the
/// rich `HaltedContext` into `report.halted`.
fn record_action_err(
    report: &mut SyncReport,
    event_log: &Path,
    lock_path: &Path,
    pack_name: &str,
    idx: usize,
    action_tag: &'static str,
    e: ExecError,
) {
    let error_summary = truncate_error_summary(&e);
    append_manifest_event(
        event_log,
        lock_path,
        &Event::ActionHalted {
            ts: Utc::now(),
            pack: pack_name.to_string(),
            action_idx: idx,
            action_name: action_tag.to_string(),
            error_summary,
        },
        &mut report.event_log_warnings,
    );
    let recovery_hint = recovery_hint_for(&e);
    report.halted = Some(SyncError::Halted(Box::new(HaltedContext {
        pack: pack_name.to_string(),
        action_idx: idx,
        action_name: action_tag.to_string(),
        error: e,
        recovery_hint,
    })));
}

/// Short stable kind-tag for an [`crate::pack::Action`]. Mirrors the
/// `ACTION_*` constants used by [`crate::execute::step`] so the audit log
/// stays uniform.
fn action_kind_tag(action: &crate::pack::Action) -> &'static str {
    use crate::pack::Action;
    match action {
        Action::Symlink(_) => "symlink",
        Action::Env(_) => "env",
        Action::Mkdir(_) => "mkdir",
        Action::Rmdir(_) => "rmdir",
        Action::Require(_) => "require",
        Action::When(_) => "when",
        Action::Exec(_) => "exec",
    }
}

/// Produce a bounded human summary of an [`ExecError`] for
/// [`Event::ActionHalted::error_summary`]. Keeps the written JSONL line
/// from pathological blowup when captured stderr is large.
fn truncate_error_summary(err: &ExecError) -> String {
    let mut s = err.to_string();
    if s.len() > ACTION_ERROR_SUMMARY_MAX {
        s.truncate(ACTION_ERROR_SUMMARY_MAX);
        s.push_str("…[truncated]");
    }
    s
}

/// Best-effort recovery hint for common [`ExecError`] shapes. Returns
/// `None` when no generic advice applies; the error's own `Display`
/// output is already shown by the `Halted` variant's format string.
fn recovery_hint_for(err: &ExecError) -> Option<String> {
    match err {
        ExecError::SymlinkDestOccupied { .. } => Some(
            "set `backup: true` on the symlink action, or remove the conflicting entry by hand"
                .into(),
        ),
        ExecError::SymlinkPrivilegeDenied { .. } => {
            Some("enable Windows Developer Mode or re-run grex as administrator".into())
        }
        ExecError::SymlinkCreateAfterBackupFailed { backup, .. } => {
            Some(format!("backup left at `{}`; restore manually then re-run", backup.display()))
        }
        ExecError::RmdirNotEmpty { .. } => {
            Some("set `force: true` on the rmdir action to recurse".into())
        }
        ExecError::EnvPersistenceDenied { .. } => {
            Some("re-run elevated (Machine scope needs admin)".into())
        }
        _ => None,
    }
}

/// Append one [`Event::Sync`] record summarising an [`ExecStep`].
///
/// Failures log a warning and are recorded in the report's
/// `event_log_warnings`; they do not abort the sync (spec: event-log write
/// failures are non-fatal).
///
/// # Concurrency
///
/// The append is serialized through a [`ManifestLock`] held across the
/// write. The lock is acquired **per action** (not once across the full
/// traversal) so cooperating grex processes can observe mid-progress log
/// state between actions; fd-lock acquisition is cheap on modern kernels
/// and sync runs are dominated by executor side effects, not lock waits.
/// This closes the bypass gap surfaced by the M3 concurrency review where
/// `append_event` was called without any cross-process serialisation.
fn append_step_event(
    log: &Path,
    lock_path: &Path,
    pack: &str,
    step: &ExecStep,
    warnings: &mut Vec<String>,
) {
    let summary = format!("{}:{:?}", step.action_name, step.result);
    let event = Event::Sync { ts: Utc::now(), id: pack.to_string(), sha: summary };
    if let Err(e) = append_event_locked(log, lock_path, &event) {
        tracing::warn!(target: "grex::sync", "manifest append failed: {e}");
        warnings.push(format!("{}: {e}", log.display()));
    }
    // Schema version is recorded once at the manifest level by existing
    // manifest code; this stub uses the constant to keep a single source of
    // truth for forward-compat.
    let _ = SCHEMA_VERSION;
}

/// Append a single [`Event`] under the shared [`ManifestLock`] path.
/// Failures are logged and recorded as non-fatal warnings — the spec
/// marks event-log write failures as non-aborting so a transient disk
/// error must not kill a sync mid-stream.
fn append_manifest_event(log: &Path, lock_path: &Path, event: &Event, warnings: &mut Vec<String>) {
    if let Err(e) = append_event_locked(log, lock_path, event) {
        tracing::warn!(target: "grex::sync", "manifest append failed: {e}");
        warnings.push(format!("{}: {e}", log.display()));
    }
}

/// Acquire [`ManifestLock`] and append one event. Parent dir of the log is
/// created lazily on first write.
fn append_event_locked(log: &Path, lock_path: &Path, event: &Event) -> Result<(), String> {
    if let Some(parent) = log.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut lock = ManifestLock::open(log, lock_path).map_err(|e| e.to_string())?;
    lock.write(|| append_event(log, event)).map_err(|e| e.to_string())?.map_err(|e| e.to_string())
}

/// Re-export a cheap helper so CLI renderers can label halted steps by node
/// name without reaching into the graph twice.
#[must_use]
pub fn pack_display_name(node: &PackNode) -> &str {
    &node.name
}

/// Test-only hook: append one [`Event::Sync`] through the same
/// [`ManifestLock`]-serialised path the sync driver uses.
///
/// Exposed so integration tests under `tests/` can exercise the locked
/// append helper without spinning up a full pack tree. Not intended for
/// downstream consumers — the signature may change without notice.
#[doc(hidden)]
pub fn __test_append_sync_event(
    log: &Path,
    lock_path: &Path,
    pack: &str,
    action_name: &str,
) -> Result<(), String> {
    let event = Event::Sync { ts: Utc::now(), id: pack.to_string(), sha: action_name.to_string() };
    append_event_locked(log, lock_path, &event)
}

// ----------------------------------------------------------------------
// PR E — pre-run teardown scan
// ----------------------------------------------------------------------

/// One `ActionStarted` event in the manifest log that has no matching
/// `ActionCompleted` or `ActionHalted` peer.
///
/// Dangling starts are the primary crash signal: the process wrote the
/// pre-action event, then died before the executor returned. Callers
/// should surface these to the operator (diagnostics only this PR; a
/// future `grex doctor` verb will act on them).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DanglingStart {
    /// Pack that owned the halted action.
    pub pack: String,
    /// 0-based action index within the pack.
    pub action_idx: usize,
    /// Short action kind tag.
    pub action_name: String,
    /// Timestamp the `ActionStarted` event was written.
    pub started_at: DateTime<Utc>,
}

/// Summary of teardown artifacts found under a pack root before a sync
/// begins.
///
/// Built by [`scan_recovery`]. All fields are diagnostic; the sync
/// proceeds regardless of what the scan finds.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// `<dst>.grex.bak` files sitting next to a non-symlink or missing
    /// original (symlink-action rollback orphan).
    pub orphan_backups: Vec<PathBuf>,
    /// `<path>.grex.bak.<timestamp>` tombstones left by `rmdir` with
    /// `backup: true`.
    pub orphan_tombstones: Vec<PathBuf>,
    /// `ActionStarted` events in the log with no matching
    /// `ActionCompleted`/`ActionHalted`.
    pub dangling_starts: Vec<DanglingStart>,
}

impl RecoveryReport {
    /// `true` when the scan found nothing worth reporting.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.orphan_backups.is_empty()
            && self.orphan_tombstones.is_empty()
            && self.dangling_starts.is_empty()
    }
}

/// Walk `pack_root` and the manifest log to find crash-recovery artifacts.
///
/// Inspects:
///
/// * `<pack_root>/.grex/workspace/**` (and the pack_root itself) for
///   `.grex.bak` orphans and timestamped `.grex.bak.<ts>` tombstones.
/// * `event_log` (the manifest JSONL) for `ActionStarted` entries that
///   have no matching `ActionCompleted` / `ActionHalted` successor.
///
/// Non-blocking: scan errors are swallowed to an empty report so a
/// half-readable directory cannot kill a sync that would otherwise
/// succeed. Call sites that want to surface scan failures should read
/// the manifest directly.
///
/// # Errors
///
/// Returns [`SyncError::Validation`] only when the manifest read itself
/// reports corruption. Filesystem traversal errors are swallowed.
pub fn scan_recovery(pack_root: &Path, event_log: &Path) -> Result<RecoveryReport, SyncError> {
    let mut report = RecoveryReport::default();
    let workspace_root = pack_root.join(".grex").join("workspace");
    walk_for_backups(&workspace_root, &mut report);
    // Also scan the pack root itself — symlink destinations often live at
    // the top of the tree (e.g. `~/.config/foo`).
    walk_for_backups(pack_root, &mut report);
    if event_log.exists() {
        match read_all(event_log) {
            Ok(events) => {
                report.dangling_starts = collect_dangling_starts(&events);
            }
            Err(e) => {
                return Err(SyncError::Validation {
                    errors: vec![PackValidationError::DependsOnUnsatisfied {
                        pack: "<event-log>".into(),
                        required: e.to_string(),
                    }],
                });
            }
        }
    }
    Ok(report)
}

/// Shallow directory walker (bounded depth = 6) that categorizes
/// `.grex.bak` and `.grex.bak.<ts>` filenames into the appropriate
/// report slot. Depth-limited so a pathological workspace with a deep
/// tree cannot stall the scan; realistic layouts are well under six
/// levels.
fn walk_for_backups(root: &Path, report: &mut RecoveryReport) {
    walk_for_backups_inner(root, report, 0);
}

fn walk_for_backups_inner(dir: &Path, report: &mut RecoveryReport, depth: u32) {
    const MAX_DEPTH: u32 = 6;
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if name_str.ends_with(".grex.bak") {
            report.orphan_backups.push(path.clone());
            continue;
        }
        if let Some(rest) = name_str.rsplit_once(".grex.bak.") {
            // `rsplit_once` returns `(prefix, suffix)`; suffix is the
            // timestamp chunk. Accept any non-empty suffix — the exact
            // timestamp shape is `fs_executor` internal.
            if !rest.1.is_empty() {
                report.orphan_tombstones.push(path.clone());
                continue;
            }
        }
        // Recurse only into real directories (not symlinks, to avoid
        // traversing into the workspace's cloned repos).
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk_for_backups_inner(&path, report, depth + 1);
        }
    }
}

/// Reduce an event stream to a list of `ActionStarted` records with no
/// matching terminator.
///
/// Matching is positional per `(pack, action_idx)`: a later
/// `ActionCompleted` or `ActionHalted` with the same key clears the
/// entry. Whatever remains in the map after the pass is dangling.
fn collect_dangling_starts(events: &[Event]) -> Vec<DanglingStart> {
    use std::collections::HashMap;
    let mut open: HashMap<(String, usize), DanglingStart> = HashMap::new();
    for ev in events {
        match ev {
            Event::ActionStarted { ts, pack, action_idx, action_name } => {
                open.insert(
                    (pack.clone(), *action_idx),
                    DanglingStart {
                        pack: pack.clone(),
                        action_idx: *action_idx,
                        action_name: action_name.clone(),
                        started_at: *ts,
                    },
                );
            }
            Event::ActionCompleted { pack, action_idx, .. }
            | Event::ActionHalted { pack, action_idx, .. } => {
                open.remove(&(pack.clone(), *action_idx));
            }
            _ => {}
        }
    }
    let mut out: Vec<DanglingStart> = open.into_values().collect();
    out.sort_by_key(|a| a.started_at);
    out
}
