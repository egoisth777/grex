//! Dry-run executor.
//!
//! [`PlanExecutor`] implements [`super::ActionExecutor`] without mutating
//! state. Every action string field is passed through
//! [`crate::vars::expand`] and every filesystem idempotency check goes
//! through read-only syscalls (`symlink_metadata`, `Path::exists`,
//! `std::env::var`). No spawns, no writes, no registry probes.
//!
//! Errors distinguish three layers:
//! 1. Variable expansion failure → [`ExecError::VarExpand`].
//! 2. Expanded path is empty or otherwise unusable → [`ExecError::InvalidPath`].
//! 3. A `require` predicate held false under `on_fail: error` →
//!    [`ExecError::RequireFailed`].
//!
//! Anything else (`when.os` not matching, `require` skip/warn) emits an
//! [`ExecStep`] with [`ExecResult::NoOp`] — "nothing to do here, carry on".

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use crate::pack::{
    Action, Combiner, EnvArgs, ExecOnFail, ExecSpec, MkdirArgs, RequireOnFail, RequireSpec,
    RmdirArgs, SymlinkArgs, WhenSpec,
};
use crate::vars::{expand, VarEnv};

use super::ctx::ExecCtx;
use super::error::ExecError;
use super::predicate::{evaluate, evaluate_when_gate};
use super::step::{
    ExecResult, ExecStep, PredicateOutcome, StepKind, ACTION_ENV, ACTION_EXEC, ACTION_MKDIR,
    ACTION_REQUIRE, ACTION_RMDIR, ACTION_SYMLINK, ACTION_WHEN,
};
use super::ActionExecutor;

/// Dry-run [`ActionExecutor`] — never mutates state.
///
/// Useful for `grex plan`, CI validation, and unit-testing pack semantics.
/// Safe to call across threads; carries no state.
#[derive(Debug, Default, Clone, Copy)]
pub struct PlanExecutor;

impl PlanExecutor {
    /// Construct a fresh planner.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ActionExecutor for PlanExecutor {
    fn name(&self) -> &'static str {
        "plan"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Symlink(s) => plan_symlink(s, ctx),
            Action::Env(e) => plan_env(e, ctx),
            Action::Mkdir(m) => plan_mkdir(m, ctx),
            Action::Rmdir(r) => plan_rmdir(r, ctx),
            Action::Require(r) => plan_require(r, ctx),
            Action::When(w) => plan_when(w, ctx),
            Action::Exec(x) => plan_exec(x, ctx),
        }
    }
}

/// Expand a string field, wrapping expansion errors with field context.
fn expand_field(raw: &str, env: &VarEnv, field: &'static str) -> Result<String, ExecError> {
    expand(raw, env).map_err(|source| ExecError::VarExpand { field, source })
}

/// Convert an expanded string into a [`PathBuf`], rejecting empty paths.
fn require_path(expanded: String) -> Result<PathBuf, ExecError> {
    if expanded.is_empty() {
        return Err(ExecError::InvalidPath(expanded));
    }
    Ok(PathBuf::from(expanded))
}

fn plan_symlink(args: &SymlinkArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let src = require_path(expand_field(&args.src, ctx.vars, "symlink.src")?)?;
    let dst = require_path(expand_field(&args.dst, ctx.vars, "symlink.dst")?)?;
    let result = classify_symlink(&src, &dst);
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_SYMLINK),
        result,
        details: StepKind::Symlink {
            src,
            dst,
            kind: args.kind,
            backup: args.backup,
            normalize: args.normalize,
        },
    })
}

fn classify_symlink(src: &Path, dst: &Path) -> ExecResult {
    match std::fs::symlink_metadata(dst) {
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(dst) {
            Ok(target) if target == src => ExecResult::AlreadySatisfied,
            _ => ExecResult::WouldPerformChange,
        },
        _ => ExecResult::WouldPerformChange,
    }
}

fn plan_env(args: &EnvArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let value = expand_field(&args.value, ctx.vars, "env.value")?;
    let result = classify_env(&args.name, &value, ctx.vars);
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_ENV),
        result,
        details: StepKind::Env { name: args.name.clone(), value, scope: args.scope },
    })
}

fn classify_env(name: &str, value: &str, vars: &VarEnv) -> ExecResult {
    match vars.get(name) {
        Some(existing) if existing == value => ExecResult::AlreadySatisfied,
        _ => ExecResult::WouldPerformChange,
    }
}

fn plan_mkdir(args: &MkdirArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let path = require_path(expand_field(&args.path, ctx.vars, "mkdir.path")?)?;
    let result =
        if path.is_dir() { ExecResult::AlreadySatisfied } else { ExecResult::WouldPerformChange };
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_MKDIR),
        result,
        details: StepKind::Mkdir { path, mode: args.mode.clone() },
    })
}

fn plan_rmdir(args: &RmdirArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let path = require_path(expand_field(&args.path, ctx.vars, "rmdir.path")?)?;
    let result =
        if path.exists() { ExecResult::WouldPerformChange } else { ExecResult::AlreadySatisfied };
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_RMDIR),
        result,
        details: StepKind::Rmdir { path, backup: args.backup, force: args.force },
    })
}

fn plan_require(spec: &RequireSpec, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let satisfied = evaluate_combiner(&spec.combiner, ctx);
    let outcome =
        if satisfied { PredicateOutcome::Satisfied } else { PredicateOutcome::Unsatisfied };
    let result = classify_require(satisfied, spec.on_fail)?;
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_REQUIRE),
        result,
        details: StepKind::Require { outcome, on_fail: spec.on_fail },
    })
}

/// Map a `(satisfied, on_fail)` pair to an [`ExecResult`].
///
/// `satisfied == true` always reports [`ExecResult::AlreadySatisfied`] — a
/// require block performs no work; it asserts. An unsatisfied predicate
/// under `on_fail: error` short-circuits to [`ExecError::RequireFailed`];
/// `skip` and `warn` both yield [`ExecResult::NoOp`]. The warn/skip
/// distinction is preserved in [`StepKind::Require::on_fail`] for audit.
fn classify_require(satisfied: bool, on_fail: RequireOnFail) -> Result<ExecResult, ExecError> {
    if satisfied {
        return Ok(ExecResult::AlreadySatisfied);
    }
    match on_fail {
        RequireOnFail::Error => {
            Err(ExecError::RequireFailed { detail: "combiner evaluated to false".to_string() })
        }
        RequireOnFail::Skip | RequireOnFail::Warn => Ok(ExecResult::NoOp),
    }
}

fn evaluate_combiner(combiner: &Combiner, ctx: &ExecCtx<'_>) -> bool {
    match combiner {
        Combiner::AllOf(list) => list.iter().all(|p| evaluate(p, ctx)),
        Combiner::AnyOf(list) => list.iter().any(|p| evaluate(p, ctx)),
        Combiner::NoneOf(list) => !list.iter().any(|p| evaluate(p, ctx)),
    }
}

fn plan_when(spec: &WhenSpec, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let branch_taken = evaluate_when_gate(spec, ctx);
    let nested_steps = if branch_taken { plan_nested(&spec.actions, ctx)? } else { Vec::new() };
    let result = if branch_taken { ExecResult::WouldPerformChange } else { ExecResult::NoOp };
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_WHEN),
        result,
        details: StepKind::When { branch_taken, nested_steps },
    })
}

fn plan_nested(actions: &[Action], ctx: &ExecCtx<'_>) -> Result<Vec<ExecStep>, ExecError> {
    let planner = PlanExecutor;
    actions.iter().map(|a| planner.execute(a, ctx)).collect()
}

fn plan_exec(spec: &ExecSpec, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let cwd = expand_optional_path(spec.cwd.as_deref(), ctx.vars, "exec.cwd")?;
    let cmdline = build_exec_cmdline(spec, ctx.vars)?;
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_EXEC),
        result: ExecResult::WouldPerformChange,
        details: StepKind::Exec { cmdline, cwd, on_fail: spec.on_fail, shell: spec.shell },
    })
}

fn expand_optional_path(
    raw: Option<&str>,
    env: &VarEnv,
    field: &'static str,
) -> Result<Option<PathBuf>, ExecError> {
    match raw {
        Some(s) => {
            let expanded = expand_field(s, env, field)?;
            Ok(Some(require_path(expanded)?))
        }
        None => Ok(None),
    }
}

/// Build a display command line for an [`ExecSpec`], expanding every arg.
///
/// The returned string is informational only — the wet-run executor will
/// reconstruct argv from the typed [`ExecSpec`] fields rather than parsing
/// this back. Keeping the display separate means authors see the same
/// quoted form regardless of platform shell quirks.
fn build_exec_cmdline(spec: &ExecSpec, env: &VarEnv) -> Result<String, ExecError> {
    match (spec.shell, &spec.cmd, &spec.cmd_shell) {
        (false, Some(argv), None) => join_argv(argv, env),
        (true, None, Some(line)) => expand_field(line, env, "exec.cmd_shell"),
        _ => Err(ExecError::ExecInvalid(
            "exec requires cmd (shell=false) XOR cmd_shell (shell=true)".to_string(),
        )),
    }
}

fn join_argv(argv: &[String], env: &VarEnv) -> Result<String, ExecError> {
    let mut parts = Vec::with_capacity(argv.len());
    for a in argv {
        parts.push(expand_field(a, env, "exec.cmd")?);
    }
    Ok(parts.join(" "))
}

// Silence clippy about the `ExecOnFail` import being behind the ExecSpec re-
// export path; kept explicit for readability of generated docs.
#[allow(dead_code)]
const _: Option<ExecOnFail> = None;
