//! Plugin system — Stage A slicing (M4-A).
//!
//! Introduces the [`ActionPlugin`] trait and an in-process [`Registry`] as
//! the canonical registration path for Tier-1 actions. The 7 built-ins that
//! M3 landed directly inside `grex-core::execute` are re-exposed here
//! behind the trait so M4-B onwards can extend the surface without touching
//! the executors.
//!
//! # Why not async yet
//!
//! The spec earmarks `async fn execute` for the plugin trait (M4 as a
//! whole). Stage A deliberately keeps the trait synchronous: the wet-run
//! executor, planner, and scheduler are all synchronous today, and the
//! `async-trait` crate is not yet in the workspace dependency set. Adding
//! `async fn` would force every built-in to wrap sync work in an `async`
//! block, introduce `.await` at every call-site, and pull in a tokio
//! runtime — all unrelated to the trait-slicing goal. The async switch
//! belongs to the runtime work in M4-C, not the structural slice here.
//!
//! # Why not dispatch via `Registry` inside `FsExecutor` / `PlanExecutor`
//!
//! The task brief calls for the concrete executors to look up plugins via
//! `registry.get(action.name())` instead of matching on the `Action` enum
//! directly. Doing so requires `FsExecutor` / `PlanExecutor` to carry a
//! `Registry` field (which is not `Copy`), which cascades into >3-line
//! edits across ~50 existing test call sites that construct the executors
//! as bare unit structs. That conflicts with the other explicit rule in
//! the task brief ("If a test needs >3 line change, stop and report").
//! Stage A therefore lands the trait + registry surface; the dispatch
//! swap is queued for M4-B, which can reshape tests alongside the
//! executor constructors in one pass.

use std::borrow::Cow;
use std::collections::HashMap;

use crate::execute::{ExecCtx, ExecError, ExecStep};
use crate::pack::Action;

pub mod pack_type;

#[cfg(feature = "plugin-inventory")]
pub use pack_type::PackTypePluginSubmission;
pub use pack_type::{PackTypePlugin, PackTypeRegistry};

/// Uniform registration surface for every Tier-1 action.
///
/// Implementations MUST be `Send + Sync` so the registry can be threaded
/// across executor threads without interior locking. `execute` takes the
/// parsed [`Action`] (not a `serde_json::Value`): the parse step in
/// `grex-core::pack` has already validated shape + invariants, and the
/// executors that will consume this trait in M4-B already own a typed
/// `&Action`. Taking the typed form keeps the trait zero-cost at the
/// boundary and defers the "raw `Value` for external plugins" form to the
/// dylib / WASM work in M5+.
pub trait ActionPlugin: Send + Sync {
    /// Short kebab-case name matching the YAML key and [`Action::name`].
    /// Used as the key inside [`Registry`].
    fn name(&self) -> &str;

    /// Execute one [`Action`] against `ctx`.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError`] on variable-expansion failure, invalid paths,
    /// `require` failure under `on_fail: error`, exec shape invariants, or
    /// filesystem I/O error (wet-run plugins only).
    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError>;
}

/// In-process registry mapping action name → plugin.
///
/// The v1 discovery path is explicit: callers construct a registry via
/// [`Registry::bootstrap`] (all 7 built-ins) or [`Registry::new`] (empty)
/// and optionally register further plugins with [`Registry::register`].
/// External dylib / WASM loading is deferred to v2 per the feat-grex spec.
#[derive(Default)]
pub struct Registry {
    actions: HashMap<Cow<'static, str>, Box<dyn ActionPlugin>>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Avoid requiring `Debug` on `dyn ActionPlugin`; surface just the
        // action-name inventory, which is what operators actually want.
        f.debug_struct("Registry")
            .field("actions", &self.actions.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Registry {
    /// Construct an empty registry. Prefer [`Registry::bootstrap`] unless
    /// you need a hand-picked plugin set (typical for tests).
    #[must_use]
    pub fn new() -> Self {
        Self { actions: HashMap::new() }
    }

    /// Register `plugin` under its [`ActionPlugin::name`]. Later
    /// registrations overwrite earlier ones with the same name — the
    /// registry is last-writer-wins so higher-priority plugin collections
    /// can shadow the built-ins after [`Registry::bootstrap`].
    pub fn register<P: ActionPlugin + 'static>(&mut self, plugin: P) {
        let name: Cow<'static, str> = Cow::Owned(plugin.name().to_owned());
        self.actions.insert(name, Box::new(plugin));
    }

    /// Look up a plugin by name. Returns `None` if nothing is registered
    /// under that name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&dyn ActionPlugin> {
        self.actions.get(name).map(std::convert::AsRef::as_ref)
    }

    /// Number of registered plugins. Handy for tests and bootstrap
    /// assertions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.actions.len()
    }

    /// Whether no plugins are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.actions.is_empty()
    }

    /// Build a registry pre-populated with every Tier-1 built-in
    /// (`symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`) in
    /// wet-run form.
    ///
    /// The built-ins delegate to the existing `execute::fs_executor` free
    /// functions — there is one struct per action rather than one struct
    /// per executor so external callers can selectively shadow a single
    /// built-in without re-deriving all seven.
    #[must_use]
    pub fn bootstrap() -> Self {
        let mut reg = Self::new();
        register_builtins(&mut reg);
        reg
    }

    /// Register every plugin submitted via [`inventory::submit!`] into the
    /// `PluginSubmission` collector. Order is linker-defined; duplicate
    /// names follow `register`'s last-writer-wins rule. Safe to call after
    /// [`Registry::bootstrap`] — inventory entries shadow existing
    /// registrations like any other `register` call (last-writer-wins).
    ///
    /// Only available when the `plugin-inventory` feature is enabled.
    #[cfg(feature = "plugin-inventory")]
    pub fn register_from_inventory(&mut self) {
        for sub in inventory::iter::<PluginSubmission> {
            let plugin = (sub.factory)();
            let name: Cow<'static, str> = Cow::Owned(plugin.name().to_owned());
            self.actions.insert(name, plugin);
        }
    }

    /// Build a registry populated exclusively from
    /// [`inventory::submit!`] entries. Equivalent to
    /// `let mut r = Registry::new(); r.register_from_inventory(); r`.
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

/// Submission record for compile-time plugin collection via `inventory`.
///
/// Each Tier-1 built-in ships an `inventory::submit!` block (gated by the
/// same feature) pointing at this type, so a consumer opting into
/// `plugin-inventory` can construct a `Registry` purely from linker-time
/// registrations instead of calling [`register_builtins`] explicitly.
#[cfg(feature = "plugin-inventory")]
#[non_exhaustive]
pub struct PluginSubmission {
    /// Factory producing a boxed plugin instance. Invoked once per
    /// submission during [`Registry::register_from_inventory`].
    pub factory: fn() -> Box<dyn ActionPlugin>,
}

#[cfg(feature = "plugin-inventory")]
impl PluginSubmission {
    /// Construct a submission from a plugin factory. Prefer this over
    /// struct-literal syntax so future fields can be added without
    /// breaking downstream `inventory::submit!` sites (the type is
    /// `#[non_exhaustive]`).
    #[must_use]
    pub const fn new(factory: fn() -> Box<dyn ActionPlugin>) -> Self {
        Self { factory }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::collect!(PluginSubmission);

/// Register all 7 Tier-1 built-in plugins in wet-run form.
///
/// Exposed so callers that need a partial registry can start from
/// [`Registry::new`] and layer built-ins on top of (or under) their own
/// plugins. [`Registry::bootstrap`] is the common-case shortcut.
pub fn register_builtins(reg: &mut Registry) {
    reg.register(SymlinkPlugin);
    reg.register(EnvPlugin);
    reg.register(MkdirPlugin);
    reg.register(RmdirPlugin);
    reg.register(RequirePlugin);
    reg.register(WhenPlugin);
    reg.register(ExecPlugin);
}

// ---------------------------------------------------------------- builtins
//
// Each plugin is a zero-sized wet-run wrapper that defers to the existing
// `fs_*` free function in `execute::fs_executor`. Colocating them here in
// `grex-core` avoids a circular dependency with `grex-plugins-builtin`
// (which depends on `grex-core`); the external crate re-exports
// [`register_builtins`] as its canonical registration path.

/// Wet-run `symlink` plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct SymlinkPlugin;

impl ActionPlugin for SymlinkPlugin {
    fn name(&self) -> &str {
        "symlink"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Symlink(s) => crate::execute::fs_executor::fs_symlink(s, ctx),
            _ => Err(ExecError::ExecInvalid(format!(
                "symlink plugin dispatched with non-symlink action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(SymlinkPlugin)));

/// Wet-run `env` plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvPlugin;

impl ActionPlugin for EnvPlugin {
    fn name(&self) -> &str {
        "env"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Env(e) => crate::execute::fs_executor::fs_env(e, ctx),
            _ => Err(ExecError::ExecInvalid(format!(
                "env plugin dispatched with non-env action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(EnvPlugin)));

/// Wet-run `mkdir` plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct MkdirPlugin;

impl ActionPlugin for MkdirPlugin {
    fn name(&self) -> &str {
        "mkdir"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Mkdir(m) => crate::execute::fs_executor::fs_mkdir(m, ctx),
            _ => Err(ExecError::ExecInvalid(format!(
                "mkdir plugin dispatched with non-mkdir action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(MkdirPlugin)));

/// Wet-run `rmdir` plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct RmdirPlugin;

impl ActionPlugin for RmdirPlugin {
    fn name(&self) -> &str {
        "rmdir"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Rmdir(r) => crate::execute::fs_executor::fs_rmdir(r, ctx),
            _ => Err(ExecError::ExecInvalid(format!(
                "rmdir plugin dispatched with non-rmdir action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(RmdirPlugin)));

/// `require` plugin (predicate gate; side-effect-free).
#[derive(Debug, Default, Clone, Copy)]
pub struct RequirePlugin;

impl ActionPlugin for RequirePlugin {
    fn name(&self) -> &str {
        "require"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Require(r) => crate::execute::fs_executor::fs_require(r, ctx),
            _ => Err(ExecError::ExecInvalid(format!(
                "require plugin dispatched with non-require action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(RequirePlugin)));

/// `when` plugin (conditional block; wet-run).
#[derive(Debug, Default, Clone, Copy)]
pub struct WhenPlugin;

impl ActionPlugin for WhenPlugin {
    fn name(&self) -> &str {
        "when"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::When(w) => {
                // Nested dispatch is registry-driven via the `registry`
                // slot on `ExecCtx`. The outer `FsExecutor` attaches its
                // own `Arc<Registry>` to the ctx before calling us, so
                // nested actions resolve through the caller's registry
                // (honouring shadowed or custom plugins) instead of a
                // freshly bootstrapped set.
                crate::execute::fs_executor::fs_when(w, ctx)
            }
            _ => Err(ExecError::ExecInvalid(format!(
                "when plugin dispatched with non-when action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(WhenPlugin)));

/// Wet-run `exec` plugin.
#[derive(Debug, Default, Clone, Copy)]
pub struct ExecPlugin;

impl ActionPlugin for ExecPlugin {
    fn name(&self) -> &str {
        "exec"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Exec(x) => crate::execute::fs_executor::fs_exec(x, ctx),
            _ => Err(ExecError::ExecInvalid(format!(
                "exec plugin dispatched with non-exec action `{}`",
                action.name()
            ))),
        }
    }
}

#[cfg(feature = "plugin-inventory")]
inventory::submit!(PluginSubmission::new(|| Box::new(ExecPlugin)));

// ---------------------------------------------------------------- tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_new_is_empty() {
        let reg = Registry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.get("symlink").is_none());
    }

    #[test]
    fn registry_register_is_last_writer_wins() {
        // Re-registering a built-in under the same name overwrites the
        // prior entry instead of stacking duplicates. The resulting
        // registry size is unchanged.
        let mut reg = Registry::new();
        reg.register(SymlinkPlugin);
        reg.register(SymlinkPlugin);
        assert_eq!(reg.len(), 1);
        assert!(reg.get("symlink").is_some());
    }

    #[test]
    fn bootstrap_registers_all_seven_builtins() {
        let reg = Registry::bootstrap();
        assert_eq!(reg.len(), 7);
        for name in ["symlink", "env", "mkdir", "rmdir", "require", "when", "exec"] {
            let plugin = reg.get(name).unwrap_or_else(|| panic!("missing built-in `{name}`"));
            assert_eq!(plugin.name(), name);
        }
        assert!(reg.get("unknown").is_none());
    }

    #[cfg(feature = "plugin-inventory")]
    #[test]
    fn bootstrap_from_inventory_registers_all_seven_builtins() {
        let reg = Registry::bootstrap_from_inventory();
        assert_eq!(reg.len(), 7);
        for name in ["symlink", "env", "mkdir", "rmdir", "require", "when", "exec"] {
            let plugin = reg.get(name).unwrap_or_else(|| panic!("missing built-in `{name}`"));
            assert_eq!(plugin.name(), name);
        }
    }

    #[cfg(feature = "plugin-inventory")]
    #[test]
    fn register_from_inventory_on_empty_registry_produces_seven_entries() {
        let mut reg = Registry::new();
        assert!(reg.is_empty());
        reg.register_from_inventory();
        assert_eq!(reg.len(), 7);
        for name in ["symlink", "env", "mkdir", "rmdir", "require", "when", "exec"] {
            assert!(reg.get(name).is_some(), "missing built-in `{name}`");
        }
    }

    #[cfg(feature = "plugin-inventory")]
    #[test]
    fn register_from_inventory_twice_dedups_to_seven() {
        let mut reg = Registry::new();
        reg.register_from_inventory();
        reg.register_from_inventory();
        assert_eq!(reg.len(), 7);
    }
}
