//! Read-only execution context threaded through every
//! [`crate::execute::ActionExecutor`] call.
//!
//! Kept deliberately small: a variable environment, two filesystem anchors,
//! and a platform tag. No interior mutability, no trait objects, no async
//! machinery. Planner and (eventually) wet-run executors share the same
//! shape so tests can round-trip either path with the same fixture.

use std::path::Path;
use std::sync::Arc;

use crate::plugin::Registry;
use crate::vars::VarEnv;

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
}

impl<'a> ExecCtx<'a> {
    /// Build a context with `platform` defaulted to the current target and
    /// no outer registry attached. Executors attach the registry via
    /// [`ExecCtx::with_registry`] before invoking plugin dispatch.
    #[must_use]
    pub fn new(vars: &'a VarEnv, pack_root: &'a Path, workspace: &'a Path) -> Self {
        Self { vars, pack_root, workspace, platform: Platform::current(), registry: None }
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
}
