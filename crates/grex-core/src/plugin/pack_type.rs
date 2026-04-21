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
use crate::pack::PackManifest;

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

/// Built-in driver for `type: meta` packs.
///
/// Meta packs compose child packs and contain no actions of their own.
/// Stage B enumerates [`PackManifest::children`] but does **not** load
/// each child's `pack.yaml` or dispatch through the
/// [`PackTypeRegistry`] — a child-pack loader helper does not yet exist
/// in the crate. Stage C will add that helper (or wire an existing one)
/// and swap this enumeration for real recursion via
/// [`ExecCtx::pack_type_registry`].
///
/// For now every lifecycle method walks children in-order (reverse order
/// for [`teardown`](MetaPlugin::teardown)) and emits one synthetic
/// [`ExecStep`] per child under a [`StepKind::When`] envelope so downstream
/// audit tooling sees the composition shape, and returns the envelope as
/// the aggregated step. An empty `children:` list yields a single
/// `noop_step` so callers can distinguish "ran over zero children" from
/// an execution error.
#[derive(Debug, Default, Clone, Copy)]
pub struct MetaPlugin;

impl MetaPlugin {
    const NAME: &'static str = "meta";

    /// Synthesise a [`StepKind::When`] envelope enumerating `children` in
    /// the provided iteration order. Each child becomes one nested
    /// `noop_step` tagged with the child's [`ChildRef::effective_path`]
    /// so audit logs can point at the on-disk directory even before Stage
    /// C threads real dispatch.
    fn compose<'c, I>(children: I) -> ExecStep
    where
        I: Iterator<Item = &'c ChildRef>,
    {
        let nested: Vec<ExecStep> = children
            .map(|c| ExecStep {
                // `effective_path` allocates once per child — acceptable
                // since pack children are on the order of tens, not
                // thousands, and the path is the only stable human-
                // readable label at this layer.
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
}

#[async_trait::async_trait]
impl PackTypePlugin for MetaPlugin {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn install(
        &self,
        _ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        Ok(Self::compose(pack.children.iter()))
    }

    async fn update(&self, _ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        Ok(Self::compose(pack.children.iter()))
    }

    async fn teardown(
        &self,
        _ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        // Teardown walks children in reverse install order so a child
        // that was composed last is torn down first — mirrors the
        // "reverse(actions)" default for declarative teardown.
        Ok(Self::compose(pack.children.iter().rev()))
    }

    async fn sync(&self, _ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        Ok(Self::compose(pack.children.iter()))
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
        let Some(registry) = ctx.registry else {
            // Callers that invoke a declarative driver without attaching
            // an action registry are a programming error (Stage B only —
            // Stage C's executor-side dispatch swap will always attach
            // one). Surface as UnknownAction so diagnostics point at the
            // missing registry rather than a missing plugin.
            return Err(ExecError::UnknownAction(
                "declarative plugin requires ctx.registry".to_string(),
            ));
        };
        let mut last: Option<ExecStep> = None;
        for action in &pack.actions {
            let plugin = registry
                .get(action.name())
                .ok_or_else(|| ExecError::UnknownAction(action.name().to_string()))?;
            last = Some(plugin.execute(action, ctx)?);
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
        Self::run_actions(ctx, pack)
    }

    async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        // Declarative actions are idempotent by contract, so update ==
        // re-install. The M4 FsExecutor guarantees "already satisfied"
        // short-circuits for symlink/env/mkdir.
        Self::run_actions(ctx, pack)
    }

    async fn teardown(
        &self,
        _ctx: &ExecCtx<'_>,
        _pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        // TODO(M5-2): honour `pack.teardown` when `Some`, auto-reverse
        // `pack.actions` when `None`. Stage B stubs this out so the
        // trait surface stays complete without pulling teardown
        // semantics into M5-1.
        Ok(noop_step(Self::NAME))
    }

    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
        // Sync mirrors install at the declarative layer; upstream fetch
        // is a meta-pack concern (child-pack git pulls in M5-2+).
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

    async fn install(
        &self,
        ctx: &ExecCtx<'_>,
        _pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        Self::run_hook(ctx, "setup").await
    }

    async fn update(&self, ctx: &ExecCtx<'_>, _pack: &PackManifest) -> Result<ExecStep, ExecError> {
        Self::run_hook(ctx, "update").await
    }

    async fn teardown(
        &self,
        ctx: &ExecCtx<'_>,
        _pack: &PackManifest,
    ) -> Result<ExecStep, ExecError> {
        Self::run_hook(ctx, "teardown").await
    }

    async fn sync(&self, ctx: &ExecCtx<'_>, _pack: &PackManifest) -> Result<ExecStep, ExecError> {
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
