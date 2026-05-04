//! Sync orchestrator — M3 Stage B slice 6.
//!
//! Glues the building blocks shipped in slices 1–5b into a single runnable
//! pipeline:
//!
//! 1. Walk a pack tree via [`crate::tree::sync_meta`] +
//!    [`crate::tree::build_graph`] + [`FsPackLoader`] + a `GitBackend`.
//! 2. Run plan-phase validators (manifest-level + graph-level).
//! 3. Execute every action via a pluggable [`ActionExecutor`]
//!    ([`PlanExecutor`] for dry-run, [`FsExecutor`] for wet-run).
//! 4. Record each step as an [`Event::Sync`] entry in the pack-root's
//!    `.grex/events.jsonl` event log.
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
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use globset::{Glob, GlobSet, GlobSetBuilder};
use thiserror::Error;
use tokio_util::sync::CancellationToken;

use crate::execute::{
    ActionExecutor, ExecCtx, ExecError, ExecResult, ExecStep, FsExecutor, MetaVisitedSet,
    PlanExecutor, Platform, StepKind,
};
use crate::fs::{ManifestLock, ScopedLock};
use crate::git::GixBackend;
use crate::lockfile::{
    branch_of, compute_actions_hash, read_lockfile, write_lockfile, LockEntry, LockfileError,
};
use crate::manifest::{append_event, read_all, Event, ACTION_ERROR_SUMMARY_MAX, SCHEMA_VERSION};
use crate::pack::{Action, PackValidationError};
use crate::plugin::{PackTypeRegistry, Registry};
use crate::scheduler::Scheduler;
use crate::tree::{
    build_graph, sync_meta, FsPackLoader, PackGraph, PackNode, SyncMetaOptions, TreeError,
};
use crate::vars::VarEnv;

/// Inputs to [`run`].
///
/// Fields are public-writable so call sites can construct with struct
/// literals and `..SyncOptions::default()`. Marked `#[non_exhaustive]`
/// so future knobs (parallelism, filter expressions, additional ref
/// strategies) can land without breaking library consumers who
/// constructed with explicit-literal syntax. Forces callers to use
/// struct-update syntax (`..Default::default()`).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct SyncOptions {
    /// When `true`, use [`PlanExecutor`] (no filesystem mutations).
    pub dry_run: bool,
    /// When `false`, skip plan-phase validators (manifest + graph). Debug
    /// escape hatch; production callers should leave this `true`.
    pub validate: bool,
    /// Override workspace directory. `None` → derived from `pack_root`
    /// (the directory holding `.grex/pack.yaml`).
    ///
    /// **v1.2.1 path (iii) semantics**: when `Some`, this path IS the
    /// canonical meta directory. Children resolve parent-relatively as
    /// `<workspace>/<child.path>` and `<workspace>/.grex/pack.yaml` is
    /// where the root manifest is read from. The path MUST exist;
    /// symlinks are resolved via `fs::canonicalize` to a single
    /// inode-stable form. Pre-v1.2.1 the override only re-anchored
    /// children — that legacy split is retired.
    pub workspace: Option<PathBuf>,
    /// Global ref override (`grex sync --ref <sha|branch|tag>`). When
    /// `Some`, every child pack clone/checkout uses this ref instead of
    /// the declared `child.ref`. Empty strings are rejected at the CLI
    /// layer.
    pub ref_override: Option<String>,
    /// Pack-path filter patterns (`grex sync --only <glob>`). Raw glob
    /// strings — compiled internally via an in-crate `globset` helper so the
    /// `globset` crate version does not leak into the public API.
    /// `None` / empty means every pack runs (M3 semantics). Matching is
    /// against the pack's **workspace-relative** path normalized to
    /// forward-slash form.
    pub only_patterns: Option<Vec<String>>,
    /// Bypass the lockfile hash-match skip (`grex sync --force`). When
    /// `true`, every pack re-executes even if its `actions_hash` is
    /// unchanged from the prior lockfile.
    pub force: bool,
    /// Max parallel pack ops for this sync run (feat-m6-1).
    ///
    /// * `None` → callers default to `num_cpus::get()` at CLI layer.
    ///   Library callers who construct `SyncOptions` directly and leave
    ///   this `None` get `num_cpus::get()` semantics too — the sync
    ///   driver resolves the default in one place so the scheduler slot
    ///   on every `ExecCtx` is always populated.
    /// * `Some(0)` → unbounded (`Semaphore::MAX_PERMITS`).
    /// * `Some(1)` → serial fast-path.
    /// * `Some(n >= 2)` → bounded parallel.
    pub parallel: Option<usize>,
    /// v1.2.0 Stage 1.l prep — when `true`, walker Phase 2 may drop
    /// dirty trees during prune. Still refuses ignored content unless
    /// [`SyncOptions::force_prune_with_ignored`] is also `true`.
    /// Default `false` preserves v1.1.1 behavior (refuse all dirty
    /// drops).
    pub force_prune: bool,
    /// v1.2.0 Stage 1.l prep — when `true` (implies
    /// [`SyncOptions::force_prune`]), walker Phase 2 also drops
    /// ignored content. Hard override — the strongest level. Default
    /// `false` preserves v1.1.1 behavior.
    pub force_prune_with_ignored: bool,
    /// v1.2.1 Item 5b — when `true` AND `force_prune` (or
    /// `force_prune_with_ignored`) is set, divert Phase 2 prunes
    /// through the snapshot-then-unlink quarantine pipeline. The
    /// dest's full subtree is recursively copied to
    /// `<workspace>/.grex/trash/<ISO8601>/<basename>/` BEFORE
    /// `unlink(dest)` fires. Snapshot or audit-fsync failure aborts
    /// the prune (no unlink). Lean theorem
    /// `quarantine_snapshot_precedes_delete` proves the safety
    /// contract. Default `false` preserves v1.2.0 direct-unlink
    /// behavior. Has no effect unless one of the `force_prune*`
    /// flags is also set (the CLI enforces this via
    /// `requires = "force_prune"`; library callers who set this
    /// with neither flag get a no-op since Phase 2 will not enter
    /// the override path at all).
    pub quarantine: bool,
    /// v1.2.0 Stage 1.h opt-in — when `true`, the walker rewrites a
    /// legacy v1.1.1 lockfile in place to the v1.2.0 shape. When
    /// `false` (default), the walker errors on the legacy shape so
    /// migration is always an explicit caller decision.
    pub migrate_lockfile: bool,
    /// v1.2.0 Stage 1.j prep — when `true` (default), the walker
    /// descends into nested meta-children. `doctor --shallow` flips
    /// this to `false` so only the immediate workspace is inspected.
    pub recurse: bool,
    /// v1.2.0 Stage 1.j prep — pairs with
    /// [`SyncOptions::recurse`] for `--shallow=N`. `None` (default)
    /// is unbounded recursion when `recurse` is `true`. `Some(n)`
    /// caps depth at `n` levels of nesting.
    pub max_depth: Option<usize>,
    /// v1.2.5 — when `Some(N)`, every meta sync starts with a
    /// best-effort GC sweep over `<meta>/.grex/trash/`, deleting
    /// entries older than `N` days. `None` (default) preserves the
    /// v1.2.1 indefinite-retention behavior. The CLI surfaces this
    /// as `grex sync --retain-days N`; library callers wire it via
    /// [`SyncOptions::with_retain_days`]. Sweep failures log via
    /// `tracing::warn!` and DO NOT halt the sync.
    pub retain_days: Option<u32>,
}

impl Default for SyncOptions {
    fn default() -> Self {
        Self {
            dry_run: false,
            validate: true,
            workspace: None,
            ref_override: None,
            only_patterns: None,
            force: false,
            parallel: None,
            // v1.2.0 Stage 1.m additions — defaults preserve v1.1.1
            // behavior. Each field is a dormant placeholder until
            // its corresponding walker stage wires it.
            force_prune: false,
            force_prune_with_ignored: false,
            quarantine: false,
            migrate_lockfile: false,
            recurse: true,
            max_depth: None,
            retain_days: None,
        }
    }
}

/// Compile raw `--only` pattern strings into a [`globset::GlobSet`].
/// Empty / absent input yields `Ok(None)` so M3's zero-config path
/// (every pack runs) stays the default.
fn compile_only_globset(patterns: Option<&Vec<String>>) -> Result<Option<GlobSet>, SyncError> {
    let Some(pats) = patterns else { return Ok(None) };
    if pats.is_empty() {
        return Ok(None);
    }
    let mut builder = GlobSetBuilder::new();
    for p in pats {
        let glob = Glob::new(p)
            .map_err(|source| SyncError::InvalidOnlyGlob { pattern: p.clone(), source })?;
        builder.add(glob);
    }
    let set = builder
        .build()
        .map_err(|source| SyncError::InvalidOnlyGlob { pattern: pats.join(","), source })?;
    Ok(Some(set))
}

impl SyncOptions {
    /// Default options: wet-run, validators enabled, default workspace path.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Set `dry_run`.
    #[must_use]
    pub fn with_dry_run(mut self, dry_run: bool) -> Self {
        self.dry_run = dry_run;
        self
    }

    /// Set `validate`.
    #[must_use]
    pub fn with_validate(mut self, validate: bool) -> Self {
        self.validate = validate;
        self
    }

    /// Set `workspace` override.
    #[must_use]
    pub fn with_workspace(mut self, workspace: Option<PathBuf>) -> Self {
        self.workspace = workspace;
        self
    }

    /// Set `ref_override` (`--ref`).
    #[must_use]
    pub fn with_ref_override(mut self, ref_override: Option<String>) -> Self {
        self.ref_override = ref_override;
        self
    }

    /// Set `only_patterns` (`--only`). Empty vector or `None` disables
    /// the filter.
    #[must_use]
    pub fn with_only_patterns(mut self, patterns: Option<Vec<String>>) -> Self {
        self.only_patterns = patterns;
        self
    }

    /// Set `force` (`--force`).
    #[must_use]
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Set `parallel` (`--parallel`). See [`SyncOptions::parallel`] for
    /// the `None` / `Some(0)` / `Some(1)` / `Some(n)` semantics.
    #[must_use]
    pub fn with_parallel(mut self, parallel: Option<usize>) -> Self {
        self.parallel = parallel;
        self
    }

    /// Set `force_prune` (`--force-prune`). See
    /// [`SyncOptions::force_prune`] for the override matrix.
    #[must_use]
    pub fn with_force_prune(mut self, force_prune: bool) -> Self {
        self.force_prune = force_prune;
        self
    }

    /// Set `force_prune_with_ignored` (`--force-prune-with-ignored`).
    /// See [`SyncOptions::force_prune_with_ignored`] for the override
    /// matrix.
    #[must_use]
    pub fn with_force_prune_with_ignored(mut self, force_prune_with_ignored: bool) -> Self {
        self.force_prune_with_ignored = force_prune_with_ignored;
        self
    }

    /// Set `quarantine` (`--quarantine`). See
    /// [`SyncOptions::quarantine`] for the snapshot-before-delete
    /// contract. Has no effect unless [`SyncOptions::force_prune`]
    /// or [`SyncOptions::force_prune_with_ignored`] is also set.
    #[must_use]
    pub fn with_quarantine(mut self, quarantine: bool) -> Self {
        self.quarantine = quarantine;
        self
    }

    /// Set `retain_days` (`--retain-days N`). See
    /// [`SyncOptions::retain_days`] for the GC-sweep contract.
    /// `None` preserves v1.2.1 indefinite-retention behavior;
    /// `Some(N)` triggers a best-effort sweep at the start of every
    /// meta sync.
    #[must_use]
    pub fn with_retain_days(mut self, retain_days: Option<u32>) -> Self {
        self.retain_days = retain_days;
        self
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
    /// One entry per child whose legacy `.grex/workspace/<name>/` layout
    /// was relocated (or considered for relocation) on this sync. Empty
    /// when no legacy directory was found — the common case for any
    /// workspace built fresh on v1.1.0+. CLI renderers should surface
    /// the list so operators see what changed.
    pub workspace_migrations: Vec<WorkspaceMigration>,
}

/// One legacy-layout migration attempt. `outcome` distinguishes the
/// move-succeeded case from the don't-clobber-user-data case so CLI
/// renderers can present different advice to the operator.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMigration {
    /// Source path under the legacy `.grex/workspace/<name>/` location,
    /// rendered relative to the pack root for log readability.
    pub from: PathBuf,
    /// Destination flat-sibling path `<pack_root>/<name>/`, relative to
    /// the pack root.
    pub to: PathBuf,
    /// What happened.
    pub outcome: MigrationOutcome,
}

/// Outcome of one legacy-layout migration attempt.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// Legacy directory was renamed onto the flat-sibling slot.
    Migrated,
    /// Both legacy and flat-sibling slots existed. Skipped — the user
    /// must inspect and reconcile manually so we never silently delete
    /// either.
    SkippedBothExist,
    /// Flat-sibling slot already had a non-grex file or directory in
    /// the way. Skipped — refusing to clobber user data even when the
    /// legacy slot is plainly the source of truth.
    SkippedDestOccupied,
    /// `fs::rename` failed (e.g. cross-volume, ACL denied). The legacy
    /// directory is still in place; surfaced so the operator can move
    /// it manually.
    Failed { error: String },
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
    /// One of the `--only <GLOB>` patterns failed to compile. Surfaced
    /// as its own variant so the CLI can map it to a dedicated usage
    /// error exit code instead of the generic sync-failure bucket.
    #[error("invalid --only glob `{pattern}`: {source}")]
    InvalidOnlyGlob {
        /// The raw pattern string that failed to compile.
        pattern: String,
        /// Underlying globset error.
        #[source]
        source: globset::Error,
    },
    /// Migrating the v1.x event log (`grex.jsonl`) to the v2 canonical
    /// path (`.grex/events.jsonl`) failed. Operator-level resolution
    /// (check filesystem permissions, free disk space, then retry).
    #[error("event-log migration failed: {0}")]
    EventLogMigration(#[source] crate::manifest::ManifestError),
    /// Cooperative cancellation fired (Ctrl-C / SIGTERM) during a
    /// parallel sync. v1.2.0 Stage 1.g wires the rayon walker to surface
    /// this distinct-from-failure variant so the CLI can exit with a
    /// dedicated cancellation code instead of a generic sync error.
    /// Dormant until Stage 1.g — the existing CLI does not yet emit it.
    #[error("sync cancelled by user")]
    SchedulerCancelled,
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
            Self::InvalidOnlyGlob { pattern, source } => Self::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<only-glob>".into(),
                    required: format!("{pattern}: {source}"),
                }],
            },
            Self::EventLogMigration(source) => Self::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<event-log-migration>".into(),
                    required: source.to_string(),
                }],
            },
            Self::SchedulerCancelled => Self::SchedulerCancelled,
        }
    }
}

/// Run a full sync over the pack tree rooted at `pack_root`.
///
/// Resolution rules:
/// * If `pack_root` is a directory the walker looks for
///   `<pack_root>/.grex/pack.yaml`.
/// * If `pack_root` ends in `.yaml` / `.yml` it is loaded verbatim.
/// * Workspace defaults to the pack root directory itself when
///   `opts.workspace` is `None`. Children resolve as flat siblings of the
///   parent pack root (since v1.1.0).
///
/// # Errors
///
/// Returns the first error that halts the pipeline — see [`SyncError`] for
/// the taxonomy.
///
/// `cancel` is the cooperative cancellation handle threaded through the
/// pipeline by feat-m7-1 stage 2. Stage 2 only wires the parameter; the
/// `is_cancelled()` polls land in stages 3-4 (scheduler + pack-lock
/// acquire). CLI callers pass a never-cancelled sentinel
/// (`CancellationToken::new()`); the MCP server passes a token tied to
/// the request lifetime.
pub fn run(
    pack_root: &Path,
    opts: &SyncOptions,
    cancel: &CancellationToken,
) -> Result<SyncReport, SyncError> {
    // Stage 2 is signature-only — silence "unused parameter" without
    // hiding it behind `_` (downstream stages will read it).
    let _ = cancel;
    let workspace = prepare_workspace(pack_root, opts)?;
    // v1.3.1 (B4) — `dry_run = true` is contractually FS-mutation-free.
    // `open_workspace_lock` (via `ScopedLock::open`) creates a sidecar
    // file at `<workspace>/.grex.sync.lock`, which would itself violate
    // the no-FS-mutation contract. Skip lock acquisition entirely in
    // dry-run; the dry-run path is read-only by construction so
    // concurrent dry-runs against the same workspace are safe.
    let mut ws_lock_holder =
        if !opts.dry_run { Some(open_workspace_lock(&workspace)?) } else { None };
    let _ws_guard = try_acquire_workspace_guard(ws_lock_holder.as_mut(), &workspace)?;

    // Compile `--only` patterns into a GlobSet here so the
    // `globset` crate version does not leak into `SyncOptions`.
    let only_set = compile_only_globset(opts.only_patterns.as_ref())?;

    // Auto-migrate legacy `.grex/workspace/<name>/` layout BEFORE the
    // walker resolves children. Idempotent: a fresh v1.1.0+ workspace
    // sees no legacy directory and the function no-ops.
    let workspace_migrations = migrate_legacy_workspace(pack_root);

    // v1.2.1 path (iii) — three-stage composition:
    //   sync_meta(workspace, prune_candidates) — mutate (rayon parallel)
    //   build_graph(workspace)                 — read-only graph
    //   run_actions(graph)                     — consume graph
    // `Walker::walk` is retired from the prod path; the symbol is kept
    // for test-suite compat. See `crates/grex-core/src/tree/graph_build.rs`.
    run_sync_meta(&workspace, opts)?;
    let graph = build_and_validate_graph(&workspace, opts.validate, opts.ref_override.as_deref())?;
    let prep = prepare_run_context(pack_root, &graph, &workspace)?;
    log_force_flag(opts.force);

    let mut report = SyncReport {
        graph,
        steps: Vec::new(),
        halted: None,
        event_log_warnings: Vec::new(),
        pre_run_recovery: prep.pre_run_recovery,
        workspace_migrations,
    };

    let mut next_lock = prep.prior_lock.clone();
    // feat-m6 B1: resolve `--parallel` once and build the scheduler
    // shared across every `ExecCtx` in this run. Library callers who
    // leave `opts.parallel == None` default to `num_cpus::get()` here
    // (clamped `>= 1`) so the scheduler slot is always populated —
    // `ctx.scheduler` being `None` would strand acquire-sites into
    // unbounded concurrency. See `.omne/concurrency.md` §Scheduler.
    let resolved_parallel: usize = opts.parallel.unwrap_or_else(|| num_cpus::get().max(1));
    let scheduler = Arc::new(Scheduler::new(resolved_parallel));
    run_actions(
        &mut report,
        &prep.order,
        &prep.vars,
        &workspace,
        &prep.event_log,
        &prep.lock_path,
        opts.dry_run,
        &prep.prior_lock,
        &mut next_lock,
        &prep.registry,
        &prep.pack_type_registry,
        only_set.as_ref(),
        opts.force,
        resolved_parallel,
        &scheduler,
    );

    persist_lockfile_if_clean(&mut report, &prep.lockfile_path, &next_lock, opts.dry_run);
    Ok(report)
}

/// Bag of context pieces assembled once at the top of [`run`]. Grouping
/// them keeps [`run`] under the workspace's 50-LOC function lint without
/// smearing the read of sequential setup across helpers. Fields are
/// consumed piecemeal by the actions loop; no getters needed.
struct RunContext {
    order: Vec<usize>,
    vars: VarEnv,
    event_log: PathBuf,
    lock_path: PathBuf,
    lockfile_path: PathBuf,
    prior_lock: std::collections::HashMap<String, LockEntry>,
    registry: Arc<Registry>,
    pack_type_registry: Arc<PackTypeRegistry>,
    pre_run_recovery: Option<RecoveryReport>,
}

/// Build the per-run context: traversal order, vars env, event/lockfile
/// paths, prior lockfile state, bootstrap registry, and (optionally) a
/// pre-run recovery scan. Kept narrow so [`run`] stays small.
///
/// `workspace` is the resolved workspace directory (post `--workspace`
/// override) so the recovery scan looks for `.grex.bak` artefacts under
/// the actual on-disk location children were materialised at — not
/// under the pack root, which differs from the workspace whenever the
/// CLI's `--workspace` flag is used. Pre-fix this anchoring drift
/// caused recovery scans to miss every backup left under an override
/// workspace.
fn prepare_run_context(
    pack_root: &Path,
    graph: &PackGraph,
    workspace: &Path,
) -> Result<RunContext, SyncError> {
    let event_log = event_log_path(pack_root);
    let lock_path = event_lock_path(&event_log);
    let vars = VarEnv::from_os();
    let order = post_order(graph);
    let pre_run_recovery = scan_recovery(workspace, &event_log).ok().filter(|r| !r.is_empty());
    let lockfile_path = lockfile_path(pack_root);
    let prior_lock = load_prior_lock(&lockfile_path)?;
    let registry = Arc::new(Registry::bootstrap());
    let pack_type_registry = Arc::new(bootstrap_pack_type_registry());
    Ok(RunContext {
        order,
        vars,
        event_log,
        lock_path,
        lockfile_path,
        prior_lock,
        registry,
        pack_type_registry,
        pre_run_recovery,
    })
}

/// Build the [`PackTypeRegistry`] the sync driver threads into every
/// [`ExecCtx`] it constructs.
///
/// Default path (no `plugin-inventory` feature) hard-codes the three
/// built-ins via [`PackTypeRegistry::bootstrap`]. With the feature on,
/// [`PackTypeRegistry::bootstrap_from_inventory`] is preferred so any
/// externally-submitted plugin types (mirroring the M4-E pattern for
/// action plugins) shadow the built-ins last-writer-wins. Kept as a free
/// helper so the `#[cfg]` split lives in one place instead of being
/// smeared across every sync call-site.
fn bootstrap_pack_type_registry() -> PackTypeRegistry {
    #[cfg(feature = "plugin-inventory")]
    {
        let mut reg = PackTypeRegistry::bootstrap();
        reg.register_from_inventory();
        reg
    }
    #[cfg(not(feature = "plugin-inventory"))]
    {
        PackTypeRegistry::bootstrap()
    }
}

/// Emit a single `tracing::info!` line when `--force` is active so
/// operators can confirm from logs that the skip short-circuit was
/// bypassed. Extracted so [`run`] stays small.
fn log_force_flag(force: bool) {
    if force {
        tracing::info!(
            target: "grex::sync",
            "--force active: bypassing lockfile skip-on-hash short-circuit"
        );
    }
}

/// v1.2.1 path (iii) — drive the v1.2.0 [`sync_meta`] walker over the
/// resolved canonical workspace.
///
/// This is the SOLE mutating pass in `sync::run`: clones, fetches,
/// prune dispatches, distributed-lockfile reads, and TOCTOU `BoundedDir`
/// opens all happen here. The subsequent [`build_and_validate_graph`]
/// pass is read-only against the disk state this fn leaves behind.
///
/// `prune_candidates` is computed from the per-meta lockfile orphans:
/// every entry in `<workspace>/.grex/grex.lock.jsonl` whose `path` no
/// longer appears in the live root manifest's `children[]` is fed into
/// Phase 2 for dispatch (with `--force-prune` / `--force-prune-with-ignored`
/// overrides honoured by the consent walk). This closes the
/// "prune-inert" gap from the previous wiring, where `sync::run` passed
/// `&[]` and `--force-prune` was a CLI flag with no behavioural reach.
///
/// `--workspace` semantics: the canonical `workspace` argument is what
/// `sync_meta` uses as its `meta_dir`. Children land at
/// `<workspace>/<child.path>` — the v1.2.0 parent-relative model. Prior
/// to v1.2.1, callers passing `--workspace` skipped the precursor
/// entirely; that bypass is retired here so override callers see the
/// same v1.2.0 semantics as the default-cwd path.
///
/// `SyncOptions::parallel` mapping (mirrors [`SyncMetaOptions::parallel`]
/// with the documented `Some(0)` carve-out):
/// * `None` → `SyncMetaOptions::parallel = None` (rayon default =
///   `num_cpus::get()`).
/// * `Some(0)` → `SyncMetaOptions::parallel = None` (the CLI sentinel
///   for "unbounded" maps to rayon's default; `Some(0)` would be
///   clamped to `1` inside `build_pool`, which is not what callers
///   asking for unbounded want).
/// * `Some(n)` for `n >= 1` → `SyncMetaOptions::parallel = Some(n)`.
fn run_sync_meta(workspace: &Path, opts: &SyncOptions) -> Result<(), SyncError> {
    let loader = FsPackLoader::new();
    let backend = GixBackend::new();
    let parallel = match opts.parallel {
        None | Some(0) => None,
        Some(n) => Some(n),
    };
    // v1.2.1 Item 5b — resolve the quarantine config relative to the
    // canonical workspace (the same `meta_dir` `sync_meta` runs on).
    // Trash bucket lives at `<workspace>/.grex/trash/`; audit log at
    // `<workspace>/.grex/events.jsonl` — same path the existing
    // `ForcePruneExecuted` event uses.
    let quarantine = opts.quarantine.then(|| crate::tree::QuarantineConfig {
        trash_root: workspace.join(".grex").join("trash"),
        audit_log: crate::manifest::event_log_path(workspace),
    });
    // v1.2.5 — thread `--retain-days N` into the per-meta options so
    // every recursion frame swept its own trash bucket. `None` skips
    // the GC entirely (v1.2.1 indefinite-retention).
    let retention =
        opts.retain_days.map(|retain_days| crate::tree::RetentionConfig { retain_days });
    let meta_opts = SyncMetaOptions {
        ref_override: opts.ref_override.clone(),
        recurse: opts.recurse,
        max_depth: opts.max_depth,
        force_prune: opts.force_prune,
        force_prune_with_ignored: opts.force_prune_with_ignored,
        parallel,
        quarantine,
        retention,
        // v1.3.1 (B4) — propagate the orchestrator's dry-run flag into
        // the walker so Phase 1 skips clone/fetch and emits the
        // would-clone records into `SyncMetaReport::dry_run_would_clone`
        // instead. The orchestrator already gates lockfile persist via
        // `persist_lockfile_if_clean`; this wires the walker side.
        dry_run: opts.dry_run,
    };
    let prune_candidates = compute_prune_candidates(workspace, &loader);
    let report = sync_meta(workspace, &backend, &loader, &meta_opts, &prune_candidates)?;
    if let Some(first) = report.errors.into_iter().next() {
        return Err(SyncError::Tree(first));
    }
    Ok(())
}

/// v1.2.1 path (iii) — orphan-prune candidate computation.
///
/// Reads `<workspace>/.grex/grex.lock.jsonl` and the root manifest;
/// returns every lockfile entry path that no longer matches a declared
/// child in `manifest.children`. Empty in three cases:
///
/// * No lockfile (fresh workspace, never synced).
/// * No manifest at `<workspace>/.grex/pack.yaml` (single-node tree —
///   `sync_meta` will surface its own diagnostic).
/// * Lockfile entries are all still declared (steady-state sync).
///
/// Lockfile read errors are tolerated as `Vec::new()`: the prune pass
/// is opportunistic, and a corrupt lockfile is the migrator's concern,
/// not the prune dispatcher's. Manifest read errors are similarly
/// tolerated — `sync_meta` will fail loudly on the same condition,
/// giving the operator a single unambiguous error surface.
fn compute_prune_candidates(
    workspace: &Path,
    loader: &dyn crate::tree::PackLoader,
) -> Vec<PathBuf> {
    use crate::lockfile::read_meta_lockfile;
    let entries = match read_meta_lockfile(workspace) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    if entries.is_empty() {
        return Vec::new();
    }
    let manifest = match loader.load(workspace) {
        Ok(m) => m,
        Err(_) => return Vec::new(),
    };
    let declared: std::collections::HashSet<String> =
        manifest.children.iter().map(crate::pack::ChildRef::effective_path).collect();
    entries
        .into_iter()
        .filter(|e| !declared.contains(&e.path))
        .map(|e| PathBuf::from(e.path))
        .collect()
}

/// v1.2.1 path (iii) — read-only graph build + plan-phase validation.
///
/// Builds the [`PackGraph`] from the on-disk meta tree rooted at
/// `workspace`. Replaces the legacy `walk_and_validate` (which used
/// [`crate::tree::Walker::walk`] and re-issued every clone/fetch as a
/// no-op probe) with the v1.2.1 split:
///
/// * The mutating half ran in [`run_sync_meta`] — all clones, fetches,
///   prune dispatches, and TOCTOU `BoundedDir` opens already happened.
/// * THIS pass is strictly READ-ONLY. It walks the manifest tree
///   parent-relatively (matching what `sync_meta` placed on disk),
///   loads each child's `pack.yaml` (or synthesises a plain-git leaf),
///   probes `head_sha`, and produces the [`PackGraph`] consumed by
///   [`run_actions`].
///
/// Plan-phase validators run against the assembled graph when
/// `validate` is true.
fn build_and_validate_graph(
    workspace: &Path,
    validate: bool,
    ref_override: Option<&str>,
) -> Result<PackGraph, SyncError> {
    let loader = FsPackLoader::new();
    let backend = GixBackend::new();
    let graph = build_graph(workspace, &backend, &loader, ref_override)?;
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

/// Try-acquire the workspace lock guard when the holder is `Some`.
/// Returns `Ok(None)` when the holder is `None` (e.g. dry-run path skips
/// lock acquisition entirely; see Blocker B4 v1.3.1). Translates the
/// busy/error outcomes into the shared [`SyncError`] taxonomy. Extracted
/// from [`run`] / [`teardown`] to keep both verb entry-points under the
/// `clippy::too-many-lines` limit while preserving the original lock
/// semantics.
fn try_acquire_workspace_guard<'a>(
    holder: Option<&'a mut (ScopedLock, PathBuf)>,
    workspace: &Path,
) -> Result<Option<fd_lock::RwLockWriteGuard<'a, std::fs::File>>, SyncError> {
    let Some((ws_lock, ws_lock_path)) = holder else {
        return Ok(None);
    };
    match ws_lock.try_acquire() {
        Ok(Some(g)) => Ok(Some(g)),
        Ok(None) => Err(SyncError::WorkspaceBusy {
            workspace: workspace.to_path_buf(),
            lock_path: ws_lock_path.clone(),
        }),
        Err(e) => Err(workspace_lock_err(ws_lock_path, &e.to_string())),
    }
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

/// Single source of truth for the legacy workspace directory name.
/// Pre-`v1.1.0` `resolve_workspace` joined `.grex/workspace/` onto the
/// pack root by default; the auto-migration in
/// [`migrate_legacy_workspace`] is the only place that legacy literal
/// is allowed to appear in `crates/grex-core/src/`. The grep gate in
/// the v1.1.0 release checklist allows this one constant.
const LEGACY_WORKSPACE_DIR: &str = ".grex/workspace";

/// Auto-migrate any legacy `.grex/workspace/<name>/` child layout left
/// over from v1.0.x to the v1.1.0 flat-sibling layout. Idempotent: a
/// fresh workspace built on v1.1.0+ sees no `.grex/workspace/`
/// directory and the function no-ops.
///
/// Per-child outcomes:
///
/// * **Both legacy + flat-sibling exist** → `SkippedBothExist`. The
///   user needs to inspect (perhaps the legacy is stale, perhaps it is
///   the source of truth); we never silently delete either.
/// * **Flat-sibling slot occupied by a non-grex file or non-empty dir**
///   → `SkippedDestOccupied`. Refuse to clobber user data.
/// * **Legacy exists, flat-sibling absent** → `Migrated` via atomic
///   `fs::rename`. Same-volume move is the common case (the migration
///   stays inside `pack_root`); cross-volume failures surface as
///   `Failed { error }` with the OS message so the operator can move
///   manually.
/// * **Legacy absent** → silent no-op (not recorded in the report).
///
/// After all per-child decisions: orphan `.grex.sync.lock` under the
/// legacy workspace is removed (best-effort) and the empty
/// `.grex/workspace/` directory is rmdir'd (best-effort). Both are
/// soft-failures: leaving them on disk is harmless, surfacing the
/// errors as a sync abort would be over-strict.
///
/// Discovery is by directory listing, not by parent-manifest parse —
/// migration must work even when the parent manifest itself was
/// rewritten between versions. A child counts as "legacy" iff
/// `<pack_root>/<LEGACY_WORKSPACE_DIR>/<name>/.git` exists (i.e. it is
/// an actual git working tree, not stray metadata).
fn migrate_legacy_workspace(pack_root: &Path) -> Vec<WorkspaceMigration> {
    let root = pack_root_dir(pack_root);
    let legacy_root = root.join(LEGACY_WORKSPACE_DIR);
    if !legacy_root.is_dir() {
        return Vec::new();
    }
    let entries = match fs::read_dir(&legacy_root) {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!(
                target: "grex::sync::migrate",
                "legacy workspace `{}` unreadable: {e}",
                legacy_root.display(),
            );
            return Vec::new();
        }
    };
    let mut migrations = Vec::new();
    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    target: "grex::sync::migrate",
                    "skipping unreadable entry under `{}`: {e}",
                    legacy_root.display(),
                );
                continue;
            }
        };
        let Ok(ft) = entry.file_type() else { continue };
        // file_type avoids symlink-following; legitimate v1.0.x children
        // were always real directories, so anything else is skipped.
        if ft.is_symlink() || !ft.is_dir() {
            continue;
        }
        let name_os = entry.file_name();
        let Some(name) = name_os.to_str() else { continue };
        // Only act on entries that look like real cloned children (have
        // a `.git`). The legacy workspace lock file (`.grex.sync.lock`)
        // is not a directory and is filtered out by the dir check above;
        // we clean it up explicitly after the migration loop completes.
        let from_abs = entry.path();
        if !from_abs.join(".git").exists() {
            continue;
        }
        let to_abs = root.join(name);
        let from_rel = PathBuf::from(LEGACY_WORKSPACE_DIR).join(name);
        let to_rel = PathBuf::from(name);
        let outcome = decide_and_migrate(&from_abs, &to_abs);
        log_migration(&from_rel, &to_rel, &outcome);
        migrations.push(WorkspaceMigration { from: from_rel, to: to_rel, outcome });
    }
    cleanup_legacy_workspace_root(&legacy_root);
    migrations
}

/// Decide what to do with one legacy child + perform the move when
/// safe. Returns the outcome to record on the [`WorkspaceMigration`].
fn decide_and_migrate(from: &Path, to: &Path) -> MigrationOutcome {
    let dest_exists = to.exists();
    let dest_is_grex_repo = dest_exists && to.join(".git").exists();
    if dest_is_grex_repo {
        // Both legacy and flat-sibling are git repos. Refuse to choose
        // between them; let the user resolve.
        return MigrationOutcome::SkippedBothExist;
    }
    if dest_exists {
        // Some other entry occupies the flat-sibling slot — a stray
        // file, an empty dir, an unrelated dir. Treat as user data and
        // leave both in place.
        return MigrationOutcome::SkippedDestOccupied;
    }
    match fs::rename(from, to) {
        Ok(()) => MigrationOutcome::Migrated,
        Err(e) => MigrationOutcome::Failed { error: e.to_string() },
    }
}

/// Emit one structured log line per migration so users see exactly what
/// happened during the upgrade. Severity matches outcome: success is
/// `info`, skips and failures are `warn` so they surface in the default
/// log level without forcing operators to crank verbosity.
fn log_migration(from: &Path, to: &Path, outcome: &MigrationOutcome) {
    let from_disp = from.display();
    let to_disp = to.display();
    match outcome {
        MigrationOutcome::Migrated => {
            tracing::info!(
                target: "grex::sync::migrate",
                "migrated: legacy={from_disp} -> new={to_disp}",
            );
        }
        MigrationOutcome::SkippedBothExist => {
            tracing::warn!(
                target: "grex::sync::migrate",
                "skipped: both legacy={from_disp} and new={to_disp} exist; resolve manually",
            );
        }
        MigrationOutcome::SkippedDestOccupied => {
            tracing::warn!(
                target: "grex::sync::migrate",
                "skipped: destination={to_disp} occupied; leaving legacy={from_disp} in place",
            );
        }
        MigrationOutcome::Failed { error } => {
            tracing::warn!(
                target: "grex::sync::migrate",
                "failed: legacy={from_disp} -> new={to_disp}: {error}",
            );
        }
    }
}

/// Best-effort cleanup of the legacy workspace root after migration:
/// remove the orphan `.grex.sync.lock` (always safe — the v1.1.0
/// workspace lock lives at `<pack_root>/.grex.sync.lock`) and try to
/// rmdir the now-empty `.grex/workspace/` directory. Errors are logged
/// at trace level only — both leftovers are harmless.
fn cleanup_legacy_workspace_root(legacy_root: &Path) {
    let orphan_lock = legacy_root.join(".grex.sync.lock");
    if orphan_lock.exists() {
        if let Err(e) = fs::remove_file(&orphan_lock) {
            tracing::warn!(
                target: "grex::sync::migrate",
                "could not remove orphan lock `{}`: {e}",
                orphan_lock.display(),
            );
        } else {
            tracing::info!(
                target: "grex::sync::migrate",
                "removed orphan lock `{}`",
                orphan_lock.display(),
            );
        }
    }
    // `remove_dir` only succeeds when the directory is empty — exactly
    // what we want; if any unmigrated child remains, the legacy root
    // stays put for the operator to inspect.
    let _ = fs::remove_dir(legacy_root);
}

/// Compute the default workspace path when `override_` is absent.
///
/// The default is the pack root directory itself, so child packs
/// resolve as flat siblings of the parent pack root. The rationale —
/// alignment with the long-standing pack-spec rule that
/// `children[].path` is a bare name — lives in the pack-spec
/// "Validation rules" section (`man/concepts/pack-spec.md` /
/// `grex-doc/src/concepts/pack-spec.md`).
/// v1.2.1 path (iii) — resolve the workspace anchor with canonical
/// symlink resolution.
///
/// Resolution rules:
/// * `override_ = None` ⇒ derive workspace from `pack_root_dir(pack_root)`.
///   No canonicalize on this branch — the pack-root path was supplied
///   directly by the caller and may legitimately reference a not-yet-real
///   directory (e.g. integration fixtures that lazily materialise the
///   pack root).
/// * `override_ = Some(path)`:
///   1. **Must-exist** check. A `--workspace` override pointing at a
///      non-existent directory is a fail-fast error (we won't silently
///      `mkdir -p` someone else's typo).
///   2. **Canonicalise.** Resolve symlinks to a real path. This is the
///      anchor every downstream pass (`sync_meta`, `build_graph`, the
///      lockfile reads, the TOCTOU `BoundedDir` opens) hangs off — they
///      MUST agree on a single inode-stable string.
///   3. **Log when input != canonical.** Surfaces symlink resolution to
///      operators so they can correlate workspace-busy diagnostics with
///      what the OS actually opened.
fn resolve_workspace(pack_root: &Path, override_: Option<&Path>) -> Result<PathBuf, SyncError> {
    let Some(input) = override_ else {
        return Ok(pack_root_dir(pack_root));
    };
    if !input.exists() {
        return Err(SyncError::Validation {
            errors: vec![PackValidationError::DependsOnUnsatisfied {
                pack: "<workspace>".into(),
                required: format!("--workspace {}: directory does not exist", input.display()),
            }],
        });
    }
    let canonical = match input.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            return Err(SyncError::Validation {
                errors: vec![PackValidationError::DependsOnUnsatisfied {
                    pack: "<workspace>".into(),
                    required: format!("--workspace {}: canonicalize failed: {e}", input.display()),
                }],
            });
        }
    };
    if canonical != input {
        tracing::info!(
            target: "grex::sync",
            "workspace: {} → {}",
            input.display(),
            canonical.display(),
        );
    }
    Ok(canonical)
}

/// Resolve the workspace, ensure the directory exists, and run the v1→v2
/// event-log migration. Extracted so [`run`] and [`teardown`] stay under
/// the workspace's 50-LOC per-function lint threshold.
fn prepare_workspace(pack_root: &Path, opts: &SyncOptions) -> Result<PathBuf, SyncError> {
    let workspace = resolve_workspace(pack_root, opts.workspace.as_deref())?;
    ensure_workspace_dir(&workspace)?;
    crate::manifest::ensure_event_log_migrated(&workspace).map_err(SyncError::EventLogMigration)?;
    Ok(workspace)
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

/// Compute the `.grex/events.jsonl` path next to the pack root.
///
/// Delegates to [`crate::manifest::event_log_path`] (single source of
/// truth for the canonical event-log location).
fn event_log_path(pack_root: &Path) -> PathBuf {
    crate::manifest::event_log_path(&pack_root_dir(pack_root))
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
// feat-m6 B1 wiring added `parallel` + `scheduler` args; the signature
// now pushes past the 50-LOC per-function lint by one line. Silence
// that one — the body itself is unchanged in scope.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
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
    pack_type_registry: &Arc<PackTypeRegistry>,
    only: Option<&GlobSet>,
    force: bool,
    parallel: usize,
    scheduler: &Arc<Scheduler>,
) {
    let plan = PlanExecutor::with_registry(registry.clone());
    let fs = FsExecutor::with_registry(registry.clone());
    let rt = build_pack_type_runtime(parallel);
    let visited_meta = new_visited_meta();
    for &id in order {
        let Some(node) = report.graph.node(id) else { continue };
        let pack_name = node.name.clone();
        let pack_path = node.path.clone();
        let actions = node.manifest.actions.clone();
        let manifest = node.manifest.clone();
        let commit_sha = node.commit_sha.clone().unwrap_or_default();
        let synthetic = node.synthetic;
        // v1.3.1 B14: parent manifest's `ref:` value for this node,
        // captured by the walker. Threaded into `upsert_lock_entry`
        // so the lockfile `branch` slot mirrors the manifest verbatim.
        let manifest_ref = node.manifest_ref.clone();
        // `--only` filter + skip-on-hash short-circuits colocated in
        // `try_skip_or_filter` so this outer loop stays within the
        // 50-LOC per-function budget.
        if try_skip_or_filter(
            report,
            only,
            &pack_name,
            &pack_path,
            &actions,
            &commit_sha,
            synthetic,
            workspace,
            prior_lock,
            next_lock,
            dry_run,
            force,
        ) {
            continue;
        }
        let pack_halted = run_pack_lifecycle(
            report,
            vars,
            workspace,
            event_log,
            lock_path,
            dry_run,
            &plan,
            &fs,
            registry,
            pack_type_registry,
            &rt,
            &pack_name,
            &pack_path,
            &manifest,
            &visited_meta,
            scheduler,
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
        // run's skip-on-hash test can succeed. Commit SHA is now plumbed
        // from the walker (M4-D): `PackNode::commit_sha` carries the
        // resolved HEAD SHA when the pack's working tree is a git
        // repository, otherwise an empty string keeps the hash stable.
        let actions_hash = compute_actions_hash(&actions, &commit_sha);
        upsert_lock_entry(
            prior_lock,
            next_lock,
            &pack_name,
            &commit_sha,
            &actions_hash,
            synthetic,
            manifest_ref.as_deref(),
        );
    }
}

/// Build the multi-thread tokio runtime used to drive async pack-type
/// plugin dispatch. Pack-type plugins expose `async fn` methods via
/// `async_trait`, but the sync driver is synchronous end-to-end — we
/// block on each plugin future inside the outer action loop. Extracted
/// into a standalone helper so the runtime construction does not
/// inflate `run_actions` beyond the 50-LOC per-function budget.
///
/// # Multi-thread rationale (M5-2c)
///
/// M5-2c enabled real [`crate::plugin::pack_type::MetaPlugin`] recursion
/// through [`crate::execute::ExecCtx::pack_type_registry`]. The recursion
/// itself is purely `async` / `.await` (no nested `block_on`), but future
/// plugin authors may reasonably compose `block_on` calls inside
/// lifecycle hooks — and external callers that drive `MetaPlugin` via
/// `rt.block_on(...)` within their own runtime would deadlock on a
/// current-thread runtime the moment a hook re-enters. A multi-thread
/// runtime with a small worker pool lets those re-entries resolve on a
/// sibling worker instead of blocking the dispatcher thread.
///
/// # Worker-thread sizing (feat-m6 H6)
///
/// The worker pool is sized from the resolved `--parallel` knob so the
/// runtime always has enough workers to service every in-flight pack op
/// plus at least one sibling for nested `block_on`. Clamped to
/// `[2, num_cpus::get()]`: `2` preserves the pre-M6 floor (one driver +
/// one sibling so re-entrant hooks never deadlock), and the upper bound
/// caps the pool at the host's CPU count so `--parallel 0`
/// (unbounded-semantics) does not explode the worker count.
fn build_pack_type_runtime(parallel: usize) -> tokio::runtime::Runtime {
    let workers = parallel.clamp(2, num_cpus::get().max(2));
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(workers)
        .enable_all()
        .build()
        .expect("tokio runtime for pack-type dispatch")
}

/// Construct a fresh [`MetaVisitedSet`] for one sync run. Walker-driven
/// dispatch does not attach it (see `dispatch_pack_type_plugin`), but
/// the argument is threaded through so future explicit-install /
/// teardown verbs can share the same set shape.
fn new_visited_meta() -> MetaVisitedSet {
    std::sync::Arc::new(std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// Combined short-circuit helper: `--only` filter + skip-on-hash. Returns
/// `true` when the outer loop should `continue` for this pack.
///
/// Extracted from `run_actions` so that function stays under the
/// workspace's 50-LOC per-function lint. Semantics are unchanged; this
/// is a pure structural refactor.
#[allow(clippy::too_many_arguments)]
fn try_skip_or_filter(
    report: &mut SyncReport,
    only: Option<&GlobSet>,
    pack_name: &str,
    pack_path: &Path,
    actions: &[Action],
    commit_sha: &str,
    current_synthetic: bool,
    workspace: &Path,
    prior_lock: &std::collections::HashMap<String, LockEntry>,
    next_lock: &mut std::collections::HashMap<String, LockEntry>,
    dry_run: bool,
    force: bool,
) -> bool {
    if skip_for_only_filter(only, pack_name, pack_path, workspace) {
        if let Some(prev) = prior_lock.get(pack_name) {
            next_lock.insert(pack_name.to_string(), prev.clone());
        }
        return true;
    }
    try_skip_pack(
        report,
        pack_name,
        pack_path,
        actions,
        commit_sha,
        current_synthetic,
        prior_lock,
        next_lock,
        dry_run,
        force,
    )
}

/// Return `true` when `--only` is active and the pack's
/// **workspace-relative path** (normalized to forward-slash form) does
/// not match any of the registered globs. Name-fallback matching was
/// dropped in the M4-D post-review fix bundle: spec §M4 req 6 says
/// "pack paths" and cross-platform consistency requires a single
/// normalized representation rather than `display()`-formatted strings
/// (which use `\\` on Windows and `/` on POSIX — globset treats `\\`
/// as a glob-escape, not a path separator). For the root pack whose
/// `pack_path` is not under `workspace`, the fallback is to match
/// against the absolute path's forward-slash form.
fn skip_for_only_filter(
    only: Option<&GlobSet>,
    pack_name: &str,
    pack_path: &Path,
    workspace: &Path,
) -> bool {
    let Some(set) = only else { return false };
    let rel = pack_path.strip_prefix(workspace).unwrap_or(pack_path);
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let matches = set.is_match(&rel_str);
    if !matches {
        tracing::info!(
            target: "grex::sync",
            "skipping pack `{pack_name}` (rel path `{rel_str}`): does not match --only filter"
        );
    }
    !matches
}

/// Per-pack lifecycle dispatch. Returns `true` when the sync must halt.
///
/// M5-1 Stage C replaces the blind `for action in manifest.actions` loop
/// with a pack-type-aware dispatch:
///
/// * [`PackType::Declarative`] retains the per-action execution shape that
///   M4 shipped — each action lands its own `ActionStarted` /
///   `ActionCompleted` / `ActionHalted` event bracket. The registry is
///   still consulted via [`PackTypeRegistry::get`] as a name-oracle so
///   mistyped packs fail closed.
/// * [`PackType::Meta`] / [`PackType::Scripted`] dispatch once through the
///   pack-type plugin's `sync` method (the sync CLI verb is the only
///   caller in M5-1; `install` / `update` / `teardown` verbs wire in
///   M5-2), returning a single aggregate [`ExecStep`]. A single event
///   bracket frames the async call.
///
/// Declarative is kept on the legacy per-action path because its event log
/// semantics (one event per action, per-step rollback context) are exactly
/// what plugin authors expect to observe. Unifying declarative under the
/// plugin dispatch is M5-2 scope — it requires reshaping the trait surface
/// to emit a step stream rather than a single aggregate.
#[allow(clippy::too_many_arguments)]
fn run_pack_lifecycle(
    report: &mut SyncReport,
    vars: &VarEnv,
    workspace: &Path,
    event_log: &Path,
    lock_path: &Path,
    dry_run: bool,
    plan: &PlanExecutor,
    fs: &FsExecutor,
    registry: &Arc<Registry>,
    pack_type_registry: &Arc<PackTypeRegistry>,
    rt: &tokio::runtime::Runtime,
    pack_name: &str,
    pack_path: &Path,
    manifest: &crate::pack::PackManifest,
    visited_meta: &MetaVisitedSet,
    scheduler: &Arc<Scheduler>,
) -> bool {
    let type_tag = manifest.r#type.as_str();
    // Name-oracle check: every pack type must be registered. Unknown
    // pack types halt the pack the same way M4 halted unknown actions.
    if pack_type_registry.get(type_tag).is_none() {
        let err = ExecError::UnknownAction(format!("pack type `{type_tag}`"));
        record_action_err(dry_run, report, event_log, lock_path, pack_name, 0, "pack-type", err);
        return true;
    }
    match manifest.r#type {
        crate::pack::PackType::Declarative => run_declarative_actions(
            report,
            vars,
            workspace,
            event_log,
            lock_path,
            dry_run,
            plan,
            fs,
            pack_name,
            pack_path,
            manifest,
            &manifest.actions,
            scheduler,
        ),
        crate::pack::PackType::Meta | crate::pack::PackType::Scripted => dispatch_pack_type_plugin(
            report,
            vars,
            workspace,
            event_log,
            lock_path,
            dry_run,
            registry,
            pack_type_registry,
            rt,
            pack_name,
            pack_path,
            manifest,
            type_tag,
            visited_meta,
            scheduler,
        ),
    }
}

/// Run a declarative pack's actions sequentially. Preserves the M4
/// per-action event-log bracket (`ActionStarted` → `ActionCompleted` |
/// `ActionHalted`). Returns `true` when the sync must halt.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_declarative_actions(
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
    manifest: &crate::pack::PackManifest,
    actions: &[Action],
    scheduler: &Arc<Scheduler>,
) -> bool {
    // B12 v1.3.1: `apply_gitignore` was previously called here for
    // declarative packs (the per-action driver bypasses the plugin
    // path). Auto-mutation of the parent meta-repo's `.gitignore` was
    // removed in v1.3.1; `grex doctor` now surfaces an advisory when
    // the parent git index tracks pack content. The function is kept
    // as a no-op shim and the call is left in place so the diff stays
    // minimal — the reviewer pass will delete it together with the
    // other call sites in pack_type.rs.
    if !dry_run {
        let ctx = ExecCtx::new(vars, pack_path, workspace)
            .with_platform(Platform::current())
            .with_scheduler(scheduler);
        if let Err(e) = crate::plugin::pack_type::apply_gitignore(&ctx, manifest) {
            record_action_err(dry_run, report, event_log, lock_path, pack_name, 0, "gitignore", e);
            return true;
        }
    }
    for (idx, action) in actions.iter().enumerate() {
        let ctx = ExecCtx::new(vars, pack_path, workspace)
            .with_platform(Platform::current())
            .with_scheduler(scheduler);
        let action_tag = action_kind_tag(action);
        append_manifest_event(
            dry_run,
            event_log,
            lock_path,
            &Event::ActionStarted {
                ts: Utc::now(),
                id: pack_name.to_string(),
                action_idx: idx,
                action_name: action_tag.to_string(),
                schema_version: SCHEMA_VERSION.to_string(),
            },
            &mut report.event_log_warnings,
        );
        let step_result =
            if dry_run { plan.execute(action, &ctx) } else { fs.execute(action, &ctx) };
        if !record_action_outcome(
            dry_run,
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

/// Dispatch a pack-type plugin (meta / scripted) through the async
/// registry. Brackets the call with a single `ActionStarted` /
/// `ActionCompleted` / `ActionHalted` trio at index 0. Returns `true`
/// when the sync must halt.
#[allow(clippy::too_many_arguments)]
fn dispatch_pack_type_plugin(
    report: &mut SyncReport,
    vars: &VarEnv,
    workspace: &Path,
    event_log: &Path,
    lock_path: &Path,
    dry_run: bool,
    registry: &Arc<Registry>,
    pack_type_registry: &Arc<PackTypeRegistry>,
    rt: &tokio::runtime::Runtime,
    pack_name: &str,
    pack_path: &Path,
    manifest: &crate::pack::PackManifest,
    type_tag: &'static str,
    visited_meta: &MetaVisitedSet,
    scheduler: &Arc<Scheduler>,
) -> bool {
    // NB: `visited_meta` is intentionally NOT attached to the ctx here.
    // The sync driver already walks children in post-order via the tree
    // walker; attaching the visited set would trigger MetaPlugin's
    // real-recursion branch and cause double dispatch (walker runs child
    // packs as their own graph nodes, then MetaPlugin would recurse into
    // them again). The `visited_meta` parameter is kept on the argument
    // list so future explicit-install / teardown verbs that invoke
    // MetaPlugin directly can share the same set shape.
    let _ = visited_meta;
    let ctx = ExecCtx::new(vars, pack_path, workspace)
        .with_platform(Platform::current())
        .with_registry(registry)
        .with_pack_type_registry(pack_type_registry)
        .with_scheduler(scheduler);
    append_manifest_event(
        dry_run,
        event_log,
        lock_path,
        &Event::ActionStarted {
            ts: Utc::now(),
            id: pack_name.to_string(),
            action_idx: 0,
            action_name: type_tag.to_string(),
            schema_version: SCHEMA_VERSION.to_string(),
        },
        &mut report.event_log_warnings,
    );
    // SAFETY: `get` just confirmed the plugin is registered for
    // `type_tag`, so this unwrap cannot panic under the matched arm.
    let plugin = pack_type_registry
        .get(type_tag)
        .expect("pack-type plugin must be registered (guarded above)");
    // feat-m6 CI fix — establish a task-local tier stack frame for every
    // async dispatch. Without this, `TierGuard::push` (which runs inside
    // the plugin lifecycle and may span `.await` / thread hops under the
    // multi-thread runtime) has no enforcement frame to push into.
    let step_result = rt.block_on(crate::pack_lock::with_tier_scope(plugin.sync(&ctx, manifest)));
    !record_action_outcome(
        dry_run,
        report,
        event_log,
        lock_path,
        pack_name,
        0,
        type_tag,
        step_result,
    )
}

/// Pure skip-eligibility decision. Returns `Some(hash)` when the pack
/// is eligible for the hash-skip short-circuit, `None` otherwise.
///
/// Splitting the decision out of [`try_skip_pack`] keeps the
/// side-effecting transcript bookkeeping testable in isolation: the
/// v1.1.1 synthetic-flag-flip regression exercises this helper without
/// having to stand up a `SyncReport` / `PackGraph`.
fn skip_eligibility(
    actions: &[Action],
    commit_sha: &str,
    current_synthetic: bool,
    prior: &LockEntry,
    dry_run: bool,
    force: bool,
) -> Option<String> {
    if dry_run || force {
        // Dry runs must always produce the planned-step transcript so
        // authors can see what `sync` *would* do. `--force` is the
        // operator's explicit opt-out from the hash short-circuit.
        return None;
    }
    let hash = compute_actions_hash(actions, commit_sha);
    if prior.actions_hash != hash {
        return None;
    }
    if prior.synthetic != current_synthetic {
        // Pack-shape flipped between runs (real ↔ synthetic). Even
        // when the actions hash matches by coincidence (e.g. a
        // declarative pack with empty `actions[]` whose pack.yaml was
        // deleted, falling through to a synthetic leaf with the same
        // empty actions list and stable commit SHA), we must NOT
        // carry the stale `synthetic` flag forward. Forcing the
        // upsert path re-emits the entry with the current flag.
        return None;
    }
    Some(hash)
}

/// Decide whether `pack_name` can be short-circuited via a lockfile
/// hash match. When the prior hash matches the freshly-computed hash,
/// emit a single [`ExecResult::Skipped`] step and carry the prior
/// lockfile entry forward unchanged. Returns `true` when the pack was
/// skipped.
///
/// `current_synthetic` is the walker-derived synthetic flag for this
/// pack on the current run. The skip eligibility check requires it to
/// match `prior.synthetic` so a pack-shape transition (e.g. user
/// deletes `pack.yaml` so a previously-real pack now walks as
/// synthetic) invalidates the skip and forces the lockfile entry to
/// be re-emitted with the fresh `synthetic` value.
#[allow(clippy::too_many_arguments)]
fn try_skip_pack(
    report: &mut SyncReport,
    pack_name: &str,
    pack_path: &Path,
    actions: &[Action],
    commit_sha: &str,
    current_synthetic: bool,
    prior_lock: &std::collections::HashMap<String, LockEntry>,
    next_lock: &mut std::collections::HashMap<String, LockEntry>,
    dry_run: bool,
    force: bool,
) -> bool {
    let Some(prior) = prior_lock.get(pack_name) else {
        return false;
    };
    let Some(hash) =
        skip_eligibility(actions, commit_sha, current_synthetic, prior, dry_run, force)
    else {
        return false;
    };
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
///
/// Stores `commit_sha` verbatim — including the empty string when the
/// pack is not a git working tree or the HEAD probe failed.
/// `actions_hash` is computed over the same `commit_sha`, so the two
/// fields stay internally consistent: if probing starts returning a
/// non-empty SHA on the next run, the hash differs and the skip is
/// correctly invalidated. The prior-preserve carve-out that was
/// introduced in M4-D was unsound (hash-vs-sha drift) and is removed
/// by the M4-D post-review fix bundle; see spec §M4 req 4a.
///
/// `prior_lock` is consulted purely for observability: when a
/// previously-real pack flips to synthetic between runs (user deleted
/// the pack's `pack.yaml` so the walker fell back to v1.1.1
/// plain-git-child synthesis), a `tracing::warn!` records the
/// downgrade so the operator notices their declarative actions have
/// stopped running.
fn upsert_lock_entry(
    prior_lock: &std::collections::HashMap<String, LockEntry>,
    next_lock: &mut std::collections::HashMap<String, LockEntry>,
    pack_name: &str,
    commit_sha: &str,
    actions_hash: &str,
    synthetic: bool,
    manifest_ref: Option<&str>,
) {
    if synthetic {
        if let Some(prior) = prior_lock.get(pack_name) {
            if !prior.synthetic {
                tracing::warn!(
                    target: "grex::sync",
                    pack = pack_name,
                    "pack `{pack_name}` downgraded from real to synthetic — \
                     pack.yaml missing on disk; only `git pull` will run going forward",
                );
            }
        }
    }
    let installed_at = Utc::now();
    let entry = next_lock.get(pack_name).map_or_else(
        || LockEntry {
            id: pack_name.to_string(),
            // v1.1.1 convention: path == id (1:1 id↔folder). Stage 1.e
            // (walker rewrite) will replace this with the parent-relative
            // manifest path captured during the walk.
            path: pack_name.to_string(),
            sha: commit_sha.to_string(),
            // v1.3.1 B14: mirror the parent manifest's `ref:` value
            // (or empty when absent), per the Lean theorem
            // `Grex.Lockfile.lockfile_branch_mirrors_manifest_ref`.
            branch: branch_of(manifest_ref),
            installed_at,
            actions_hash: actions_hash.to_string(),
            schema_version: "1".to_string(),
            synthetic,
        },
        |prev| LockEntry {
            installed_at,
            actions_hash: actions_hash.to_string(),
            sha: commit_sha.to_string(),
            synthetic,
            ..prev.clone()
        },
    );
    next_lock.insert(pack_name.to_string(), entry);
}

/// Record one action outcome into `report` + event log. Returns `false`
/// when the run must halt (on error); `true` otherwise.
#[allow(clippy::too_many_arguments)]
fn record_action_outcome(
    dry_run: bool,
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
            record_action_ok(dry_run, report, event_log, lock_path, pack_name, idx, step);
            true
        }
        Err(e) => {
            record_action_err(dry_run, report, event_log, lock_path, pack_name, idx, action_tag, e);
            false
        }
    }
}

/// Success-path bookkeeping: emit legacy `Sync` summary + `ActionCompleted`
/// audit event, then push the step onto the report.
///
/// v1.3.1 B4 fix-up: under `dry_run = true`, the on-disk event-log writes
/// are skipped. The in-memory `report.steps` push still happens — dry-run
/// callers rely on the planned-step transcript for output.
#[allow(clippy::too_many_arguments)]
fn record_action_ok(
    dry_run: bool,
    report: &mut SyncReport,
    event_log: &Path,
    lock_path: &Path,
    pack_name: &str,
    idx: usize,
    step: ExecStep,
) {
    append_step_event(
        dry_run,
        event_log,
        lock_path,
        pack_name,
        &step,
        &mut report.event_log_warnings,
    );
    append_manifest_event(
        dry_run,
        event_log,
        lock_path,
        &Event::ActionCompleted {
            ts: Utc::now(),
            id: pack_name.to_string(),
            action_idx: idx,
            result_summary: format!("{:?}", step.result),
            schema_version: SCHEMA_VERSION.to_string(),
        },
        &mut report.event_log_warnings,
    );
    report.steps.push(SyncStep { pack: pack_name.to_string(), action_idx: idx, exec_step: step });
}

/// Halt-path bookkeeping: emit `ActionHalted` audit event, then stash the
/// rich `HaltedContext` into `report.halted`.
///
/// v1.3.1 B4 fix-up: under `dry_run = true`, the on-disk event-log write
/// is skipped; the `report.halted` slot still receives the
/// [`HaltedContext`] so callers can render the halt reason without
/// touching disk.
#[allow(clippy::too_many_arguments)]
fn record_action_err(
    dry_run: bool,
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
        dry_run,
        event_log,
        lock_path,
        &Event::ActionHalted {
            ts: Utc::now(),
            id: pack_name.to_string(),
            action_idx: idx,
            action_name: action_tag.to_string(),
            error_summary,
            schema_version: SCHEMA_VERSION.to_string(),
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
        Action::Unlink(_) => "unlink",
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
    dry_run: bool,
    log: &Path,
    lock_path: &Path,
    pack: &str,
    step: &ExecStep,
    warnings: &mut Vec<String>,
) {
    if dry_run {
        return;
    }
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
///
/// v1.3.1 B4 fix-up: when `dry_run` is `true`, this function is a no-op
/// — the dry-run contract forbids any write to `<workspace>/.grex/`,
/// including the audit `events.jsonl`. In-memory `event_log_warnings`
/// records remain available; only the on-disk side effect is gated.
fn append_manifest_event(
    dry_run: bool,
    log: &Path,
    lock_path: &Path,
    event: &Event,
    warnings: &mut Vec<String>,
) {
    if dry_run {
        return;
    }
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

/// Run a full teardown over the pack tree rooted at `pack_root`.
///
/// Mirrors [`run`] but invokes
/// [`crate::plugin::PackTypePlugin::teardown`] on every pack in
/// **reverse** post-order so a parent tears down before its children
/// (the inverse of install). Children composed later by an author
/// consequently teardown earlier, matching the declarative
/// auto-reverse contract (R-M5-11).
///
/// All other concerns are identical to [`run`]: workspace lock, plan-
/// phase validators, lockfile update skipped (teardown does not
/// write a `actions_hash` forward), and event-log bracketing.
/// Teardown does NOT consult the lockfile skip-on-hash shortcut — a
/// user explicitly asked to remove the pack, so we always dispatch.
///
/// # Errors
///
/// Returns the first error that halts the pipeline — see [`SyncError`].
///
/// See [`run`] for the `cancel` contract — feat-m7-1 stage 2 threads
/// the parameter through teardown for parity; stages 3-4 add the polls.
pub fn teardown(
    pack_root: &Path,
    opts: &SyncOptions,
    cancel: &CancellationToken,
) -> Result<SyncReport, SyncError> {
    let _ = cancel;
    let workspace = prepare_workspace(pack_root, opts)?;
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

    // v1.2.1 path (iii) — teardown is read-only against the existing
    // disk state (no clones / fetches / prunes). It only needs the
    // graph build pass; `sync_meta` is intentionally skipped here.
    let graph = build_and_validate_graph(&workspace, opts.validate, opts.ref_override.as_deref())?;
    let prep = prepare_run_context(pack_root, &graph, &workspace)?;

    let mut report = SyncReport {
        graph,
        steps: Vec::new(),
        halted: None,
        event_log_warnings: Vec::new(),
        pre_run_recovery: prep.pre_run_recovery,
        // teardown does not run the legacy-layout migration — by the time
        // a user is tearing down, the layout has already been migrated
        // (or was never legacy in the first place). Surfacing an empty
        // list keeps the report shape symmetric with `run()`.
        workspace_migrations: Vec::new(),
    };

    // feat-m6 B1: mirror `run()` — resolve `--parallel`, build a
    // Scheduler, thread it through every `ExecCtx` the teardown path
    // constructs. Teardown is the other user-facing verb that owns a
    // runtime, so it gets the same wiring.
    let resolved_parallel: usize = opts.parallel.unwrap_or_else(|| num_cpus::get().max(1));
    let scheduler = Arc::new(Scheduler::new(resolved_parallel));
    run_teardown(
        &mut report,
        &prep.order,
        &prep.vars,
        &workspace,
        &prep.event_log,
        &prep.lock_path,
        &prep.registry,
        &prep.pack_type_registry,
        resolved_parallel,
        &scheduler,
    );
    Ok(report)
}

/// Dispatch `teardown` for every pack in **reverse** post-order.
/// Declarative packs go through [`crate::plugin::PackTypePlugin`]
/// rather than the per-action M4 path because the trait's
/// auto-reverse / explicit-block logic must compose with the
/// registry; going through the per-action path would mean
/// re-implementing inverse synthesis in the sync loop.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn run_teardown(
    report: &mut SyncReport,
    order: &[usize],
    vars: &VarEnv,
    workspace: &Path,
    event_log: &Path,
    lock_path: &Path,
    registry: &Arc<Registry>,
    pack_type_registry: &Arc<PackTypeRegistry>,
    parallel: usize,
    scheduler: &Arc<Scheduler>,
) {
    let rt = build_pack_type_runtime(parallel);
    // Reverse post-order: root first, then children. Pack-type plugin
    // teardown methods reverse their own children/actions, so the
    // outer loop only flips the inter-pack order.
    for &id in order.iter().rev() {
        let Some(node) = report.graph.node(id) else { continue };
        let pack_name = node.name.clone();
        let pack_path = node.path.clone();
        let manifest = node.manifest.clone();
        let type_tag = manifest.r#type.as_str();
        if pack_type_registry.get(type_tag).is_none() {
            let err = ExecError::UnknownAction(format!("pack type `{type_tag}`"));
            // Teardown has no dry-run mode — pass `false` so the
            // event-log writes proceed as before.
            record_action_err(false, report, event_log, lock_path, &pack_name, 0, "pack-type", err);
            return;
        }
        let ctx = ExecCtx::new(vars, &pack_path, workspace)
            .with_platform(Platform::current())
            .with_registry(registry)
            .with_pack_type_registry(pack_type_registry)
            .with_scheduler(scheduler);
        append_manifest_event(
            false,
            event_log,
            lock_path,
            &Event::ActionStarted {
                ts: Utc::now(),
                id: pack_name.clone(),
                action_idx: 0,
                action_name: type_tag.to_string(),
                schema_version: SCHEMA_VERSION.to_string(),
            },
            &mut report.event_log_warnings,
        );
        let plugin = pack_type_registry
            .get(type_tag)
            .expect("pack-type plugin must be registered (guarded above)");
        // feat-m6 CI fix — see dispatch_pack_type note.
        let step_result =
            rt.block_on(crate::pack_lock::with_tier_scope(plugin.teardown(&ctx, &manifest)));
        if !record_action_outcome(
            false,
            report,
            event_log,
            lock_path,
            &pack_name,
            0,
            type_tag,
            step_result,
        ) {
            return;
        }
    }
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

/// Walk `workspace` and the manifest log to find crash-recovery artifacts.
///
/// Inspects:
///
/// * `workspace` for `.grex.bak` orphans and timestamped `.grex.bak.<ts>`
///   tombstones. The workspace IS where children materialise (whether
///   the default flat-sibling layout under the pack root, or an
///   explicit `--workspace` override directory) so this single bounded
///   walk covers every backup site.
/// * `event_log` (the manifest JSONL) for `ActionStarted` entries that
///   have no matching `ActionCompleted` / `ActionHalted` successor.
///
/// Non-blocking: scan errors are swallowed to an empty report so a
/// half-readable directory cannot kill a sync that would otherwise
/// succeed. Call sites that want to surface scan failures should read
/// the manifest directly.
///
/// Pre-`v1.1.0` post-review fix this anchored at `pack_root_dir(pack_root)`,
/// which missed every backup under a `--workspace` override.
///
/// # Errors
///
/// Returns [`SyncError::Validation`] only when the manifest read itself
/// reports corruption. Filesystem traversal errors are swallowed.
pub fn scan_recovery(workspace: &Path, event_log: &Path) -> Result<RecoveryReport, SyncError> {
    let mut report = RecoveryReport::default();
    walk_for_backups(workspace, &mut report);
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
    for entry_result in entries {
        let entry = match entry_result {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(
                    target: "grex::sync::recover",
                    "skipping unreadable entry under `{}`: {e}",
                    dir.display(),
                );
                continue;
            }
        };
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
        // traversing into the workspace's cloned repos via aliased
        // paths). `entry.file_type()` does NOT follow symlinks (unlike
        // `entry.metadata()` which would dereference and report the
        // target's type — defeating the very check this guards). The
        // symlink-skip is also explicit so the intent is recoverable
        // from the source: backup-recovery never crosses a symlink.
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
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
            // v1.3.1 schema v2: pack-id field is `id`. The destructure
            // binds `id` and `schema_version` is ignored via `..`.
            Event::ActionStarted { ts, id, action_idx, action_name, .. } => {
                open.insert(
                    (id.clone(), *action_idx),
                    DanglingStart {
                        pack: id.clone(),
                        action_idx: *action_idx,
                        action_name: action_name.clone(),
                        started_at: *ts,
                    },
                );
            }
            Event::ActionCompleted { id, action_idx, .. }
            | Event::ActionHalted { id, action_idx, .. } => {
                open.remove(&(id.clone(), *action_idx));
            }
            _ => {}
        }
    }
    let mut out: Vec<DanglingStart> = open.into_values().collect();
    out.sort_by_key(|a| a.started_at);
    out
}

#[cfg(test)]
mod synthetic_transition_tests {
    //! v1.1.1 — regression cover for the pack-shape transition fixes.
    //!
    //! These tests exercise [`skip_eligibility`] / [`upsert_lock_entry`]
    //! directly (no walker, no fs) so the assertion is on the plumbing
    //! itself: skip eligibility must require synthetic-flag agreement
    //! even when the actions hash matches by coincidence, and the
    //! upsert path must record the real-to-synthetic downgrade in the
    //! lockfile so the operator's lockfile reflects what just happened.
    use super::{skip_eligibility, upsert_lock_entry, LockEntry};
    use crate::lockfile::compute_actions_hash;
    use chrono::{TimeZone, Utc};
    use std::collections::HashMap;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 27, 10, 0, 0).unwrap()
    }

    /// Stable empty-actions hash with a fixed commit SHA. The same
    /// inputs feed both the prior (real) and the new (synthetic)
    /// configuration in the regression below, which is exactly the
    /// coincidental-hash-match scenario FIX 3 must catch.
    fn stable_hash() -> String {
        compute_actions_hash(&[], "deadbeef")
    }

    fn prior_entry(synthetic: bool) -> LockEntry {
        LockEntry {
            id: "alpha".into(),
            path: "alpha".into(),
            sha: "deadbeef".into(),
            branch: "main".into(),
            installed_at: ts(),
            actions_hash: stable_hash(),
            schema_version: "1".into(),
            synthetic,
        }
    }

    /// FIX 3 — pack flips from real → synthetic but `actions_hash` and
    /// `commit_sha` happen to match. The skip MUST be invalidated so
    /// the upsert path re-emits the lockfile entry with `synthetic =
    /// true`.
    #[test]
    fn skip_eligibility_invalidates_when_synthetic_flag_flips() {
        let prior = prior_entry(false);
        let decision = skip_eligibility(&[], "deadbeef", true, &prior, false, false);
        assert!(decision.is_none(), "skip must be invalidated when synthetic flag flips");
    }

    /// Same hash, same synthetic flag → skip is allowed (baseline).
    #[test]
    fn skip_eligibility_allows_skip_when_synthetic_matches() {
        let prior = prior_entry(true);
        let decision = skip_eligibility(&[], "deadbeef", true, &prior, false, false);
        assert_eq!(
            decision.as_deref(),
            Some(stable_hash().as_str()),
            "skip must be honoured when synthetic flag matches",
        );
    }

    /// `dry_run` and `force` always disable the skip regardless of
    /// flag agreement.
    #[test]
    fn skip_eligibility_respects_dry_run_and_force() {
        let prior = prior_entry(true);
        assert!(skip_eligibility(&[], "deadbeef", true, &prior, true, false).is_none());
        assert!(skip_eligibility(&[], "deadbeef", true, &prior, false, true).is_none());
    }

    /// FIX 4 — `upsert_lock_entry` records the downgrade in the
    /// lockfile (entry flips to `synthetic = true`) when the prior
    /// entry was real. The `tracing::warn!` is fire-and-forget, but
    /// the lockfile transition itself is observable and must be
    /// correct.
    #[test]
    fn upsert_lock_entry_records_real_to_synthetic_downgrade() {
        let mut prior: HashMap<String, LockEntry> = HashMap::new();
        prior.insert(
            "beta".into(),
            LockEntry {
                id: "beta".into(),
                path: "beta".into(),
                sha: "deadbeef".into(),
                branch: "main".into(),
                installed_at: ts(),
                actions_hash: stable_hash(),
                schema_version: "1".into(),
                synthetic: false,
            },
        );
        let mut next: HashMap<String, LockEntry> = HashMap::new();

        upsert_lock_entry(&prior, &mut next, "beta", "deadbeef", &stable_hash(), true, None);

        let entry = next.get("beta").expect("entry must be upserted");
        assert!(entry.synthetic, "downgraded entry must carry synthetic = true");
        assert_eq!(entry.actions_hash, stable_hash(), "actions_hash must reflect current run");
    }

    /// Upsert path is a no-op for the steady-state case (synthetic →
    /// synthetic): the entry is replaced with the current run's
    /// timestamp/hash but the synthetic flag is preserved. This
    /// guards against an over-eager warning fire.
    #[test]
    fn upsert_lock_entry_no_op_for_steady_state_synthetic() {
        let mut prior: HashMap<String, LockEntry> = HashMap::new();
        prior.insert(
            "gamma".into(),
            LockEntry {
                id: "gamma".into(),
                path: "gamma".into(),
                sha: "deadbeef".into(),
                branch: "main".into(),
                installed_at: ts(),
                actions_hash: stable_hash(),
                schema_version: "1".into(),
                synthetic: true,
            },
        );
        let mut next: HashMap<String, LockEntry> = HashMap::new();

        upsert_lock_entry(&prior, &mut next, "gamma", "deadbeef", &stable_hash(), true, None);

        let entry = next.get("gamma").expect("entry must be upserted");
        assert!(entry.synthetic, "synthetic must remain true on no-op refresh");
    }
}

#[cfg(test)]
mod error_display_tests {
    //! v1.2.0 Stage 1.k — `SyncError` Display assertions.
    //!
    //! Pure construction + `to_string()` checks. Variants land dormant —
    //! Stage 1.g (rayon scheduler) wires `SchedulerCancelled` once
    //! cooperative cancel polls reach the parallel walker.
    use super::SyncError;

    #[test]
    fn test_sync_error_scheduler_cancelled_display() {
        let err = SyncError::SchedulerCancelled;
        assert_eq!(err.to_string(), "sync cancelled by user");
    }
}

#[cfg(test)]
mod sync_options_v1_2_0_tests {
    //! v1.2.0 Stage 1.m — leaf cover for new [`SyncOptions`] fields.
    //!
    //! These tests are mechanical default-value assertions plus simple
    //! builder/clone round-trips. They exist to lock down that:
    //!
    //! 1. Adding the new fields preserves v1.1.1 behavior (defaults
    //!    leave existing call sites observably unchanged).
    //! 2. The shape is what later walker stages (1.h / 1.j / 1.l) will
    //!    consume — if any of these fields are renamed or change type,
    //!    those stages must update in lock-step.
    //!
    //! The fields themselves are *dormant placeholders* at 1.m scope —
    //! no behavior wiring lives in this stage.
    use super::{pack_root_dir, resolve_workspace, SyncError, SyncOptions};

    /// `force_prune` defaults to `false` so existing call sites refuse
    /// to drop dirty trees (v1.1.1 behavior).
    #[test]
    fn test_sync_options_default_force_prune_false() {
        let opts = SyncOptions::default();
        assert!(!opts.force_prune, "force_prune must default to false");
    }

    /// `force_prune_with_ignored` defaults to `false` so existing call
    /// sites refuse to drop ignored content (v1.1.1 behavior).
    #[test]
    fn test_sync_options_default_force_prune_with_ignored_false() {
        let opts = SyncOptions::default();
        assert!(!opts.force_prune_with_ignored, "force_prune_with_ignored must default to false");
    }

    /// `migrate_lockfile` defaults to `false` so the walker errors on
    /// legacy v1.1.1 lockfile shapes unless the caller opts in.
    #[test]
    fn test_sync_options_default_migrate_lockfile_false() {
        let opts = SyncOptions::default();
        assert!(!opts.migrate_lockfile, "migrate_lockfile must default to false");
    }

    /// `recurse` defaults to `true` — the walker descends into nested
    /// meta-children unless `--shallow` is requested.
    #[test]
    fn test_sync_options_default_recurse_true() {
        let opts = SyncOptions::default();
        assert!(opts.recurse, "recurse must default to true");
    }

    /// `max_depth` defaults to `None` — unbounded recursion when
    /// `recurse` is `true`.
    #[test]
    fn test_sync_options_default_max_depth_none() {
        let opts = SyncOptions::default();
        assert!(opts.max_depth.is_none(), "max_depth must default to None");
    }

    /// Setting `force_prune_with_ignored = true` alongside
    /// `force_prune = true` is the documented "stronger" combination.
    /// No contradiction: `with_ignored` is the harder override and
    /// implies the base `force_prune` semantics. This test guards the
    /// invariant that both flags coexist as plain `bool` (not enum)
    /// so callers can set them independently without runtime panic.
    #[test]
    fn test_sync_options_force_prune_with_ignored_implies_force_prune() {
        let opts = SyncOptions {
            force_prune: true,
            force_prune_with_ignored: true,
            ..SyncOptions::default()
        };
        assert!(opts.force_prune);
        assert!(opts.force_prune_with_ignored);
    }

    /// `max_depth = Some(n)` paired with `recurse = true` is the
    /// documented `--shallow=N` shape. The fields are independent
    /// `bool` / `Option<usize>` so callers may set `max_depth` while
    /// `recurse` is left at its default (`true`). Stage 1.j will
    /// later define the precise interaction; this test only locks
    /// the two fields' types and defaults.
    #[test]
    fn test_sync_options_max_depth_pairs_with_recurse() {
        let opts = SyncOptions { max_depth: Some(2), ..SyncOptions::default() };
        assert_eq!(opts.max_depth, Some(2));
        assert!(opts.recurse, "recurse stays at its default (true) when only max_depth is set");
    }

    /// Round-trip via `Clone` — guards that all new fields participate
    /// in the existing `Clone` derive (no `#[clone(skip)]` slipped in).
    #[test]
    fn test_sync_options_clone_preserves_new_fields() {
        let opts = SyncOptions {
            force_prune: true,
            force_prune_with_ignored: true,
            migrate_lockfile: true,
            recurse: false,
            max_depth: Some(7),
            ..SyncOptions::default()
        };
        let cloned = opts.clone();
        assert_eq!(cloned.force_prune, opts.force_prune);
        assert_eq!(cloned.force_prune_with_ignored, opts.force_prune_with_ignored);
        assert_eq!(cloned.migrate_lockfile, opts.migrate_lockfile);
        assert_eq!(cloned.recurse, opts.recurse);
        assert_eq!(cloned.max_depth, opts.max_depth);
    }

    // ------------------------------------------------------------------
    // v1.2.1 path (iii) — `resolve_workspace` canonicalisation tests
    // ------------------------------------------------------------------

    /// `--workspace` pointing at a non-existent directory must fail
    /// fast with a Validation error citing the offending path. We
    /// explicitly do NOT mkdir-p someone else's typo — `--workspace`
    /// is an opt-in operator decision and a missing target is always
    /// a configuration mistake.
    #[test]
    fn test_resolve_workspace_errors_on_missing_override_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        let pack_root = tmp.path();
        let err = resolve_workspace(pack_root, Some(missing.as_path())).expect_err("must fail");
        match err {
            SyncError::Validation { errors } => {
                assert!(errors.iter().any(|e| format!("{e}").contains("does not exist")));
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    /// `--workspace = None` is the default cwd-meta path — no
    /// canonicalize, no fail-on-missing. The pack-root path is
    /// returned verbatim (post `pack_root_dir` normalisation).
    #[test]
    fn test_resolve_workspace_none_returns_pack_root_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let pack_root = tmp.path().join("nonexistent-yet");
        let resolved = resolve_workspace(&pack_root, None).expect("None override is always Ok");
        assert_eq!(resolved, pack_root_dir(&pack_root));
    }

    /// `--workspace = Some(<existing>)` returns the canonicalised path.
    /// On Windows this typically inserts the `\\?\` long-path prefix;
    /// on Unix it resolves any `..` / symlink components. Either way
    /// the returned path is what every downstream pass anchors against.
    #[test]
    fn test_resolve_workspace_canonicalises_existing_override() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real-ws");
        std::fs::create_dir_all(&real).unwrap();
        let pack_root = tmp.path();
        let resolved =
            resolve_workspace(pack_root, Some(real.as_path())).expect("existing dir must resolve");
        let canonical = real.canonicalize().unwrap();
        assert_eq!(resolved, canonical);
    }
}
