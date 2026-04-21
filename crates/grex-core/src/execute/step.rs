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

use std::borrow::Cow;
use std::path::PathBuf;

use crate::pack::{EnvScope, ExecOnFail, RequireOnFail, SymlinkKind};

/// Coarse-grained outcome of a single step.
///
/// Marked `#[non_exhaustive]` so future milestones (M4 plugin system,
/// lockfile idempotency) can introduce additional outcomes without breaking
/// downstream consumers. External match sites must include a `_` arm.
#[non_exhaustive]
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
    /// Action was deliberately skipped by a caller-level policy — in M4 the
    /// trigger is a lockfile `actions_hash` match on the pack. The variant
    /// carries the pack path and the matched hash so downstream audit
    /// tooling can render "pack X at hash Y was skipped" without having to
    /// thread extra context.
    ///
    /// Marked `#[non_exhaustive]` at the variant level so future audit
    /// fields (e.g. `skipped_at`, `policy_source`) can be added without
    /// breaking downstream struct-pattern match sites.
    #[non_exhaustive]
    Skipped {
        /// Path to the pack whose actions were skipped.
        pack_path: std::path::PathBuf,
        /// Actions-hash that matched the lockfile entry.
        actions_hash: String,
    },
}

/// Whether a `require` predicate tree evaluated to true.
///
/// Marked `#[non_exhaustive]` so predicate evaluation can grow richer
/// outcomes (e.g. `Indeterminate` for deferred probes) without breaking
/// downstream match sites.
#[non_exhaustive]
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
///
/// Marked `#[non_exhaustive]` so the M4 plugin layer can contribute new
/// step-detail shapes without breaking downstream renderers.
#[non_exhaustive]
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
    /// Resolved unlink descriptor — synthesized inverse of
    /// [`StepKind::Symlink`] for auto-reverse teardown (R-M5-09).
    Unlink {
        /// Post-expansion destination path to remove.
        dst: PathBuf,
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
    /// Dedicated pack-level skip detail. Emitted when a pack's
    /// `actions_hash` matches a prior lockfile entry and the sync layer
    /// short-circuits the entire pack. Replaces the M4-B proxy of
    /// `Require { Satisfied, Skip }` with `action_name == "pack"`.
    PackSkipped {
        /// Actions-hash that matched the lockfile entry for this pack.
        actions_hash: String,
    },
}

/// Observable record of a single action's execution (or planned execution).
///
/// Marked `#[non_exhaustive]` so adding audit fields (duration, dry-run tag,
/// plugin-contributed metadata) is a non-breaking change for downstream
/// library consumers.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ExecStep {
    /// Short stable action identifier (one of the action-key strings, or a
    /// plugin-contributed label in future milestones).
    ///
    /// Typed as [`Cow<'static, str>`] so built-in executors can emit
    /// zero-cost static strings via [`Cow::Borrowed`] while M4 plugins can
    /// contribute heap-allocated names via [`Cow::Owned`].
    pub action_name: Cow<'static, str>,
    /// Coarse outcome.
    pub result: ExecResult,
    /// Variant-specific detail.
    pub details: StepKind,
}

/// Short stable action identifiers emitted by built-in executors. Exposed
/// for downstream consumers that need to match step kinds without
/// hard-coding string literals.
pub const ACTION_SYMLINK: &str = "symlink";
/// Built-in `unlink` action identifier (synthesized inverse of symlink).
pub const ACTION_UNLINK: &str = "unlink";
/// Built-in `env` action identifier.
pub const ACTION_ENV: &str = "env";
/// Built-in `mkdir` action identifier.
pub const ACTION_MKDIR: &str = "mkdir";
/// Built-in `rmdir` action identifier.
pub const ACTION_RMDIR: &str = "rmdir";
/// Built-in `require` action identifier.
pub const ACTION_REQUIRE: &str = "require";
/// Built-in `when` action identifier.
pub const ACTION_WHEN: &str = "when";
/// Built-in `exec` action identifier.
pub const ACTION_EXEC: &str = "exec";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_carries_pack_path_and_hash() {
        // Within-crate construction of a `#[non_exhaustive]` variant is
        // allowed without `..`; this guards against accidental promotion
        // of the variant to `#[non_exhaustive(pub)]`-equivalent semantics.
        let r = ExecResult::Skipped {
            pack_path: PathBuf::from("/tmp/packs/foo"),
            actions_hash: "a".repeat(64),
        };
        match r {
            ExecResult::Skipped { pack_path, actions_hash } => {
                assert_eq!(pack_path, PathBuf::from("/tmp/packs/foo"));
                assert_eq!(actions_hash.len(), 64);
            }
            _ => panic!("expected Skipped"),
        }
    }

    #[test]
    fn pack_skipped_round_trips() {
        let k = StepKind::PackSkipped { actions_hash: "abc".into() };
        match k {
            StepKind::PackSkipped { actions_hash } => {
                assert_eq!(actions_hash, "abc");
            }
            _ => panic!("expected PackSkipped"),
        }
    }
}
