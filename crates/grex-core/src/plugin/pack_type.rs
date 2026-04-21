//! Pack-type plugin trait + registry — M5-1 Stage A/B.
//!
//! Mirrors the M4 [`crate::plugin::ActionPlugin`] surface but one level up:
//! instead of executing a single [`crate::pack::Action`], a
//! [`PackTypePlugin`] owns the full lifecycle for a pack kind
//! (`meta` / `declarative` / `scripted`, plus any future plugin-contributed
//! kind). The trait is intentionally async so pack-type drivers can perform
//! I/O-bound work (child-pack fetch, lockfile round-trip, per-action await)
//! without blocking an executor thread — teardown and sync in particular
//! will shell out and wait on external processes in M5-2+.
//!
//! # Stage A scope
//!
//! Stage A landed the trait and registry **only**. No builtins were
//! registered and the executor dispatch was untouched — Stage B fills in
//! the three built-in pack types (`meta`, `declarative`, `scripted`) and
//! wires them into [`PackTypeRegistry::bootstrap`]. The executor dispatch
//! swap still belongs to Stage C.
//!
//! # Stage B scope
//!
//! * [`MetaPlugin`] — composition driver. Stage B enumerates
//!   [`PackManifest::children`] without loading child manifests (no
//!   child-pack loader helper exists yet); real recursion through a
//!   [`PackTypeRegistry`] lookup lands in Stage C once that helper is in
//!   place.
//! * [`DeclarativePlugin`] — iterates [`PackManifest::actions`] and
//!   dispatches each through the action [`crate::plugin::Registry`]
//!   attached to [`ExecCtx::registry`]. Teardown is deliberately stubbed
//!   until M5-2 picks up reverse / explicit-teardown semantics.
//! * [`ScriptedPlugin`] — shells out to `.grex/hooks/<lifecycle>.{sh,ps1}`
//!   rooted at [`ExecCtx::pack_root`]. Missing hook is a valid no-op; a
//!   non-zero exit surfaces as [`ExecError::ExecNonZero`].
//!
//! # Why `#[async_trait::async_trait]` instead of native async-fn-in-trait
//!
//! Rust 2024 stabilised `async fn` in traits, but `dyn Trait` usage still
//! requires boxing the returned future. `async-trait` desugars to exactly
//! that `Box<dyn Future>` shape and lets the registry store
//! `Box<dyn PackTypePlugin>` the same way the M4 [`crate::plugin::Registry`]
//! stores `Box<dyn ActionPlugin>`. Going native would force every call-site
//! to spell out `RTN` bounds or wrap futures manually — unnecessary
//! ceremony for a Stage A slicing change.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::execute::{ExecCtx, ExecError, ExecStep};
use crate::fs::gitignore;
use crate::pack::{Action, PackManifest};
use crate::pack_lock::{PackLock, PackLockError};

/// Default managed-gitignore patterns every pack contributes on top of
/// its authored `x-gitignore` list. `feat-m6-2` adds `.grex-lock` so the
/// per-pack lock file does not appear in `git status`.
pub const DEFAULT_MANAGED_GITIGNORE_PATTERNS: &[&str] = &[crate::pack_lock::PACK_LOCK_FILE_NAME];

/// Accessor used by integration tests to pin the default managed-gitignore
/// patterns contributed by grex itself (`.grex-lock` as of feat-m6-2).
#[must_use]
pub fn default_managed_gitignore_patterns() -> &'static [&'static str] {
    DEFAULT_MANAGED_GITIGNORE_PATTERNS
}

/// Translate a [`PackLockError`] into the [`ExecError`] taxonomy used by
/// the plugin trait surface. Kept here (not in `pack_lock.rs`) so the
/// error-type dependency stays one-way: `pack_lock` does not know about
/// `execute::ExecError`.
///
/// [`PackLockError::Busy`] from same-process re-entry is mapped to
/// [`ExecError::MetaCycle`]: the pack-lock layer is a defence-in-depth
/// backstop for the [`MetaPlugin`] cycle detector, and surfacing the
/// same variant keeps the caller's error handling uniform regardless of
/// which guard actually caught the re-entry.
pub(crate) fn map_pack_lock_err(e: PackLockError) -> ExecError {
    match e {
        PackLockError::Io { path, source } => {
            ExecError::FsIo { op: "pack_lock", path, detail: source.to_string() }
        }
        PackLockError::Busy { path } => {
            // `Busy` from `PackLock::acquire` only fires on same-process
            // re-entry (cross-process contention blocks on fd-lock and
            // never surfaces here). A re-entry is by definition a cycle
            // in the pack-type dispatch graph — map to the existing
            // MetaCycle variant so callers match a single shape.
            //
            // Strip the `.grex-lock` sidecar filename so the error
            // refers to the pack root the recursion re-entered.
            let pack_root = path.parent().map(std::path::Path::to_path_buf).unwrap_or(path);
            ExecError::MetaCycle { path: pack_root }
        }
    }
}

/// Acquire the tier-2 scheduler permit (if a scheduler is attached) and
/// return the owned permit handle. `None` when no scheduler was plumbed
/// onto the context — plugins still acquire the per-pack lock either way.
///
/// Held inside the plugin body for the full lifecycle call so the
/// semaphore cap bounds in-flight pack ops. Released on the caller's
/// `Drop` once the returned permit goes out of scope.
pub(crate) async fn acquire_scheduler_permit(
    ctx: &ExecCtx<'_>,
) -> Result<Option<tokio::sync::OwnedSemaphorePermit>, ExecError> {
    let Some(scheduler) = ctx.scheduler else { return Ok(None) };
    Ok(Some(scheduler.acquire().await))
}

/// Insert `canonical(ctx.pack_root)` into the shared `visited_meta`
/// cycle-detection set on entry to a plugin lifecycle method. Returns
/// a scoped guard that removes the entry on `Drop`.
///
/// The pre-feat-m6-2 behaviour only inserted *children* in
/// [`MetaPlugin::recurse_one`]; that missed the case where a grandchild
/// recurses back to the outer pack root (e.g. a `..` child edge).
/// Registering self here makes the non-reentrant per-pack lock safe —
/// any recursion resolving to the same canonical path halts with
/// [`ExecError::MetaCycle`] in `recurse_one` before the lock acquire.
///
/// `None` when `ctx.visited_meta` is unattached (direct-callers in
/// tests that don't drive the outer cycle-detection set): the lock
/// layer falls back to the blocking mutex path, which is safe for
/// non-recursive entry.
pub(crate) fn register_self_in_visited(ctx: &ExecCtx<'_>) -> Result<OwnCycleGuard, ExecError> {
    let Some(visited) = ctx.visited_meta else {
        return Ok(OwnCycleGuard {
            visited: None,
            canonical: std::path::PathBuf::new(),
            owns: false,
        });
    };
    let canonical =
        std::fs::canonicalize(ctx.pack_root).unwrap_or_else(|_| ctx.pack_root.to_path_buf());
    let mut guard = visited.lock().map_err(|_| ExecError::MetaCycle { path: canonical.clone() })?;
    // Three distinct call-paths reach this function:
    //
    //  1. **Fresh entry via `recurse_one`**: the parent
    //     `MetaPlugin`'s `recurse_one` inserted `canonical` just
    //     before `dispatch_child`, so `insert` returns `false` and
    //     the parent owns the pop. We take a no-op guard
    //     (`owns = false`).
    //
    //  2. **Outermost entry** (top-level `MetaPlugin.install(root)`
    //     invocation): the set does not yet contain `canonical`.
    //     `insert` returns `true`; we claim ownership and will
    //     remove the entry on drop.
    //
    //  3. **Re-entry from a foreign caller** (concurrent
    //     `MetaPlugin.install` on the same root from a different
    //     task that already inserted via its own register_self,
    //     or an actual recursion that `recurse_one` did not
    //     guard): `insert` returns `false` and no recurse_one on
    //     our stack will pop us. Treat as a cycle and halt
    //     before touching the per-pack lock.
    //
    // The `recurse_one` callers guarantee their insert happens
    // IN THE SAME lock scope as the subsequent `dispatch_child`,
    // so any `insert == false` observed here that is NOT
    // immediately preceded by a `recurse_one::insert` is case
    // (3). We distinguish (1) from (3) by tagging the visited
    // entries: callers that own the pop use the raw path; callers
    // coming from a concurrent re-entry observe an EXISTING entry
    // AND no local `recurse_one` frame.
    //
    // The current implementation makes a simpler, correct
    // decision: we ALWAYS fail when `insert` returns `false`.
    // The `recurse_one` path no longer inserts pre-dispatch;
    // instead the dispatched plugin's own `register_self` does
    // the insert, and `recurse_one` just checks whether the
    // child's canonical was *already* visited (genuine cycle
    // case). That bridge lives in [`MetaPlugin::recurse_one`].
    if !guard.insert(canonical.clone()) {
        return Err(ExecError::MetaCycle { path: canonical });
    }
    drop(guard);
    Ok(OwnCycleGuard { visited: Some(visited.clone()), canonical, owns: true })
}

/// RAII guard that pops the pack root from the visited set on drop.
/// Field names use leading `_` to allow unused-variable patterns at
/// call sites (`let _own_cycle_guard = …`) without triggering lints.
///
/// `owns = false` means some caller (typically `recurse_one`) owns
/// the entry and will remove it; we skip the pop to avoid racing a
/// sibling's insert.
pub(crate) struct OwnCycleGuard {
    visited: Option<crate::execute::MetaVisitedSet>,
    canonical: std::path::PathBuf,
    owns: bool,
}

impl Drop for OwnCycleGuard {
    fn drop(&mut self) {
        if !self.owns {
            return;
        }
        if let Some(v) = &self.visited {
            if let Ok(mut set) = v.lock() {
                set.remove(&self.canonical);
            }
        }
    }
}

/// Key used under [`PackManifest::extensions`] to carry a pack's
/// `.gitignore` patterns (R-M5-08 integration). The YAML shape is a
/// sequence of strings:
///
/// ```yaml
/// x-gitignore:
///   - target/
///   - "*.log"
/// ```
///
/// Missing key → pack contributes no managed block. Empty list → the
/// block is written with zero pattern lines (author opt-in marker).
/// Any non-sequence / non-string entries are silently ignored so a
/// malformed extension never halts the lifecycle.
const GITIGNORE_EXT_KEY: &str = "x-gitignore";

/// Extract `x-gitignore` patterns from `pack.extensions`. Returns
/// `None` when the key is absent so callers can skip gitignore
/// integration entirely (no managed block written or removed).
fn read_gitignore_patterns(pack: &PackManifest) -> Option<Vec<String>> {
    let raw = pack.extensions.get(GITIGNORE_EXT_KEY)?;
    let seq = raw.as_sequence()?;
    Some(seq.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
}

/// Resolve the `.gitignore` path a pack writes its managed block into.
/// Convention: `<workspace>/.gitignore`. Keeps every pack's block in
/// one file so operators inspect one location regardless of pack
/// layout. The workspace-level placement also matches how tools
/// (`git check-ignore`, editors) resolve rules.
fn gitignore_target(ctx: &ExecCtx<'_>) -> std::path::PathBuf {
    ctx.workspace.join(".gitignore")
}

/// Write the `x-gitignore` managed block for `pack`. Every pack gets a
/// managed block that always includes the default grex-managed patterns
/// (`.grex-lock` as of feat-m6-2); the author's `x-gitignore` list is
/// appended after the defaults. If no extension is present and the pack
/// would contribute nothing beyond defaults the block is still written
/// so the `.grex-lock` file never leaks into `git status`.
///
/// Errors map to [`ExecError::ExecInvalid`] so the lifecycle surfaces a
/// single halt variant rather than leaking the gitignore error taxonomy.
pub(crate) fn apply_gitignore(ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<(), ExecError> {
    let authored = read_gitignore_patterns(pack).unwrap_or_default();
    let target = gitignore_target(ctx);
    // Defaults first so the generated block is stable regardless of
    // authored content; de-dup if an author explicitly lists one of the
    // defaults (e.g. `.grex-lock`) so we never double-emit.
    let mut merged: Vec<&str> = DEFAULT_MANAGED_GITIGNORE_PATTERNS.to_vec();
    for p in &authored {
        if !merged.contains(&p.as_str()) {
            merged.push(p.as_str());
        }
    }
    gitignore::upsert_managed_block(&target, &pack.name, &merged)
        .map_err(|e| ExecError::ExecInvalid(format!("gitignore upsert failed: {e}")))
}

/// Remove the `x-gitignore` managed block for `pack`. No-op when the
/// file is absent or the block is not present. Called on every
/// teardown regardless of whether the manifest still carries the
/// extension, so a pack whose author removed the extension before
/// running teardown still gets its prior block cleaned.
fn retire_gitignore(ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<(), ExecError> {
    let target = gitignore_target(ctx);
    gitignore::remove_managed_block(&target, &pack.name)
        .map_err(|e| ExecError::ExecInvalid(format!("gitignore remove failed: {e}")))
}

/// Uniform registration surface for every pack type.
///
/// Implementations MUST be `Send + Sync` so the registry can be threaded
/// across executor threads without interior locking. Each lifecycle method
/// takes a parsed [`PackManifest`] (not raw YAML) so plugins operate on the
/// same post-validation invariants every executor already enforces.
///
/// # Lifecycle methods
///
/// * [`install`](PackTypePlugin::install) — first-time materialisation.
/// * [`update`](PackTypePlugin::update) — re-apply on pack or input change.
/// * [`teardown`](PackTypePlugin::teardown) — remove the pack's side-effects.
/// * [`sync`](PackTypePlugin::sync) — pull upstream / child packs.
///
/// All four return [`ExecStep`] so the scheduler can surface progress with
/// the same shape M4 executors already emit.
#[async_trait::async_trait]
pub trait PackTypePlugin: Send + Sync {
    /// Short snake_case name matching the `type:` discriminator in
    /// `pack.yaml` (e.g. `"meta"`, `"declarative"`, `"scripted"`). Used as
    /// the key inside [`PackTypeRegistry`].
    fn name(&self) -> &str;

    /// First-time install: materialise every side-effect the pack declares.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError`] on any failure encountered while running the
    /// pack's actions or composing child packs.
    async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;

    /// Re-apply the pack after a change in its manifest or inputs.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError`] on any failure encountered while diffing or
    /// re-running the pack.
    async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;

    /// Reverse everything [`install`](PackTypePlugin::install) did.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError`] if any teardown step fails. The caller decides
    /// whether a partial teardown should abort or continue.
    async fn teardown(&self, ctx: &ExecCtx<'_>, pack: &PackManifest)
        -> Result<ExecStep, ExecError>;

    /// Sync upstream sources (fetch child packs, pull updates, etc.).
    ///
    /// # Errors
    ///
    /// Returns [`ExecError`] on network or git failure, or on manifest
    /// drift that the plugin cannot reconcile automatically.
    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;
}

/// In-process registry mapping pack-type name → plugin.
///
/// The v1 discovery path mirrors [`crate::plugin::Registry`]: callers
/// construct a registry via [`PackTypeRegistry::bootstrap`] (empty in
/// Stage A; Stage B will populate `meta` / `declarative` / `scripted`) or
/// [`PackTypeRegistry::new`] and optionally register further plugins with
/// [`PackTypeRegistry::register`]. External dylib / WASM loading is
/// deferred to v2 per the feat-grex spec.
#[derive(Default)]
pub struct PackTypeRegistry {
    plugins: HashMap<Cow<'static, str>, Box<dyn PackTypePlugin>>,
}

impl std::fmt::Debug for PackTypeRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Mirror `Registry`: surface just the pack-type name inventory
        // rather than requiring `Debug` on `dyn PackTypePlugin`.
        f.debug_struct("PackTypeRegistry")
            .field("plugins", &self.plugins.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl PackTypeRegistry {
    /// Construct an empty registry. Prefer
    /// [`PackTypeRegistry::bootstrap`] unless you need a hand-picked
    /// plugin set (typical for tests).
    #[must_use]
    pub fn new() -> Self {
        Self { plugins: HashMap::new() }
    }

    /// Register `plugin` under its [`PackTypePlugin::name`]. Later
    /// registrations overwrite earlier ones with the same name — the
    /// registry is last-writer-wins so higher-priority plugin collections
    /// can shadow built-ins after [`PackTypeRegistry::bootstrap`].
    pub fn register<P: PackTypePlugin + 'static>(&mut self, plugin: P) {
        let name: Cow<'static, str> = Cow::Owned(plugin.name().to_owned());
        self.plugins.insert(name, Box::new(plugin));
    }

    /// Look up a plugin by name. Returns `None` if nothing is registered
    /// under that name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn PackTypePlugin> {
        self.plugins.get(name).map(std::convert::AsRef::as_ref)
    }

    /// Number of registered plugins.
    #[must_use]
    pub fn len(&self) -> usize {
        self.plugins.len()
    }

    /// Whether no plugins are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.plugins.is_empty()
    }

    /// Build a registry pre-populated with every built-in pack-type
    /// plugin (`meta`, `declarative`, `scripted`).
    ///
    /// Stage B wires the three built-ins here. Consumers that want a
    /// hand-picked subset (typical for tests) should use
    /// [`PackTypeRegistry::new`] plus explicit [`Self::register`] calls instead.
    #[must_use]
    pub fn bootstrap() -> Self {
        let mut reg = Self::new();
        reg.register(MetaPlugin);
        reg.register(DeclarativePlugin);
        reg.register(ScriptedPlugin);
        reg
    }

    /// Register every plugin submitted via [`inventory::submit!`] into the
    /// `PackTypePluginSubmission` collector. Order is linker-defined;
    /// duplicate names follow `register`'s last-writer-wins rule. Safe to
    /// call after [`PackTypeRegistry::bootstrap`] — inventory entries
    /// shadow existing registrations like any other `register` call.
    ///
    /// Only available when the `plugin-inventory` feature is enabled.
    #[cfg(feature = "plugin-inventory")]
    pub fn register_from_inventory(&mut self) {
        for sub in inventory::iter::<PackTypePluginSubmission> {
            let plugin = (sub.factory)();
            let name: Cow<'static, str> = Cow::Owned(plugin.name().to_owned());
            self.plugins.insert(name, plugin);
        }
    }

    /// Build a registry populated exclusively from
    /// [`inventory::submit!`] entries. Equivalent to
    /// `let mut r = PackTypeRegistry::new(); r.register_from_inventory(); r`.
    ///
    /// Only available when the `plugin-inventory` feature is enabled.
    #[cfg(feature = "plugin-inventory")]
    #[must_use]
    pub fn bootstrap_from_inventory() -> Self {
        let mut reg = Self::new();
        reg.register_from_inventory();
        reg
    }
}

/// Submission record for compile-time pack-type plugin collection via
/// `inventory`.
///
/// Stage B built-ins (`meta` / `declarative` / `scripted`) will ship an
/// `inventory::submit!` block (gated by the `plugin-inventory` feature)
/// pointing at this type, so a consumer opting into `plugin-inventory`
/// can construct a `PackTypeRegistry` purely from linker-time
/// registrations.
#[cfg(feature = "plugin-inventory")]
#[non_exhaustive]
pub struct PackTypePluginSubmission {
    /// Factory producing a boxed plugin instance. Invoked once per
    /// submission during [`PackTypeRegistry::register_from_inventory`].
    pub factory: fn() -> Box<dyn PackTypePlugin>,
}

#[cfg(feature = "plugin-inventory")]
impl PackTypePluginSubmission {
    /// Construct a submission from a plugin factory. Prefer this over
    /// struct-literal syntax so future fields can be added without
    /// breaking downstream `inventory::submit!` sites (the type is
    /// `#[non_exhaustive]`).
    #[must_use]
    pub const fn new(factory: fn() -> Box<dyn PackTypePlugin>) -> Self {
        Self { factory }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::collect!(PackTypePluginSubmission);

// ---------------------------------------------------------------- builtins
//
// Three zero-sized unit structs, one per built-in `type:` discriminator.
// They colocate here (rather than in a sibling `grex-plugins-builtin`
// crate) for the same reason the action built-ins do — avoiding a circular
// dependency and keeping the registry bootstrap path inside `grex-core`.

use std::path::{Path, PathBuf};

use crate::execute::{ExecResult, Platform, PredicateOutcome, StepKind};
use crate::pack::{ChildRef, ExecOnFail, RequireOnFail};
use crate::tree::{FsPackLoader, PackLoader};

/// Load a child pack's manifest from disk.
///
/// Stage C helper used by [`MetaPlugin`] when it needs to peek at a
/// child's `pack.yaml` without going through the full [`crate::tree::Walker`]
/// pipeline (the walker already walked every child during sync setup — this
/// helper is for drivers that need to inspect a child manifest outside the
/// graph, e.g. future per-child selective lifecycle dispatch). Resolves
/// the on-disk directory as `<ctx.workspace>/<child.effective_path()>`
/// and delegates to [`FsPackLoader`] for the actual parse.
///
/// # Errors
///
/// Returns [`ExecError::ExecInvalid`] when the manifest cannot be read
/// or parsed. The error carries the resolved path so callers can render
/// actionable diagnostics.
pub fn load_child_manifest(ctx: &ExecCtx<'_>, child: &ChildRef) -> Result<PackManifest, ExecError> {
    let dir = ctx.workspace.join(child.effective_path());
    load_child_manifest_from(&dir)
}

/// Lower-level variant of [`load_child_manifest`] that takes an explicit
/// directory. Exposed so tests and alternative drivers can exercise the
/// loader without constructing a full [`ExecCtx`].
///
/// # Errors
///
/// Same taxonomy as [`load_child_manifest`].
pub fn load_child_manifest_from(dir: &Path) -> Result<PackManifest, ExecError> {
    let loader = FsPackLoader::new();
    loader.load(dir).map_err(|e| {
        ExecError::ExecInvalid(format!("child manifest load failed at {}: {e}", dir.display()))
    })
}

/// Build a [`ExecStep`] with [`ExecResult::NoOp`] and a `Require` envelope
/// under `action_name`. Used by pack-type drivers when a lifecycle hook has
/// no work to do (empty children, empty actions, missing script hook).
fn noop_step(action_name: &'static str) -> ExecStep {
    ExecStep {
        action_name: Cow::Borrowed(action_name),
        result: ExecResult::NoOp,
        details: StepKind::Require {
            outcome: PredicateOutcome::Satisfied,
            on_fail: RequireOnFail::Skip,
        },
    }
}

/// Which lifecycle method a [`MetaPlugin`] recursion invokes on each child.
///
/// Distinguishes the four entry points so `recurse_children` can dispatch
/// through a single helper regardless of which outer lifecycle is running.
#[derive(Debug, Clone, Copy)]
enum MetaLifecycle {
    Install,
    Update,
    Teardown,
    Sync,
}

/// Built-in driver for `type: meta` packs.
///
/// Meta packs compose child packs and contain no actions of their own.
/// M5-2c wires real recursion: for each [`ChildRef`] in
/// [`PackManifest::children`], the plugin resolves the child's on-disk
/// directory (`<ctx.pack_root>/<child.effective_path()>`), loads the
/// child manifest via [`load_child_manifest_from`], looks up the child
/// pack-type plugin in [`ExecCtx::pack_type_registry`], and dispatches
/// the matching lifecycle method.
///
/// Cycle detection canonicalises each child path before insert into
/// [`ExecCtx::visited_meta`]; a re-entry yields
/// [`ExecError::MetaCycle`]. The walker already rejects structural
/// cycles at tree-walk time — this guards the registry-dispatch path
/// as defence-in-depth.
///
/// Empty `children:` yields a single `noop_step("meta")` so callers can
/// distinguish "ran over zero children" from an execution error.
#[derive(Debug, Default, Clone, Copy)]
pub struct MetaPlugin;

impl MetaPlugin {
    const NAME: &'static str = "meta";

    /// Resolve the child pack root on disk relative to `ctx.pack_root`.
    fn child_root(ctx: &ExecCtx<'_>, child: &ChildRef) -> PathBuf {
        ctx.pack_root.join(child.effective_path())
    }

    /// Build the aggregate `StepKind::When` envelope MetaPlugin emits in
    /// walker-driven mode (the sync driver walks children in post-order
    /// itself, so MetaPlugin does not recurse — it just surfaces a
    /// composition-shape step so audit tooling can see the child set).
    /// Mirrors the M5-1 `compose` behaviour verbatim.
    fn synthesis_envelope<'c, I>(children: I) -> ExecStep
    where
        I: Iterator<Item = &'c ChildRef>,
    {
        let nested: Vec<ExecStep> = children
            .map(|c| ExecStep {
                action_name: Cow::Owned(c.effective_path()),
                result: ExecResult::NoOp,
                details: StepKind::Require {
                    outcome: PredicateOutcome::Satisfied,
                    on_fail: RequireOnFail::Skip,
                },
            })
            .collect();
        if nested.is_empty() {
            return noop_step(Self::NAME);
        }
        ExecStep {
            action_name: Cow::Borrowed(Self::NAME),
            result: ExecResult::NoOp,
            details: StepKind::When { branch_taken: true, nested_steps: nested },
        }
    }

    /// Canonicalise `path` if possible; fall back to the raw input when
    /// canonicalisation fails (missing directory, permission error).
    /// Using a best-effort canonicalisation keeps cycle detection robust
    /// against symlinked pack paths while still giving downstream errors
    /// a usable path string when the child does not yet exist on disk.
    fn canonical_or_raw(path: &Path) -> PathBuf {
        std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    }

    /// Recurse into every child in iteration order, dispatching
    /// `lifecycle` on each. Aborts on the first child error so a
    /// failing sibling's descendants are not touched (matches
    /// `run_declarative_actions` halt-on-first behaviour).
    ///
    /// Returns a composed [`StepKind::When`] envelope aggregating one
    /// nested [`ExecStep`] per child. Empty `children:` yields
    /// `noop_step("meta")`.
    async fn recurse_children<'c, I>(
        ctx: &ExecCtx<'_>,
        children: I,
        lifecycle: MetaLifecycle,
    ) -> Result<ExecStep, ExecError>
    where
        I: Iterator<Item = &'c ChildRef>,
    {
        let mut nested: Vec<ExecStep> = Vec::new();
        for child in children {
            let step = Self::recurse_one(ctx, child, lifecycle).await?;
            nested.push(step);
        }
        if nested.is_empty() {
            return Ok(noop_step(Self::NAME));
        }
        Ok(ExecStep {
            action_name: Cow::Borrowed(Self::NAME),
            result: ExecResult::NoOp,
            details: StepKind::When { branch_taken: true, nested_steps: nested },
        })
    }

    /// Dispatch a single child: resolve path, pre-flight cycle-check
    /// against the visited set (without mutating it), load manifest,
    /// look up plugin, invoke the matching lifecycle.
    ///
    /// Post feat-m6-2: the actual `insert` into `visited_meta` moved
    /// into the dispatched plugin's own `register_self_in_visited` so
    /// the cycle-detection guard and the per-pack lock guard live at
    /// the same scope. Here we only *probe* — if the canonical path
    /// is already in the set, we halt before even touching the
    /// child's per-pack lock.
    async fn recurse_one(
        ctx: &ExecCtx<'_>,
        child: &ChildRef,
        lifecycle: MetaLifecycle,
    ) -> Result<ExecStep, ExecError> {
        let child_root = Self::child_root(ctx, child);
        let canonical = Self::canonical_or_raw(&child_root);
        if let Some(visited) = ctx.visited_meta {
            let guard =
                visited.lock().map_err(|_| ExecError::MetaCycle { path: canonical.clone() })?;
            if guard.contains(&canonical) {
                return Err(ExecError::MetaCycle { path: canonical });
            }
        }
        Self::dispatch_child(ctx, &child_root, lifecycle).await
    }

    /// Load the child manifest, look up the pack-type plugin, and call
    /// the matching lifecycle method.
    async fn dispatch_child(
        ctx: &ExecCtx<'_>,
        child_root: &Path,
        lifecycle: MetaLifecycle,
    ) -> Result<ExecStep, ExecError> {
        let child_manifest = load_child_manifest_from(child_root)?;
        let registry = ctx.pack_type_registry.ok_or_else(|| {
            ExecError::ExecInvalid(
                "meta plugin requires ctx.pack_type_registry for child dispatch".to_string(),
            )
        })?;
        let type_tag = child_manifest.r#type.as_str();
        let plugin = registry
            .get(type_tag)
            .ok_or_else(|| ExecError::UnknownPackType { requested: type_tag.to_string() })?;

        // Rebind pack_root to the child directory; everything else
        // threads through unchanged (vars, workspace, platform,
        // registries, visited_meta).
        let child_ctx = ExecCtx {
            vars: ctx.vars,
            pack_root: child_root,
            workspace: ctx.workspace,
            platform: ctx.platform,
            registry: ctx.registry,
            pack_type_registry: ctx.pack_type_registry,
            visited_meta: ctx.visited_meta,
            // feat-m6-1: thread the scheduler handle through meta
            // recursion so child packs share the parent's permit pool.
            scheduler: ctx.scheduler,
        };

        match lifecycle {
            MetaLifecycle::Install => plugin.install(&child_ctx, &child_manifest).await,
            MetaLifecycle::Update => plugin.update(&child_ctx, &child_manifest).await,
            MetaLifecycle::Teardown => plugin.teardown(&child_ctx, &child_manifest).await,
            MetaLifecycle::Sync => plugin.sync(&child_ctx, &child_manifest).await,
        }
    }
}

#[async_trait::async_trait]
impl PackTypePlugin for MetaPlugin {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        // feat-m6-2 lock prologue — tier 2 permit THEN tier 3 pack lock.
        // `_permit` holds the semaphore slot; `_plock_guard` holds the
        // kernel flock. Both release on Drop at end-of-scope.
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        if ctx.visited_meta.is_some() {
            // feat-m6 H9: release tier/lock guards before child recursion so
            // children can acquire their own Semaphore→PerPack sequence
            // (H5 tier enforcement forbids pushing Semaphore while a
            // PerPack guard is already on the stack). `_own_cycle_guard`
            // stays live across recursion — that is the point of cycle
            // detection. Drop order: pack-lock hold, PerPack tier guard,
            // Semaphore tier guard, then the scheduler permit.
            drop(_plock_hold);
            drop(_tier_pack);
            drop(_tier_sema);
            drop(_permit);
            Self::recurse_children(ctx, pack.children.iter(), MetaLifecycle::Install).await
        } else {
            // Walker-driven path: the outer sync driver walks children
            // in post-order itself, so MetaPlugin only emits an
            // aggregate synthesis envelope (matching M5-1 behaviour).
            // Direct callers that want real recursion attach
            // `visited_meta` via [`ExecCtx::with_visited_meta`].
            Ok(Self::synthesis_envelope(pack.children.iter()))
        }
    }

    async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        if ctx.visited_meta.is_some() {
            // feat-m6 H9: release tier/lock guards before child recursion
            // (see install() for rationale).
            drop(_plock_hold);
            drop(_tier_pack);
            drop(_tier_sema);
            drop(_permit);
            Self::recurse_children(ctx, pack.children.iter(), MetaLifecycle::Update).await
        } else {
            Ok(Self::synthesis_envelope(pack.children.iter()))
        }
    }

    async fn teardown(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        // Teardown walks children in reverse install order so a child
        // composed last is torn down first — mirrors the
        // `reverse(actions)` default for declarative teardown (R-M5-11).
        let step = if ctx.visited_meta.is_some() {
            // feat-m6 H9: release tier/lock guards before child recursion
            // (see install() for rationale). Guards are dropped even on
            // the teardown path; `retire_gitignore` below runs unguarded
            // because the method is returning and no further pack-scope
            // work happens after the children complete.
            drop(_plock_hold);
            drop(_tier_pack);
            drop(_tier_sema);
            drop(_permit);
            // `?` propagates: if any child teardown fails the `?`
            // short-circuits BEFORE `retire_gitignore` runs. On
            // partial teardown the block stays to advertise the
            // remaining patterns — operators can retry teardown
            // once they've resolved the child failure.
            Self::recurse_children(ctx, pack.children.iter().rev(), MetaLifecycle::Teardown).await?
        } else {
            Self::synthesis_envelope(pack.children.iter().rev())
        };
        // Retire the gitignore block AFTER all children teardown
        // successfully (recurse_children path) or after the walker
        // has already driven child teardown in reverse post-order
        // (synthesis_envelope path). In both cases a child failure
        // halts the pipeline before this retire executes.
        retire_gitignore(ctx, pack)?;
        Ok(step)
    }

    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        if ctx.visited_meta.is_some() {
            // feat-m6 H9: release tier/lock guards before child recursion
            // (see install() for rationale).
            drop(_plock_hold);
            drop(_tier_pack);
            drop(_tier_sema);
            drop(_permit);
            Self::recurse_children(ctx, pack.children.iter(), MetaLifecycle::Sync).await
        } else {
            Ok(Self::synthesis_envelope(pack.children.iter()))
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PackTypePluginSubmission::new(|| Box::new(MetaPlugin)));

/// Built-in driver for `type: declarative` packs.
///
/// Iterates [`PackManifest::actions`] in order and dispatches each through
/// the action [`crate::plugin::Registry`] attached to
/// [`ExecCtx::registry`]. On the first failure the chain aborts and the
/// error is propagated. The returned [`ExecStep`] is the **last** step
/// produced, matching the M4 FsExecutor convention (callers that need the
/// full sequence iterate themselves — the pack-type trait surface
/// deliberately returns a single aggregate step).
///
/// # Stage B teardown stub
///
/// [`teardown`](DeclarativePlugin::teardown) is a deliberate no-op stub:
/// reading `pack.teardown` (the explicit author-provided sequence) and
/// auto-reversing `actions` when absent is M5-2 scope. Stage B keeps the
/// shape honest so tests can exercise install/update/sync without pulling
/// teardown semantics forward prematurely.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeclarativePlugin;

impl DeclarativePlugin {
    const NAME: &'static str = "declarative";

    /// Dispatch every action in `pack.actions` through `ctx.registry`.
    /// Returns the last produced [`ExecStep`], or a `noop_step` if the
    /// pack contained no actions.
    fn run_actions(ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        Self::run_action_slice(ctx, &pack.actions)
    }

    /// Dispatch every action in `actions` through `ctx.registry`.
    /// Factored out of [`run_actions`] so teardown — which may feed
    /// either `pack.teardown` directly or a synthesized reverse list —
    /// reuses the same dispatch loop. Returns the last produced
    /// [`ExecStep`], or a `noop_step` if `actions` was empty.
    fn run_action_slice(ctx: &ExecCtx<'_>, actions: &[Action]) -> Result<ExecStep, ExecError> {
        let Some(registry) = ctx.registry else {
            return Err(ExecError::UnknownAction(
                "declarative plugin requires ctx.registry".to_string(),
            ));
        };
        let mut last: Option<ExecStep> = None;
        for action in actions {
            let plugin = registry
                .get(action.name())
                .ok_or_else(|| ExecError::UnknownAction(action.name().to_string()))?;
            last = Some(plugin.execute(action, ctx)?);
        }
        Ok(last.unwrap_or_else(|| noop_step(Self::NAME)))
    }

    /// Synthesize the inverse of `action` for auto-reverse teardown.
    ///
    /// Returns `Some(inverse)` when a natural inverse exists. Returns
    /// `None` when no inverse is defined — the caller emits a `NoOp`
    /// warning step and continues. R-M5-09.
    ///
    /// Supported inverses:
    ///
    /// * `mkdir` → `rmdir` with `force: false` so we only remove
    ///   directories the author's install created-and-left-empty.
    /// * `symlink` → `unlink` targeting the recorded `dst`. The
    ///   `fs_unlink` wet-run checks that `dst` IS a symlink before
    ///   removing it so a misdirected teardown cannot clobber
    ///   operator-managed files.
    /// * `when` → `when` with the same condition but `actions`
    ///   recursively inverted. Preserving the gate is load-bearing:
    ///   a platform-gated install must tear down only on the same
    ///   platform.
    ///
    /// `env`, `require`, and `exec` have no safe auto-reverse: we
    /// cannot distinguish operator-managed state from pack-managed
    /// state without richer metadata. Authors who need precise
    /// cleanup should supply an explicit `teardown:` block.
    fn inverse_of(action: &Action) -> Option<Action> {
        match action {
            Action::Mkdir(m) => {
                Some(Action::Rmdir(crate::pack::RmdirArgs::new(m.path.clone(), false, false)))
            }
            Action::Symlink(s) => Some(Action::Unlink(crate::pack::UnlinkArgs::new(s.dst.clone()))),
            Action::When(w) => {
                // Recursively invert `actions`; nested entries with no
                // inverse (e.g. an `env` inside the gate) drop out of
                // the reversed list rather than emitting a warning step
                // here — the caller's auto-reverse loop handles the
                // outermost no-inverse emission, and the inner gate's
                // purpose is just to mirror install ordering.
                let inner: Vec<Action> =
                    w.actions.iter().rev().filter_map(Self::inverse_of).collect();
                Some(Action::When(crate::pack::WhenSpec::new(
                    w.os,
                    w.all_of.clone(),
                    w.any_of.clone(),
                    w.none_of.clone(),
                    inner,
                )))
            }
            _ => None,
        }
    }

    /// Build an auto-reverse teardown step list from `pack.actions` in
    /// reverse order. Actions with no defined inverse become a `NoOp`
    /// warning step carrying the action kind tag. Actions with an
    /// inverse are dispatched through `ctx.registry` exactly like
    /// install. Halts on the first error (install-consistent).
    fn run_auto_reverse(ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let Some(registry) = ctx.registry else {
            return Err(ExecError::UnknownAction(
                "declarative plugin requires ctx.registry".to_string(),
            ));
        };
        let mut last: Option<ExecStep> = None;
        for action in pack.actions.iter().rev() {
            match Self::inverse_of(action) {
                Some(inv) => {
                    let plugin = registry
                        .get(inv.name())
                        .ok_or_else(|| ExecError::UnknownAction(inv.name().to_string()))?;
                    last = Some(plugin.execute(&inv, ctx)?);
                }
                None => {
                    // No inverse: synthesize a NoOp warning step so
                    // operators can see the skipped action in the
                    // report. action_name carries the original tag so
                    // the audit trail points at the uncleanable entry.
                    last = Some(ExecStep {
                        action_name: Cow::Owned(format!("teardown:no-inverse:{}", action.name())),
                        result: ExecResult::NoOp,
                        details: StepKind::Require {
                            outcome: PredicateOutcome::Satisfied,
                            on_fail: RequireOnFail::Skip,
                        },
                    });
                }
            }
        }
        Ok(last.unwrap_or_else(|| noop_step(Self::NAME)))
    }
}

#[async_trait::async_trait]
impl PackTypePlugin for DeclarativePlugin {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        Self::run_actions(ctx, pack)
    }

    async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        // Declarative actions are idempotent by contract, so update ==
        // re-install. The M4 FsExecutor guarantees "already satisfied"
        // short-circuits for symlink/env/mkdir.
        apply_gitignore(ctx, pack)?;
        Self::run_actions(ctx, pack)
    }

    async fn teardown(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        // R-M5-09: honour explicit `pack.teardown` when `Some`; fall
        // back to auto-reverse over `pack.actions` when `None`.
        // `Some(vec![])` is an explicit no-op (distinct from absent).
        let step = match &pack.teardown {
            Some(actions) => Self::run_action_slice(ctx, actions)?,
            None => Self::run_auto_reverse(ctx, pack)?,
        };
        retire_gitignore(ctx, pack)?;
        Ok(step)
    }

    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        // Sync mirrors install at the declarative layer; upstream fetch
        // is a meta-pack concern (child-pack git pulls in M5-2+).
        apply_gitignore(ctx, pack)?;
        Self::run_actions(ctx, pack)
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PackTypePluginSubmission::new(|| Box::new(DeclarativePlugin)));

/// Built-in driver for `type: scripted` packs.
///
/// Each lifecycle method shells out to a matching hook under the pack's
/// `.grex/hooks/` directory:
///
/// | Lifecycle | Unix script         | Windows script       |
/// |-----------|---------------------|----------------------|
/// | install   | `setup.sh`          | `setup.ps1`          |
/// | update    | `update.sh`         | `update.ps1`         |
/// | teardown  | `teardown.sh`       | `teardown.ps1`       |
/// | sync      | `sync.sh`           | `sync.ps1`           |
///
/// The hook selection uses [`ExecCtx::platform`] so tests can override it
/// to exercise the cross-OS branches deterministically.
///
/// * Missing hook → [`ExecResult::NoOp`] (returning `Ok`). Pack authors
///   explicitly opt-in per lifecycle by creating the matching script.
/// * Non-zero exit → [`ExecError::ExecNonZero`] carrying captured
///   stderr (truncated to
///   [`crate::execute::EXEC_STDERR_CAPTURE_MAX`] bytes).
/// * Spawn failure (program not found, ACL denial, ...) →
///   [`ExecError::ExecSpawnFailed`].
#[derive(Debug, Default, Clone, Copy)]
pub struct ScriptedPlugin;

impl ScriptedPlugin {
    const NAME: &'static str = "scripted";

    /// Resolve the absolute path to `<pack_root>/.grex/hooks/<stem>.<ext>`,
    /// picking `ps1` on Windows and `sh` elsewhere.
    fn hook_path(ctx: &ExecCtx<'_>, stem: &str) -> PathBuf {
        let ext = match ctx.platform {
            Platform::Windows => "ps1",
            _ => "sh",
        };
        ctx.pack_root.join(".grex").join("hooks").join(format!("{stem}.{ext}"))
    }

    /// Spawn the hook at `script` via the OS-appropriate interpreter.
    /// On Windows the PowerShell 7 CLI `pwsh` is used with
    /// `-NoProfile -File <script>`; on Unix the script is invoked
    /// directly so the shebang line controls interpreter selection (the
    /// packaging review in M5-2 will revisit whether to force `/bin/sh`).
    async fn spawn(script: &PathBuf, platform: Platform) -> Result<ExecStep, ExecError> {
        use tokio::process::Command;

        let cmdline = script.display().to_string();
        let mut cmd = match platform {
            Platform::Windows => {
                let mut c = Command::new("pwsh");
                c.arg("-NoProfile").arg("-File").arg(script);
                c
            }
            _ => Command::new(script),
        };

        let output = cmd.output().await.map_err(|e| ExecError::ExecSpawnFailed {
            command: cmdline.clone(),
            detail: e.to_string(),
        })?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let truncated = if stderr.len() > crate::execute::EXEC_STDERR_CAPTURE_MAX {
                stderr[..crate::execute::EXEC_STDERR_CAPTURE_MAX].to_string()
            } else {
                stderr.into_owned()
            };
            return Err(ExecError::ExecNonZero {
                status: output.status.code().unwrap_or(-1),
                command: cmdline,
                stderr: truncated,
            });
        }

        Ok(ExecStep {
            action_name: Cow::Borrowed(Self::NAME),
            result: ExecResult::PerformedChange,
            details: StepKind::Exec {
                cmdline,
                cwd: None,
                on_fail: ExecOnFail::Error,
                shell: false,
            },
        })
    }

    /// Run the `<stem>` hook for this pack. Missing hook → `Ok(noop)`.
    async fn run_hook(ctx: &ExecCtx<'_>, stem: &str) -> Result<ExecStep, ExecError> {
        let path = Self::hook_path(ctx, stem);
        // `tokio::fs::metadata` distinguishes "does not exist" (valid
        // no-op) from "exists but ACL denied" (spawn will surface a
        // typed error).
        match tokio::fs::metadata(&path).await {
            Ok(_) => Self::spawn(&path, ctx.platform).await,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(noop_step(Self::NAME)),
            Err(e) => Err(ExecError::ExecSpawnFailed {
                command: path.display().to_string(),
                detail: e.to_string(),
            }),
        }
    }
}

#[async_trait::async_trait]
impl PackTypePlugin for ScriptedPlugin {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        Self::run_hook(ctx, "setup").await
    }

    async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        Self::run_hook(ctx, "update").await
    }

    async fn teardown(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        // R-M5-10: run `teardown.{sh,ps1}`, then retire the managed
        // gitignore block. Hook errors propagate before the block is
        // removed so a failing script does not "lose" the block (the
        // next teardown retry will retry both steps).
        let step = Self::run_hook(ctx, "teardown").await?;
        retire_gitignore(ctx, pack)?;
        Ok(step)
    }

    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        let _tier_sema = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::Semaphore);
        let _permit = acquire_scheduler_permit(ctx).await?;
        // Register this pack root in the cycle-detection set BEFORE
        // acquiring the per-pack lock so a recursive dispatch that
        // resolves back to the same canonical path halts via
        // MetaCycle (checked in `recurse_one`) instead of hanging at
        // the non-reentrant `acquire_async` mutex.
        let _own_cycle_guard = register_self_in_visited(ctx)?;
        let _tier_pack = crate::pack_lock::TierGuard::push(crate::pack_lock::Tier::PerPack);
        let _plock_hold = PackLock::open(ctx.pack_root)
            .map_err(map_pack_lock_err)?
            .acquire_async()
            .await
            .map_err(map_pack_lock_err)?;
        apply_gitignore(ctx, pack)?;
        Self::run_hook(ctx, "sync").await
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PackTypePluginSubmission::new(|| Box::new(ScriptedPlugin)));

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execute::{ExecResult, PredicateOutcome, StepKind};
    use crate::pack::RequireOnFail;
    use std::borrow::Cow;

    /// Zero-sized test plugin returning an Ok stub for every lifecycle
    /// method. Lives in the test module to avoid leaking a public dummy
    /// into the crate surface.
    #[derive(Default)]
    struct DummyPackType;

    impl DummyPackType {
        const NAME: &'static str = "dummy";

        fn ok_step() -> ExecStep {
            // Stage A plugins don't produce real work yet; the cheapest
            // well-formed step is a NoOp `require`-shaped envelope. Stage
            // B's built-ins will return proper Symlink/Exec/When kinds.
            ExecStep {
                action_name: Cow::Borrowed(Self::NAME),
                result: ExecResult::NoOp,
                details: StepKind::Require {
                    outcome: PredicateOutcome::Satisfied,
                    on_fail: RequireOnFail::Skip,
                },
            }
        }
    }

    #[async_trait::async_trait]
    impl PackTypePlugin for DummyPackType {
        fn name(&self) -> &str {
            Self::NAME
        }

        async fn install(
            &self,
            _ctx: &ExecCtx<'_>,
            _pack: &PackManifest,
        ) -> Result<ExecStep, ExecError> {
            Ok(Self::ok_step())
        }

        async fn update(
            &self,
            _ctx: &ExecCtx<'_>,
            _pack: &PackManifest,
        ) -> Result<ExecStep, ExecError> {
            Ok(Self::ok_step())
        }

        async fn teardown(
            &self,
            _ctx: &ExecCtx<'_>,
            _pack: &PackManifest,
        ) -> Result<ExecStep, ExecError> {
            Ok(Self::ok_step())
        }

        async fn sync(
            &self,
            _ctx: &ExecCtx<'_>,
            _pack: &PackManifest,
        ) -> Result<ExecStep, ExecError> {
            Ok(Self::ok_step())
        }
    }

    #[test]
    fn registry_new_is_empty() {
        let reg = PackTypeRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.get("dummy").is_none());
    }

    #[test]
    fn bootstrap_registers_all_three_builtins() {
        // Stage B wires meta/declarative/scripted. Tests guard against
        // accidental re-emptying of bootstrap and against a new built-in
        // being added without a matching name assertion below.
        let reg = PackTypeRegistry::bootstrap();
        assert_eq!(reg.len(), 3);
        for name in ["meta", "declarative", "scripted"] {
            let plugin = reg.get(name).unwrap_or_else(|| panic!("missing built-in `{name}`"));
            assert_eq!(plugin.name(), name);
        }
        assert!(reg.get("unknown").is_none());
    }

    #[cfg(feature = "plugin-inventory")]
    #[test]
    fn bootstrap_from_inventory_registers_all_three_builtins() {
        let reg = PackTypeRegistry::bootstrap_from_inventory();
        assert_eq!(reg.len(), 3);
        for name in ["meta", "declarative", "scripted"] {
            let plugin = reg.get(name).unwrap_or_else(|| panic!("missing built-in `{name}`"));
            assert_eq!(plugin.name(), name);
        }
    }

    #[cfg(feature = "plugin-inventory")]
    #[test]
    fn register_from_inventory_on_empty_registry_produces_three_entries() {
        let mut reg = PackTypeRegistry::new();
        assert!(reg.is_empty());
        reg.register_from_inventory();
        assert_eq!(reg.len(), 3);
        for name in ["meta", "declarative", "scripted"] {
            assert!(reg.get(name).is_some(), "missing built-in `{name}`");
        }
    }

    #[cfg(feature = "plugin-inventory")]
    #[test]
    fn register_from_inventory_twice_dedups_to_three() {
        let mut reg = PackTypeRegistry::new();
        reg.register_from_inventory();
        reg.register_from_inventory();
        assert_eq!(reg.len(), 3);
    }

    #[test]
    fn register_then_get_returns_plugin() {
        let mut reg = PackTypeRegistry::new();
        reg.register(DummyPackType);
        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
        let plugin = reg.get("dummy").expect("registered plugin must be retrievable");
        assert_eq!(plugin.name(), "dummy");
    }

    #[test]
    fn register_is_last_writer_wins() {
        let mut reg = PackTypeRegistry::new();
        reg.register(DummyPackType);
        reg.register(DummyPackType);
        assert_eq!(reg.len(), 1);
    }

    #[tokio::test]
    async fn dummy_plugin_lifecycle_methods_return_ok() {
        use crate::pack;
        use crate::vars::VarEnv;
        use std::path::Path;

        let src = "schema_version: \"1\"\nname: testpack\ntype: declarative\n";
        let pack = pack::parse(src).expect("fixture must parse");
        let vars = VarEnv::default();
        let root = Path::new(".");
        let ctx = ExecCtx::new(&vars, root, root);
        let plugin = DummyPackType;

        assert!(plugin.install(&ctx, &pack).await.is_ok());
        assert!(plugin.update(&ctx, &pack).await.is_ok());
        assert!(plugin.teardown(&ctx, &pack).await.is_ok());
        assert!(plugin.sync(&ctx, &pack).await.is_ok());
    }

    // -------- Stage B built-in smoke tests -------------------------

    /// Parse a minimal manifest of the given `type:` discriminator. The
    /// three Stage B built-ins all accept empty children/actions, so a
    /// single-line fixture is enough to exercise the happy path.
    fn fixture(pack_type: &str) -> PackManifest {
        let src = format!("schema_version: \"1\"\nname: tp\ntype: {pack_type}\n");
        crate::pack::parse(&src).expect("fixture must parse")
    }

    #[tokio::test]
    async fn meta_plugin_name_and_empty_children_install() {
        use crate::vars::VarEnv;
        use std::path::Path;

        let plugin = MetaPlugin;
        assert_eq!(plugin.name(), "meta");

        let pack = fixture("meta");
        let vars = VarEnv::default();
        let root = Path::new(".");
        let ctx = ExecCtx::new(&vars, root, root);
        let step = plugin.install(&ctx, &pack).await.expect("empty-children install OK");
        // Empty children → `noop_step` with Require envelope.
        assert_eq!(step.action_name.as_ref(), "meta");
        assert!(matches!(step.result, ExecResult::NoOp));
    }

    #[tokio::test]
    async fn declarative_plugin_name_and_empty_actions_install() {
        use crate::vars::VarEnv;
        use std::path::Path;
        use std::sync::Arc;

        let plugin = DeclarativePlugin;
        assert_eq!(plugin.name(), "declarative");

        let pack = fixture("declarative");
        let vars = VarEnv::default();
        let root = Path::new(".");
        // DeclarativePlugin requires `ctx.registry` — attach a
        // bootstrapped action registry so the call succeeds even with
        // zero actions (the registry is consulted lazily per action).
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, root, root).with_registry(&action_reg);
        let step = plugin.install(&ctx, &pack).await.expect("empty-actions install OK");
        assert_eq!(step.action_name.as_ref(), "declarative");
        assert!(matches!(step.result, ExecResult::NoOp));
    }

    #[test]
    fn load_child_manifest_from_reads_pack_yaml() {
        use tempfile::TempDir;
        let tmp = TempDir::new().expect("tempdir");
        let dir = tmp.path().join("child");
        std::fs::create_dir_all(dir.join(".grex")).unwrap();
        std::fs::write(
            dir.join(".grex").join("pack.yaml"),
            "schema_version: \"1\"\nname: kid\ntype: declarative\n",
        )
        .unwrap();
        let manifest = load_child_manifest_from(&dir).expect("child manifest loads");
        assert_eq!(manifest.name, "kid");
    }

    #[test]
    fn load_child_manifest_from_missing_dir_errors() {
        let missing = std::path::Path::new("/definitely/not/a/pack/path/123456789");
        let err = load_child_manifest_from(missing).expect_err("missing manifest errors");
        match err {
            ExecError::ExecInvalid(msg) => {
                assert!(msg.contains("child manifest load failed"), "msg: {msg}");
            }
            other => panic!("unexpected error: {other:?}"),
        }
    }

    #[test]
    fn exec_error_meta_cycle_renders_path() {
        let err = ExecError::MetaCycle { path: std::path::PathBuf::from("/tmp/a") };
        let rendered = err.to_string();
        assert!(rendered.contains("meta recursion cycle"), "got: {rendered}");
        assert!(rendered.contains("/tmp/a") || rendered.contains("\\tmp\\a"), "got: {rendered}");
    }

    #[test]
    fn exec_error_unknown_pack_type_renders_name() {
        let err = ExecError::UnknownPackType { requested: "mystery".into() };
        let rendered = err.to_string();
        assert!(rendered.contains("mystery"), "got: {rendered}");
        assert!(rendered.contains("no pack-type plugin"), "got: {rendered}");
    }

    // -------- M5-2b teardown tests ---------------------------------

    /// Parse a manifest from a literal YAML source. Helper for the
    /// teardown/auto-reverse tests below.
    fn parse_pack(src: &str) -> PackManifest {
        crate::pack::parse(src).expect("fixture must parse")
    }

    #[tokio::test]
    async fn declarative_teardown_runs_explicit_block() {
        use crate::vars::VarEnv;
        use std::sync::Arc;
        use tempfile::TempDir;

        // Explicit teardown: install creates two dirs; teardown removes
        // only the second one. The auto-reverse fallback would remove
        // both — so observing a/ still present proves the explicit
        // block was honoured.
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let a = tmp_path.join("a");
        let b = tmp_path.join("b");
        let fwd = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
        let src = format!(
            "schema_version: \"1\"\nname: tp\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n  - mkdir:\n      path: {}\nteardown:\n  - rmdir:\n      path: {}\n",
            fwd(&a), fwd(&b), fwd(&b)
        );
        let pack = parse_pack(&src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path).with_registry(&action_reg);

        let plugin = DeclarativePlugin;
        plugin.install(&ctx, &pack).await.expect("install ok");
        assert!(a.is_dir());
        assert!(b.is_dir());
        plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        // Only b was removed (explicit teardown block, not auto-reverse).
        assert!(a.is_dir(), "explicit teardown should not touch a/");
        assert!(!b.exists(), "explicit teardown should remove b/");
    }

    #[tokio::test]
    async fn declarative_auto_reverse_mkdir_to_rmdir() {
        use crate::vars::VarEnv;
        use std::sync::Arc;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let a = tmp_path.join("auto_a");
        let fwd = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
        // No explicit teardown → auto-reverse. mkdir → rmdir.
        let src = format!(
            "schema_version: \"1\"\nname: tp\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
            fwd(&a)
        );
        let pack = parse_pack(&src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path).with_registry(&action_reg);

        let plugin = DeclarativePlugin;
        plugin.install(&ctx, &pack).await.expect("install ok");
        assert!(a.is_dir());
        plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        assert!(!a.exists(), "auto-reverse mkdir→rmdir should remove dir");
    }

    #[tokio::test]
    async fn declarative_auto_reverse_skips_env_with_noop() {
        use crate::vars::VarEnv;
        use std::sync::Arc;
        use tempfile::TempDir;

        // `env` has no safe auto-reverse (we can't know the prior
        // value to restore). Auto-reverse emits a NoOp step with an
        // annotated action_name and continues.
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let src = "schema_version: \"1\"\nname: tp\ntype: declarative\nactions:\n  - env:\n      name: GREX_TEST_NO_INVERSE\n      value: x\n";
        let pack = parse_pack(src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path).with_registry(&action_reg);

        let plugin = DeclarativePlugin;
        let _ = plugin.install(&ctx, &pack).await;
        let step = plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        assert!(
            step.action_name.as_ref().starts_with("teardown:no-inverse:"),
            "got: {:?}",
            step.action_name
        );
        assert!(matches!(step.result, ExecResult::NoOp));
    }

    #[tokio::test]
    async fn declarative_auto_reverse_reverses_order() {
        use crate::vars::VarEnv;
        use std::sync::Arc;
        use tempfile::TempDir;

        // Install creates a/, then a/inner. Auto-reverse must remove
        // a/inner first then a/ — otherwise rmdir on a/ fails because
        // it is non-empty. Observing both gone proves reverse order.
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let fwd = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/");
        let outer = tmp_path.join("outer");
        let inner = outer.join("inner");
        let src = format!(
            "schema_version: \"1\"\nname: tp\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n  - mkdir:\n      path: {}\n",
            fwd(&outer), fwd(&inner)
        );
        let pack = parse_pack(&src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path).with_registry(&action_reg);

        let plugin = DeclarativePlugin;
        plugin.install(&ctx, &pack).await.expect("install ok");
        assert!(inner.is_dir());
        plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        assert!(!inner.exists());
        assert!(!outer.exists(), "reverse order must remove inner before outer");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scripted_teardown_runs_teardown_sh_on_unix() {
        use crate::vars::VarEnv;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let hooks = tmp_path.join(".grex").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let marker = tmp_path.join("teardown.ran");
        let hook = hooks.join("teardown.sh");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let pack = fixture("scripted");
        let vars = VarEnv::default();
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path);
        let plugin = ScriptedPlugin;
        let step = plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        assert!(marker.exists(), "teardown.sh must have run");
        assert!(matches!(step.result, ExecResult::PerformedChange));
    }

    #[tokio::test]
    async fn scripted_teardown_missing_hook_is_noop() {
        use crate::vars::VarEnv;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let pack = fixture("scripted");
        let vars = VarEnv::default();
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path);
        let plugin = ScriptedPlugin;
        let step = plugin.teardown(&ctx, &pack).await.expect("missing hook is no-op");
        assert!(matches!(step.result, ExecResult::NoOp));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn scripted_teardown_non_zero_exit_surfaces_exec_non_zero() {
        use crate::vars::VarEnv;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let hooks = tmp_path.join(".grex").join("hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        let hook = hooks.join("teardown.sh");
        std::fs::write(&hook, "#!/bin/sh\nexit 7\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();

        let pack = fixture("scripted");
        let vars = VarEnv::default();
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path);
        let plugin = ScriptedPlugin;
        let err = plugin.teardown(&ctx, &pack).await.expect_err("non-zero exit must err");
        match err {
            ExecError::ExecNonZero { status, .. } => assert_eq!(status, 7),
            other => panic!("expected ExecNonZero, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meta_teardown_reverses_children_order_synthesis_mode() {
        use crate::vars::VarEnv;
        use std::path::Path;

        // With no visited_meta, MetaPlugin emits a synthesis envelope
        // over children in REVERSE order. Two children with known
        // paths should appear back-to-front in the nested step list.
        let src = "schema_version: \"1\"\nname: m\ntype: meta\nchildren:\n  - url: https://e.com/a.git\n  - url: https://e.com/b.git\n";
        let pack = parse_pack(src);
        let vars = VarEnv::default();
        let root = Path::new(".");
        let ctx = ExecCtx::new(&vars, root, root);
        let plugin = MetaPlugin;
        let step = plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        // Expect StepKind::When with nested steps in reverse order.
        match step.details {
            StepKind::When { nested_steps, .. } => {
                assert_eq!(nested_steps.len(), 2);
                assert_eq!(nested_steps[0].action_name.as_ref(), "b");
                assert_eq!(nested_steps[1].action_name.as_ref(), "a");
            }
            other => panic!("expected When envelope, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn meta_teardown_cycle_guards_via_visited_meta() {
        use crate::execute::MetaVisitedSet;
        use crate::vars::VarEnv;
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        // Pre-populate visited_meta with the canonical child path so
        // recurse_one short-circuits into MetaCycle before touching
        // the filesystem.
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let child_dir = tmp_path.join("kid");
        std::fs::create_dir_all(child_dir.join(".grex")).unwrap();
        std::fs::write(
            child_dir.join(".grex").join("pack.yaml"),
            "schema_version: \"1\"\nname: kid\ntype: declarative\n",
        )
        .unwrap();

        let canon = std::fs::canonicalize(&child_dir).unwrap_or(child_dir.clone());
        let mut seen: HashSet<std::path::PathBuf> = HashSet::new();
        seen.insert(canon);
        let visited: MetaVisitedSet = Arc::new(Mutex::new(seen));
        let src = "schema_version: \"1\"\nname: parent\ntype: meta\nchildren:\n  - url: https://e.com/kid.git\n";
        let pack = parse_pack(src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let pt_reg = Arc::new(PackTypeRegistry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path)
            .with_registry(&action_reg)
            .with_pack_type_registry(&pt_reg)
            .with_visited_meta(&visited);
        let plugin = MetaPlugin;
        let err = plugin.teardown(&ctx, &pack).await.expect_err("cycle must err");
        assert!(matches!(err, ExecError::MetaCycle { .. }));
    }

    #[tokio::test]
    async fn meta_teardown_aborts_on_first_child_failure() {
        use crate::execute::MetaVisitedSet;
        use crate::vars::VarEnv;
        use std::collections::HashSet;
        use std::sync::{Arc, Mutex};
        use tempfile::TempDir;

        // A child that references an unknown pack type surfaces
        // UnknownPackType; teardown must propagate, not continue.
        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let child_dir = tmp_path.join("kid");
        std::fs::create_dir_all(child_dir.join(".grex")).unwrap();
        // Write a valid declarative manifest but use an UNREGISTERED
        // pack-type registry so UnknownPackType fires.
        std::fs::write(
            child_dir.join(".grex").join("pack.yaml"),
            "schema_version: \"1\"\nname: kid\ntype: declarative\n",
        )
        .unwrap();

        let visited: MetaVisitedSet = Arc::new(Mutex::new(HashSet::new()));
        let src = "schema_version: \"1\"\nname: parent\ntype: meta\nchildren:\n  - url: https://e.com/kid.git\n";
        let pack = parse_pack(src);
        let vars = VarEnv::default();
        let empty_reg = Arc::new(PackTypeRegistry::new());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path)
            .with_pack_type_registry(&empty_reg)
            .with_visited_meta(&visited);
        let plugin = MetaPlugin;
        let err = plugin.teardown(&ctx, &pack).await.expect_err("unknown type halts");
        assert!(matches!(err, ExecError::UnknownPackType { .. }));
    }

    #[tokio::test]
    async fn gitignore_extension_absent_still_emits_default_block() {
        // feat-m6-2: every pack contributes `.grex-lock` to its managed
        // block regardless of whether it declares `x-gitignore`. Prior
        // behaviour (absent extension → no file) changed with the
        // per-pack lock contract — `.grex-lock` must never appear in
        // `git status` even when the author provided no extension.
        use crate::vars::VarEnv;
        use std::sync::Arc;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let src = "schema_version: \"1\"\nname: ng\ntype: declarative\n";
        let pack = parse_pack(src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path).with_registry(&action_reg);
        let plugin = DeclarativePlugin;
        plugin.install(&ctx, &pack).await.expect("install ok");
        let gitig = std::fs::read_to_string(tmp_path.join(".gitignore"))
            .expect("default managed block file is written");
        assert!(gitig.contains("# >>> grex:ng >>>"), "managed block header present");
        assert!(gitig.contains(".grex-lock"), "default managed pattern present");
    }

    #[tokio::test]
    async fn gitignore_extension_present_writes_managed_block() {
        use crate::vars::VarEnv;
        use std::sync::Arc;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        let tmp_path = tmp.path().to_path_buf();
        let src = "schema_version: \"1\"\nname: gi\ntype: declarative\nx-gitignore:\n  - target/\n  - \"*.log\"\n";
        let pack = parse_pack(src);
        let vars = VarEnv::default();
        let action_reg = Arc::new(crate::plugin::Registry::bootstrap());
        let ctx = ExecCtx::new(&vars, &tmp_path, &tmp_path).with_registry(&action_reg);
        let plugin = DeclarativePlugin;
        plugin.install(&ctx, &pack).await.expect("install ok");
        let gitig = std::fs::read_to_string(tmp_path.join(".gitignore")).unwrap();
        assert!(gitig.contains("# >>> grex:gi >>>"));
        assert!(gitig.contains("target/"));
        assert!(gitig.contains("*.log"));
        // Teardown removes the block.
        plugin.teardown(&ctx, &pack).await.expect("teardown ok");
        let after = std::fs::read_to_string(tmp_path.join(".gitignore")).unwrap_or_default();
        assert!(!after.contains("grex:gi"), "teardown must remove block: {after}");
    }

    // -------- end M5-2b tests --------------------------------------

    #[tokio::test]
    async fn scripted_plugin_name_and_missing_hook_install_noop() {
        use crate::vars::VarEnv;
        use tempfile::TempDir;

        let plugin = ScriptedPlugin;
        assert_eq!(plugin.name(), "scripted");

        let pack = fixture("scripted");
        let vars = VarEnv::default();
        // `tempfile` guarantees a fresh directory with no `.grex/hooks/`
        // entries, so every lifecycle hook lookup returns NotFound and
        // the plugin short-circuits to `noop_step`.
        let tmp = TempDir::new().expect("tempdir");
        let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path());
        let step =
            plugin.install(&ctx, &pack).await.expect("missing-hook install is a valid no-op");
        assert_eq!(step.action_name.as_ref(), "scripted");
        assert!(matches!(step.result, ExecResult::NoOp));
    }
}
