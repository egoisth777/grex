//! Read-only execution context threaded through every
//! [`crate::execute::ActionExecutor`] call.
//!
//! Kept deliberately small: a variable environment, two filesystem anchors,
//! and a platform tag. No interior mutability, no trait objects, no async
//! machinery. Planner and (eventually) wet-run executors share the same
//! shape so tests can round-trip either path with the same fixture.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::plugin::{PackTypeRegistry, Registry};
use crate::scheduler::Scheduler;
use crate::vars::VarEnv;

/// Shared cycle-detection set threaded through
/// [`crate::plugin::pack_type::MetaPlugin`] recursion.
///
/// Elements are canonicalised pack directories: every time `MetaPlugin`
/// dispatches into a child, it canonicalises the child pack root and
/// inserts it here. A re-entry check before insertion turns registry-level
/// cycles into [`crate::execute::ExecError::MetaCycle`] rather than stack
/// overflow.
///
/// `Arc<Mutex<HashSet<PathBuf>>>` (rather than `RefCell`) because `ExecCtx`
/// is threaded into `async` plugin methods and the M5-2c multi-thread
/// tokio runtime can dispatch siblings concurrently — the mutex window
/// is cheap (two hashset lookups) and uncontended in the common
/// sequential install path.
pub type MetaVisitedSet = Arc<Mutex<HashSet<PathBuf>>>;

/// OS discriminator used by the planner and `when`/`os` predicate paths.
///
/// Kept as a plain C-style enum so `matches!` patterns in the planner stay
/// exhaustive-checked. The [`Platform::Other`] escape hatch carries a
/// `&'static str` rather than `String` — unsupported platforms are rare and
/// don't warrant per-instance allocation.
///
/// Marked `#[non_exhaustive]` so dedicated tags for BSD variants, WASM, or
/// other platforms can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// Linux (any distro).
    Linux,
    /// Apple macOS.
    MacOs,
    /// Microsoft Windows.
    Windows,
    /// Anything else (BSDs, WASM, etc.). Carries the `cfg!(target_os)` tag.
    Other(&'static str),
}

impl Platform {
    /// Detect the current platform from `cfg!(target_os)`.
    #[must_use]
    pub fn current() -> Self {
        #[cfg(target_os = "linux")]
        {
            Self::Linux
        }
        #[cfg(target_os = "macos")]
        {
            Self::MacOs
        }
        #[cfg(target_os = "windows")]
        {
            Self::Windows
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
        {
            Self::Other(std::env::consts::OS)
        }
    }

    /// Return `true` when `token` (a lowercase authored OS tag) matches `self`.
    ///
    /// Accepted tokens: `"windows"`, `"linux"`, `"macos"`, and the umbrella
    /// `"unix"` which covers Linux + macOS. Unknown tokens are conservatively
    /// rejected (false). The comparison is case-sensitive because action
    /// manifests are case-normalised upstream.
    #[must_use]
    pub fn matches_os_token(&self, token: &str) -> bool {
        matches!(
            (self, token),
            (Self::Windows, "windows")
                | (Self::Linux, "linux")
                | (Self::MacOs, "macos")
                | (Self::Linux | Self::MacOs, "unix")
        )
    }
}

/// Read-only context passed to every [`crate::execute::ActionExecutor::execute`] call.
///
/// Lifetimes are carried through rather than cloning so the planner can run
/// over a borrowed `VarEnv` without incurring a copy per action. The ctx is
/// `Copy`-cheap in the sense that all fields are `&`-references — callers
/// typically pass `&ctx` rather than cloning.
///
/// ## Why references, not owned data
///
/// Executors are stateless by contract; any "state" lives in the caller's
/// driver. Owning data inside [`ExecCtx`] would either force clones per
/// action or require interior mutability — both violate the framework goal
/// of "future-proof, maximally decoupled".
///
/// Marked `#[non_exhaustive]` so future slots (plugin registry handle,
/// scheduler token, teardown hook …) can land without breaking library
/// consumers who destructure the struct.
#[non_exhaustive]
#[derive(Debug)]
pub struct ExecCtx<'a> {
    /// Variable lookup table used by every `expand_*` call.
    pub vars: &'a VarEnv,
    /// Pack workdir (the pack's on-disk root). Relative `src` paths in
    /// symlink/exec actions resolve against this directory.
    pub pack_root: &'a Path,
    /// Workspace root (the user's configured grex workspace). Relative
    /// destination paths (though rare — spec encourages absolute) resolve
    /// here.
    pub workspace: &'a Path,
    /// Platform tag. Defaults to [`Platform::current`] but is overridable in
    /// tests to exercise `when.os` branches deterministically.
    pub platform: Platform,
    /// Outer [`Registry`] handle for plugins that recurse into nested
    /// actions (today: `when`). Populated by the concrete executors
    /// (`FsExecutor`, `PlanExecutor`) right before plugin dispatch so
    /// nested `execute` calls go through the caller's registry instead of
    /// a freshly bootstrapped default. `None` outside an executor-driven
    /// call (e.g. direct plugin invocation in tests) — plugins must treat
    /// absence as "no nested dispatch available" and fall back to their
    /// own bootstrap.
    pub registry: Option<&'a Arc<Registry>>,
    /// Outer [`PackTypeRegistry`] handle for pack-type plugins that recurse
    /// across sibling pack types (today: `meta` dispatching into child
    /// packs of arbitrary type). Populated by the pack-level driver before
    /// invoking [`crate::plugin::PackTypePlugin`] methods. `None` outside a
    /// driver-scoped call — see [`ExecCtx::registry`] for the same pattern
    /// at the action level.
    ///
    /// Stage B (M5-1B) only exposes the slot; the dispatch swap that
    /// actually threads a pack-type registry through the executor chain
    /// lands in Stage C.
    pub pack_type_registry: Option<&'a Arc<PackTypeRegistry>>,
    /// Shared cycle-detection set owned by the outer sync driver.
    ///
    /// M5-2c: [`crate::plugin::pack_type::MetaPlugin`] mutates this set
    /// under a lock at every recursion boundary. Absent (`None`) means
    /// no outer driver is tracking recursion — `MetaPlugin` treats that
    /// as "caller promises a single-level dispatch" and skips the check.
    /// The sync driver attaches a fresh empty set at the top of every
    /// install / update / sync run so the first plugin call observes
    /// an empty history. Teardown runs do NOT attach a set:
    /// [`crate::sync::teardown`] drives every pack through the
    /// walker's reverse post-order, so each
    /// [`crate::plugin::PackTypePlugin::teardown`] invocation
    /// corresponds to a single pack and has no in-process recursion
    /// to guard. The cycle-detection set stays defense-in-depth for
    /// direct plugin callers (e.g. the `meta_recursion` integration
    /// tests) that recurse through `MetaPlugin::recurse_children`.
    pub visited_meta: Option<&'a MetaVisitedSet>,
    /// Bounded parallel [`Scheduler`] handle — feat-m6-1.
    ///
    /// Populated by [`crate::sync::run`] at the top of every sync run so
    /// plugins that fan out can bound in-flight children via the same
    /// permit pool used by the outer walker. `None` outside a sync-driven
    /// call (e.g. direct plugin invocation in tests) — plugins that need
    /// to respect the cap must treat absence as "no bound configured"
    /// and fall back to unbounded/serial per their own policy.
    ///
    /// feat-m6-1 lands the slot and CLI flag; plugin acquisition sites
    /// land in feat-m6-2 alongside per-pack `.grex-lock` coordination.
    pub scheduler: Option<&'a Arc<Scheduler>>,
}

impl<'a> ExecCtx<'a> {
    /// Build a context with `platform` defaulted to the current target and
    /// no outer registry attached. Executors attach the registry via
    /// [`ExecCtx::with_registry`] before invoking plugin dispatch.
    #[must_use]
    pub fn new(vars: &'a VarEnv, pack_root: &'a Path, workspace: &'a Path) -> Self {
        Self {
            vars,
            pack_root,
            workspace,
            platform: Platform::current(),
            registry: None,
            pack_type_registry: None,
            visited_meta: None,
            scheduler: None,
        }
    }

    /// Override the platform tag (useful for tests and dry-run overrides).
    #[must_use]
    pub fn with_platform(mut self, p: Platform) -> Self {
        self.platform = p;
        self
    }

    /// Attach the outer [`Registry`] so plugins that recurse (today:
    /// `when`) dispatch nested actions through the caller's registry
    /// instead of a fresh [`Registry::bootstrap`]. Used by
    /// [`crate::execute::FsExecutor`] and [`crate::execute::PlanExecutor`]
    /// just before they hand control to a plugin.
    #[must_use]
    pub fn with_registry(mut self, reg: &'a Arc<Registry>) -> Self {
        self.registry = Some(reg);
        self
    }

    /// Attach the outer [`PackTypeRegistry`] so pack-type plugins that
    /// recurse across child packs (today: `meta`) dispatch through the
    /// caller's registry rather than a fresh
    /// [`PackTypeRegistry::bootstrap`]. The dispatch swap that exercises
    /// this slot ships in M5-1 Stage C; Stage B only lands the slot and
    /// the builder method.
    #[must_use]
    pub fn with_pack_type_registry(mut self, reg: &'a Arc<PackTypeRegistry>) -> Self {
        self.pack_type_registry = Some(reg);
        self
    }

    /// Attach the shared cycle-detection set used by
    /// [`crate::plugin::pack_type::MetaPlugin`] recursion. The sync
    /// driver builds one empty set per `run()` invocation and threads
    /// it through every `ExecCtx` it constructs so nested `install` /
    /// `sync` / `update` / `teardown` calls observe the same history.
    #[must_use]
    pub fn with_visited_meta(mut self, visited: &'a MetaVisitedSet) -> Self {
        self.visited_meta = Some(visited);
        self
    }

    /// Attach a bounded parallel [`Scheduler`] handle. The sync driver
    /// builds one [`Scheduler`] per `run()` invocation (permits ==
    /// `--parallel N`) and threads the same `Arc` through every
    /// `ExecCtx` so sibling plugin dispatch shares the permit pool.
    ///
    /// feat-m6-1 only plumbs the slot; acquisition sites land in
    /// feat-m6-2. Callers may still attach a scheduler today — it is
    /// observably inert until the per-pack lock wiring lands.
    #[must_use]
    pub fn with_scheduler(mut self, scheduler: &'a Arc<Scheduler>) -> Self {
        self.scheduler = Some(scheduler);
        self
    }
}
