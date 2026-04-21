//! Pack-type plugin trait + registry — M5-1 Stage A.
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
//! This file lands the trait and registry **only**. No builtins are
//! registered (`bootstrap` returns empty) and the executor dispatch is
//! untouched — both come in Stage B. Tests exercise the trait and registry
//! shape in isolation with a dummy plugin.
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
    /// plugin.
    ///
    /// # Stage A note
    ///
    /// Stage A lands the registry shape only — no built-ins exist yet, so
    /// this currently returns an empty registry. Stage B will wire up
    /// `meta`, `declarative`, and `scripted` here.
    #[must_use]
    pub fn bootstrap() -> Self {
        Self::new()
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
    fn bootstrap_is_empty_in_stage_a() {
        // Stage A ships the trait + registry only; Stage B will populate
        // meta/declarative/scripted here. Lock the current behaviour so
        // Stage B has a visible diff when it adds built-ins.
        let reg = PackTypeRegistry::bootstrap();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
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
}
