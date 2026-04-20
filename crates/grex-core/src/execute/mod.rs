//! Action execution framework.
//!
//! Stage 5a of M3B wires the surface through which any
//! [`crate::pack::Action`] is consumed at run time. The [`ActionExecutor`]
//! trait is the uniform boundary:
//!
//! * [`PlanExecutor`] — produces [`ExecStep`] records describing what a
//!   wet-run _would_ do without mutating state. Safe to call over any pack.
//! * A future `FsExecutor` (slice 5b) will perform the side effects and
//!   return the same [`ExecStep`] shape with [`ExecResult::PerformedChange`].
//!
//! Keeping the trait narrow (one `execute` method, read-only [`ExecCtx`])
//! means implementations can be tested in isolation and swapped freely.
//!
//! # Scope (5a)
//!
//! The planner applies variable expansion, evaluates predicates, reads the
//! filesystem for idempotency (`path.exists()`, symlink target), but never
//! writes. `RegKey` and `PsVersion` predicates are conservatively stubbed to
//! `false` until slice 5b grows real backends.

pub mod error;
pub mod plan;

mod ctx;
mod predicate;
mod step;

use crate::pack::Action;

pub use ctx::{ExecCtx, Platform};
pub use error::ExecError;
pub use plan::PlanExecutor;
pub use step::{ExecResult, ExecStep, PredicateOutcome, StepKind};

/// Uniform surface for anything that consumes an [`Action`].
///
/// Implementations MUST treat [`ExecCtx`] as read-only and MUST return a
/// [`ExecStep`] on success even for no-op paths (e.g. a predicate that was
/// not satisfied under `on_fail: skip`). A [`ExecStep`] with
/// [`ExecResult::NoOp`] is NOT an error — errors are reserved for
/// authoring bugs (bad var expansion, exec shape invariants, hard predicate
/// failure under `on_fail: error`).
pub trait ActionExecutor: Send + Sync {
    /// Short stable identifier for the implementation. Used in logs / audit
    /// trails. Never rendered to end users as a translation key.
    fn name(&self) -> &'static str;

    /// Execute a single [`Action`] against `ctx`.
    ///
    /// # Errors
    ///
    /// Returns [`ExecError`] on variable-expansion failure, invalid post-
    /// expansion paths, `require` predicates that fail with `on_fail:
    /// error`, or `exec`-shape invariant violations.
    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError>;
}
