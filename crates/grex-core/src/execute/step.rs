//! Observable record of a single action run.
//!
//! A planner produces an [`ExecStep`] describing _what would happen_ with
//! [`ExecResult::WouldPerformChange`] or [`ExecResult::AlreadySatisfied`].
//! A future wet-run executor produces the same shape with
//! [`ExecResult::PerformedChange`]. Downstream audit tooling (lockfile,
//! `grex status`) consumes this shape uniformly.
//!
//! # Why `StepKind` mirrors [`crate::pack::Action`] instead of referencing it
//!
//! `pack::Action` carries **parse-time** strings: `"$HOME/.foo"`. A step's
//! job is to record the _post-expansion_ outcome: `"/home/user/.foo"`.
//! Re-using the parse struct would force consumers to expand again (or
//! thread the `VarEnv` into the audit log) and would conflate "the user
//! wrote X" with "we resolved X to Y". Keeping a separate enum is a clean
//! decoupling that also leaves room for wet-run executors to attach
//! side-effect metadata (e.g. `backup_path`) without polluting the parse
//! model.

use std::path::PathBuf;

use crate::pack::{EnvScope, ExecOnFail, RequireOnFail, SymlinkKind};

/// Coarse-grained outcome of a single step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecResult {
    /// Wet-run executor actually mutated state.
    PerformedChange,
    /// Planner determined the change would happen in a wet run.
    WouldPerformChange,
    /// Target state already matches (e.g. symlink already points at the right
    /// src). Idempotent short-circuit.
    AlreadySatisfied,
    /// Action was a no-op: `when.os` branch not taken, or `require` failed
    /// with `on_fail: skip | warn`. Not an error.
    NoOp,
}

/// Whether a `require` predicate tree evaluated to true.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PredicateOutcome {
    /// Predicate(s) held.
    Satisfied,
    /// Predicate(s) did not hold.
    Unsatisfied,
}

/// Variant-specific detail for a recorded step.
///
/// Paths are [`PathBuf`] rather than `String` because after expansion every
/// path field is a concrete OS path. Command lines remain [`String`]
/// because argv joining for display is lossy by design — the wet-run
/// executor re-reads the underlying [`crate::pack::ExecSpec`] when spawning.
#[derive(Debug, Clone)]
pub enum StepKind {
    /// Resolved symlink descriptor.
    Symlink {
        /// Post-expansion source path.
        src: PathBuf,
        /// Post-expansion destination path.
        dst: PathBuf,
        /// Link-kind selector, passed through from the action.
        kind: SymlinkKind,
        /// Whether an existing `dst` would be backed up.
        backup: bool,
        /// Whether both sides would be canonicalised.
        normalize: bool,
    },
    /// Resolved environment-variable assignment.
    Env {
        /// Variable name (not expanded).
        name: String,
        /// Post-expansion value.
        value: String,
        /// Persistence scope.
        scope: EnvScope,
    },
    /// Resolved mkdir descriptor.
    Mkdir {
        /// Post-expansion path.
        path: PathBuf,
        /// Optional POSIX mode string, verbatim.
        mode: Option<String>,
    },
    /// Resolved rmdir descriptor.
    Rmdir {
        /// Post-expansion path.
        path: PathBuf,
        /// Whether to rename instead of delete.
        backup: bool,
        /// Whether recursive delete is permitted.
        force: bool,
    },
    /// Resolved require gate.
    Require {
        /// Whether the predicate tree held.
        outcome: PredicateOutcome,
        /// Behaviour configured for unsatisfied outcomes.
        on_fail: RequireOnFail,
    },
    /// Resolved when gate.
    When {
        /// Whether the composite condition evaluated to true.
        branch_taken: bool,
        /// Nested planned steps when `branch_taken == true`. Empty otherwise.
        nested_steps: Vec<ExecStep>,
    },
    /// Resolved exec descriptor.
    Exec {
        /// Display-friendly command line (argv joined or cmd_shell verbatim).
        cmdline: String,
        /// Post-expansion working directory, when set.
        cwd: Option<PathBuf>,
        /// Error-propagation policy.
        on_fail: ExecOnFail,
        /// Whether this is a shell form.
        shell: bool,
    },
}

/// Observable record of a single action's execution (or planned execution).
#[derive(Debug, Clone)]
pub struct ExecStep {
    /// Short stable action identifier (one of the action-key strings).
    pub action_name: &'static str,
    /// Coarse outcome.
    pub result: ExecResult,
    /// Variant-specific detail.
    pub details: StepKind,
}
