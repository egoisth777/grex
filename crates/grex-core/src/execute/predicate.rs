//! Predicate evaluator used by [`super::plan`].
//!
//! Scope (5a):
//! * `path_exists`, `cmd_available`, `os`, `symlink_ok` — real checks.
//! * `reg_key`, `psversion` — conservative stubs returning `false` on every
//!   platform. Real backends land in slice 5b so the registry crate and
//!   powershell shell-out stay off the 5a dependency graph.
//! * `all_of` / `any_of` / `none_of` — short-circuit recursion.
//!
//! Expansion failures short-circuit to `false` at the leaf level: a
//! predicate that references an undefined variable can never be satisfied,
//! and pushing the expansion error up through the tree would entangle
//! evaluation with parse diagnostics. Callers wanting fail-loud behaviour
//! should run [`super::plan`] over the owning action directly — it surfaces
//! the underlying [`crate::execute::ExecError::VarExpand`] from the action
//! field, which is strictly more informative.

use std::path::Path;

use crate::pack::{OsKind, Predicate, WhenSpec};
use crate::vars::{expand, VarEnv};

use super::ctx::ExecCtx;

/// Evaluate the composite `when` gate.
///
/// `os` and each combiner compose with AND semantics per `actions.md`.
/// Shared between [`super::plan::PlanExecutor`] and
/// [`super::fs_executor::FsExecutor`] so dry-run and wet-run agree on which
/// branches are taken.
pub(super) fn evaluate_when_gate(spec: &WhenSpec, ctx: &ExecCtx<'_>) -> bool {
    if let Some(os) = spec.os {
        if !evaluate(&Predicate::Os(os), ctx) {
            return false;
        }
    }
    if let Some(list) = &spec.all_of {
        if !list.iter().all(|p| evaluate(p, ctx)) {
            return false;
        }
    }
    if let Some(list) = &spec.any_of {
        if !list.iter().any(|p| evaluate(p, ctx)) {
            return false;
        }
    }
    if let Some(list) = &spec.none_of {
        if list.iter().any(|p| evaluate(p, ctx)) {
            return false;
        }
    }
    true
}

/// Evaluate a predicate tree against `ctx`.
pub(super) fn evaluate(predicate: &Predicate, ctx: &ExecCtx<'_>) -> bool {
    match predicate {
        Predicate::PathExists(raw) => eval_path_exists(raw, ctx.vars),
        Predicate::CmdAvailable(name) => eval_cmd_available(name, ctx.vars),
        Predicate::RegKey { .. } => eval_reg_key_stub(),
        Predicate::Os(os) => eval_os(*os, ctx),
        Predicate::PsVersion(_) => eval_ps_version_stub(),
        Predicate::SymlinkOk { src, dst } => eval_symlink_ok(src, dst, ctx.vars),
        Predicate::AllOf(children) => children.iter().all(|p| evaluate(p, ctx)),
        Predicate::AnyOf(children) => children.iter().any(|p| evaluate(p, ctx)),
        Predicate::NoneOf(children) => !children.iter().any(|p| evaluate(p, ctx)),
    }
}

fn eval_path_exists(raw: &str, env: &VarEnv) -> bool {
    let Ok(expanded) = expand(raw, env) else { return false };
    Path::new(&expanded).exists()
}

fn eval_cmd_available(raw: &str, env: &VarEnv) -> bool {
    let Ok(expanded) = expand(raw, env) else { return false };
    if expanded.is_empty() {
        return false;
    }
    // PATHEXT handles `.exe`/`.bat` on Windows; on Unix we probe the bare
    // name. `which`-style scan: walk PATH, return first hit that resolves
    // to a regular file.
    let path = match env.get("PATH") {
        Some(v) => v.to_string(),
        None => std::env::var("PATH").unwrap_or_default(),
    };
    if path.is_empty() {
        return false;
    }
    #[cfg(windows)]
    let sep = ';';
    #[cfg(not(windows))]
    let sep = ':';

    #[cfg(windows)]
    let extensions: Vec<String> = env
        .get("PATHEXT")
        .map(str::to_string)
        .or_else(|| std::env::var("PATHEXT").ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    #[cfg(not(windows))]
    let extensions: Vec<String> = vec![String::new()];

    for dir in path.split(sep) {
        if dir.is_empty() {
            continue;
        }
        for ext in &extensions {
            let candidate = Path::new(dir).join(format!("{expanded}{ext}"));
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

/// TODO(slice-5b): wire a real registry reader. Until then a `reg_key`
/// predicate is conservatively unsatisfied everywhere; a wet-run that
/// relies on this predicate must run through the 5b `FsExecutor`.
fn eval_reg_key_stub() -> bool {
    false
}

/// TODO(slice-5b): shell out to `powershell -Command '$PSVersionTable...'`
/// and compare against the requested spec. Stubbed to `false` so planners
/// surface the limitation conservatively rather than lying.
fn eval_ps_version_stub() -> bool {
    false
}

fn eval_os(os: OsKind, ctx: &ExecCtx<'_>) -> bool {
    let token = match os {
        OsKind::Windows => "windows",
        OsKind::Linux => "linux",
        OsKind::Macos => "macos",
    };
    ctx.platform.matches_os_token(token)
}

fn eval_symlink_ok(src: &str, dst: &str, env: &VarEnv) -> bool {
    let Ok(src_exp) = expand(src, env) else { return false };
    let Ok(dst_exp) = expand(dst, env) else { return false };
    let dst_path = Path::new(&dst_exp);
    let Ok(meta) = std::fs::symlink_metadata(dst_path) else { return false };
    if !meta.file_type().is_symlink() {
        return false;
    }
    match std::fs::read_link(dst_path) {
        Ok(target) => target == Path::new(&src_exp),
        Err(_) => false,
    }
}
