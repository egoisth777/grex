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

use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::execute::{
    ActionExecutor, ExecCtx, ExecError, ExecStep, FsExecutor, PlanExecutor, Platform,
};
use crate::fs::{ManifestLock, ScopedLock};
use crate::git::GixBackend;
use crate::manifest::{append_event, Event, SCHEMA_VERSION};
use crate::pack::PackValidationError;
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
    #[error("action execution failed: {0}")]
    Exec(#[from] ExecError),
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
            Self::WorkspaceBusy { workspace, lock_path } => {
                Self::WorkspaceBusy { workspace: workspace.clone(), lock_path: lock_path.clone() }
            }
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
    if !workspace.exists() {
        std::fs::create_dir_all(&workspace).map_err(|e| SyncError::Validation {
            errors: vec![PackValidationError::DependsOnUnsatisfied {
                pack: "<workspace>".into(),
                required: format!("{}: {e}", workspace.display()),
            }],
        })?;
    }

    // Workspace-level lock: prevent two concurrent `grex sync` runs from
    // clobbering each other's clones/checkouts on the same workspace. Fail
    // fast — the user almost certainly has two terminals open and needs to
    // see the collision (waiting mode is deferred to M6 concurrency work).
    let ws_lock_path = workspace_lock_path(&workspace);
    let mut ws_lock = ScopedLock::open(&ws_lock_path).map_err(|e| SyncError::Validation {
        errors: vec![PackValidationError::DependsOnUnsatisfied {
            pack: "<workspace-lock>".into(),
            required: format!("{}: {e}", ws_lock_path.display()),
        }],
    })?;
    let _ws_guard = match ws_lock.try_acquire() {
        Ok(Some(g)) => g,
        Ok(None) => {
            return Err(SyncError::WorkspaceBusy {
                workspace: workspace.clone(),
                lock_path: ws_lock_path,
            });
        }
        Err(e) => {
            return Err(SyncError::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<workspace-lock>".into(),
                    required: format!("{}: {e}", ws_lock_path.display()),
                }],
            });
        }
    };

    let loader = FsPackLoader::new();
    let backend = GixBackend::new();
    let walker = Walker::new(&loader, &backend, workspace.clone());
    let graph = walker.walk(pack_root)?;

    if opts.validate {
        validate_graph(&graph)?;
    }

    let event_log = event_log_path(pack_root);
    let lock_path = event_lock_path(&event_log);
    let vars = VarEnv::from_os();
    let order = post_order(&graph);

    let mut report =
        SyncReport { graph, steps: Vec::new(), halted: None, event_log_warnings: Vec::new() };

    run_actions(&mut report, &order, &vars, &workspace, &event_log, &lock_path, opts.dry_run);
    Ok(report)
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
fn run_actions(
    report: &mut SyncReport,
    order: &[usize],
    vars: &VarEnv,
    workspace: &Path,
    event_log: &Path,
    lock_path: &Path,
    dry_run: bool,
) {
    let plan = PlanExecutor::new();
    let fs = FsExecutor::new();
    for &id in order {
        let Some(node) = report.graph.node(id) else { continue };
        // Clone the data we need so report is borrow-free inside the loop.
        let pack_name = node.name.clone();
        let pack_path = node.path.clone();
        let actions = node.manifest.actions.clone();
        for (idx, action) in actions.iter().enumerate() {
            let ctx = ExecCtx::new(vars, &pack_path, workspace).with_platform(Platform::current());
            let step_result =
                if dry_run { plan.execute(action, &ctx) } else { fs.execute(action, &ctx) };
            match step_result {
                Ok(step) => {
                    append_step_event(
                        event_log,
                        lock_path,
                        &pack_name,
                        &step,
                        &mut report.event_log_warnings,
                    );
                    report.steps.push(SyncStep {
                        pack: pack_name.clone(),
                        action_idx: idx,
                        exec_step: step,
                    });
                }
                Err(e) => {
                    report.halted = Some(SyncError::Exec(e));
                    return;
                }
            }
        }
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
