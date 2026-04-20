//! Wet-run executor — slice 5b.
//!
//! [`FsExecutor`] is the concrete counterpart to
//! [`super::plan::PlanExecutor`]: same trait surface, real side effects. The
//! `execute` method stays a thin dispatcher (one arm per action variant) so
//! cyclomatic complexity lives in the per-action helpers rather than the
//! vtable entry point.
//!
//! # Platform gating
//!
//! * Symlink creation uses `std::os::unix::fs::symlink` on Unix and
//!   `std::os::windows::fs::{symlink_file, symlink_dir}` on Windows.
//! * Persistent env writes use `winreg` on Windows; Unix returns
//!   [`ExecError::EnvPersistenceNotSupported`] for `user` / `machine` scopes
//!   (shell-rc editing is out of scope for this slice).
//! * Mode bits are applied on Unix only; Windows ignores them.
//!
//! # Error propagation
//!
//! Every filesystem op routes through a small internal `io_to_fs` helper so
//! the resulting [`ExecError::FsIo`] carries the op tag and the offending
//! path. Blanket `From<std::io::Error>` is deliberately avoided so unrelated
//! call sites cannot silently leak a context-free io error.

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::pack::{
    Action, EnvArgs, EnvScope, ExecOnFail, ExecSpec, MkdirArgs, RequireOnFail, RequireSpec,
    RmdirArgs, SymlinkArgs, SymlinkKind, WhenSpec,
};
use crate::vars::{expand, VarEnv};

use super::ctx::ExecCtx;
use super::error::{io_to_fs, ExecError};
use super::predicate::{evaluate, evaluate_when_gate};
use super::step::{
    ExecResult, ExecStep, PredicateOutcome, StepKind, ACTION_ENV, ACTION_EXEC, ACTION_MKDIR,
    ACTION_REQUIRE, ACTION_RMDIR, ACTION_SYMLINK, ACTION_WHEN,
};
use super::ActionExecutor;

/// Wet-run [`ActionExecutor`] — performs real filesystem and process work.
///
/// Stateless by contract; cheap to clone and safe to share across threads.
/// Callers are responsible for driving the sequence (plan-phase validators,
/// ordering, rollback on failure); `FsExecutor` operates on one action at a
/// time and never looks at peers.
#[derive(Debug, Default, Clone, Copy)]
pub struct FsExecutor;

impl FsExecutor {
    /// Construct a fresh wet-run executor.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl ActionExecutor for FsExecutor {
    fn name(&self) -> &'static str {
        "fs"
    }

    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
        match action {
            Action::Symlink(s) => fs_symlink(s, ctx),
            Action::Env(e) => fs_env(e, ctx),
            Action::Mkdir(m) => fs_mkdir(m, ctx),
            Action::Rmdir(r) => fs_rmdir(r, ctx),
            Action::Require(r) => fs_require(r, ctx),
            Action::When(w) => fs_when(self, w, ctx),
            Action::Exec(x) => fs_exec(x, ctx),
        }
    }
}

// ---------------------------------------------------------------- shared

fn expand_field(raw: &str, env: &VarEnv, field: &'static str) -> Result<String, ExecError> {
    expand(raw, env).map_err(|source| ExecError::VarExpand { field, source })
}

fn require_path(expanded: String) -> Result<PathBuf, ExecError> {
    if expanded.is_empty() {
        return Err(ExecError::InvalidPath(expanded));
    }
    Ok(PathBuf::from(expanded))
}

// ---------------------------------------------------------------- symlink

fn fs_symlink(args: &SymlinkArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let src = require_path(expand_field(&args.src, ctx.vars, "symlink.src")?)?;
    let dst = require_path(expand_field(&args.dst, ctx.vars, "symlink.dst")?)?;

    let result = match classify_symlink_dst(&src, &dst) {
        SymlinkState::AlreadyCorrect => ExecResult::AlreadySatisfied,
        SymlinkState::Missing => {
            create_symlink(&src, &dst, args.kind)?;
            ExecResult::PerformedChange
        }
        SymlinkState::OccupiedByOther => {
            if !args.backup {
                return Err(ExecError::SymlinkDestOccupied { dst: dst.clone() });
            }
            backup_path(&dst)?;
            create_symlink(&src, &dst, args.kind)?;
            ExecResult::PerformedChange
        }
    };

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

enum SymlinkState {
    AlreadyCorrect,
    Missing,
    OccupiedByOther,
}

fn classify_symlink_dst(src: &Path, dst: &Path) -> SymlinkState {
    match std::fs::symlink_metadata(dst) {
        Err(_) => SymlinkState::Missing,
        Ok(meta) if meta.file_type().is_symlink() => match std::fs::read_link(dst) {
            Ok(target) if target == src => SymlinkState::AlreadyCorrect,
            _ => SymlinkState::OccupiedByOther,
        },
        Ok(_) => SymlinkState::OccupiedByOther,
    }
}

/// Rename `dst` to `<dst>.grex.bak`, overwriting any prior backup.
///
/// This is a deliberately simple convention — one canonical backup slot per
/// path. More elaborate tombstones (timestamped, rotated) belong in the
/// future teardown runner.
fn backup_path(dst: &Path) -> Result<(), ExecError> {
    let mut backup = dst.as_os_str().to_owned();
    backup.push(".grex.bak");
    let backup = PathBuf::from(backup);
    // Best-effort remove of an existing backup before rename — if it fails
    // we let the rename surface a clean error rather than masking it.
    let _ = std::fs::remove_file(&backup);
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(dst, &backup).map_err(|e| io_to_fs("rename", dst.to_path_buf(), e))
}

#[cfg(unix)]
fn create_symlink(src: &Path, dst: &Path, _kind: SymlinkKind) -> Result<(), ExecError> {
    std::os::unix::fs::symlink(src, dst).map_err(|e| io_to_fs("symlink", dst.to_path_buf(), e))
}

#[cfg(windows)]
fn create_symlink(src: &Path, dst: &Path, kind: SymlinkKind) -> Result<(), ExecError> {
    let resolved = resolve_windows_symlink_kind(src, kind);
    let result = match resolved {
        SymlinkKind::Directory => std::os::windows::fs::symlink_dir(src, dst),
        // Auto is normalised to File below when src is absent.
        SymlinkKind::File | SymlinkKind::Auto => std::os::windows::fs::symlink_file(src, dst),
    };
    result.map_err(|e| map_windows_symlink_error(dst, e))
}

#[cfg(windows)]
fn resolve_windows_symlink_kind(src: &Path, kind: SymlinkKind) -> SymlinkKind {
    match kind {
        SymlinkKind::File | SymlinkKind::Directory => kind,
        SymlinkKind::Auto => match std::fs::symlink_metadata(src) {
            Ok(meta) if meta.file_type().is_dir() => SymlinkKind::Directory,
            _ => SymlinkKind::File,
        },
    }
}

#[cfg(windows)]
fn map_windows_symlink_error(dst: &Path, err: std::io::Error) -> ExecError {
    // Windows raw OS error 1314 = ERROR_PRIVILEGE_NOT_HELD.
    if err.raw_os_error() == Some(1314) {
        return ExecError::SymlinkPrivilegeDenied { detail: err.to_string() };
    }
    io_to_fs("symlink", dst.to_path_buf(), err)
}

// ---------------------------------------------------------------- env

fn fs_env(args: &EnvArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let value = expand_field(&args.value, ctx.vars, "env.value")?;
    apply_env(&args.name, &value, args.scope)?;
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_ENV),
        result: ExecResult::PerformedChange,
        details: StepKind::Env { name: args.name.clone(), value, scope: args.scope },
    })
}

fn apply_env(name: &str, value: &str, scope: EnvScope) -> Result<(), ExecError> {
    match scope {
        EnvScope::Session => {
            // SAFETY: `set_var` is unsafe in nightly editions; on stable it's
            // still safe. Process-scoped env is transient — the wet-run docs
            // note this.
            std::env::set_var(name, value);
            Ok(())
        }
        EnvScope::User => apply_env_user(name, value),
        EnvScope::Machine => apply_env_machine(name, value),
    }
}

#[cfg(windows)]
fn apply_env_user(name: &str, value: &str) -> Result<(), ExecError> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_SET_VALUE};
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu.open_subkey_with_flags("Environment", KEY_SET_VALUE).map_err(|e| {
        ExecError::EnvPersistenceDenied { scope: "user".to_string(), detail: e.to_string() }
    })?;
    env.set_value(name, &value.to_string()).map_err(|e| ExecError::EnvPersistenceDenied {
        scope: "user".to_string(),
        detail: e.to_string(),
    })
}

#[cfg(not(windows))]
fn apply_env_user(_name: &str, _value: &str) -> Result<(), ExecError> {
    Err(ExecError::EnvPersistenceNotSupported {
        scope: "user".to_string(),
        platform: std::env::consts::OS,
    })
}

#[cfg(windows)]
fn apply_env_machine(name: &str, value: &str) -> Result<(), ExecError> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_SET_VALUE};
    use winreg::RegKey;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let env = hklm
        .open_subkey_with_flags(
            r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
            KEY_SET_VALUE,
        )
        .map_err(|e| ExecError::EnvPersistenceDenied {
            scope: "machine".to_string(),
            detail: e.to_string(),
        })?;
    env.set_value(name, &value.to_string()).map_err(|e| ExecError::EnvPersistenceDenied {
        scope: "machine".to_string(),
        detail: e.to_string(),
    })
}

#[cfg(not(windows))]
fn apply_env_machine(_name: &str, _value: &str) -> Result<(), ExecError> {
    Err(ExecError::EnvPersistenceNotSupported {
        scope: "machine".to_string(),
        platform: std::env::consts::OS,
    })
}

// ---------------------------------------------------------------- mkdir

fn fs_mkdir(args: &MkdirArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let path = require_path(expand_field(&args.path, ctx.vars, "mkdir.path")?)?;
    let result = apply_mkdir(&path, args.mode.as_deref())?;
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_MKDIR),
        result,
        details: StepKind::Mkdir { path, mode: args.mode.clone() },
    })
}

fn apply_mkdir(path: &Path, mode: Option<&str>) -> Result<ExecResult, ExecError> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_dir() => return Ok(ExecResult::AlreadySatisfied),
        Ok(_) => {
            return Err(ExecError::PathConflict {
                path: path.to_path_buf(),
                reason: "exists as file",
            });
        }
        Err(_) => {}
    }
    std::fs::create_dir_all(path).map_err(|e| io_to_fs("create_dir", path.to_path_buf(), e))?;
    apply_mode(path, mode)?;
    Ok(ExecResult::PerformedChange)
}

#[cfg(unix)]
fn apply_mode(path: &Path, mode: Option<&str>) -> Result<(), ExecError> {
    use std::os::unix::fs::PermissionsExt;
    let Some(mode) = mode else { return Ok(()) };
    let Ok(bits) = u32::from_str_radix(mode, 8) else {
        return Err(ExecError::InvalidPath(format!("invalid POSIX mode `{mode}`")));
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(bits))
        .map_err(|e| io_to_fs("set_permissions", path.to_path_buf(), e))
}

/// Mode bits are POSIX-specific; Windows silently accepts the parsed value
/// as a no-op so pack authors can publish cross-platform manifests.
#[cfg(windows)]
fn apply_mode(_path: &Path, _mode: Option<&str>) -> Result<(), ExecError> {
    Ok(())
}

// ---------------------------------------------------------------- rmdir

fn fs_rmdir(args: &RmdirArgs, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let path = require_path(expand_field(&args.path, ctx.vars, "rmdir.path")?)?;
    let result = apply_rmdir(&path, args.backup, args.force)?;
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_RMDIR),
        result,
        details: StepKind::Rmdir { path, backup: args.backup, force: args.force },
    })
}

fn apply_rmdir(path: &Path, backup: bool, force: bool) -> Result<ExecResult, ExecError> {
    if !path.exists() {
        return Ok(ExecResult::NoOp);
    }
    if backup {
        backup_with_timestamp(path)?;
        return Ok(ExecResult::PerformedChange);
    }
    let res = if force { std::fs::remove_dir_all(path) } else { std::fs::remove_dir(path) };
    match res {
        Ok(()) => Ok(ExecResult::PerformedChange),
        Err(e) if !force && is_not_empty(&e) => {
            Err(ExecError::RmdirNotEmpty { path: path.to_path_buf() })
        }
        Err(e) => Err(io_to_fs("remove_dir", path.to_path_buf(), e)),
    }
}

/// `ErrorKind::DirectoryNotEmpty` is nightly-only; sniff the raw OS error
/// instead (ENOTEMPTY on POSIX, ERROR_DIR_NOT_EMPTY = 145 on Windows).
fn is_not_empty(err: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        matches!(err.raw_os_error(), Some(libc_enotempty) if libc_enotempty == 39 || libc_enotempty == 66)
    }
    #[cfg(windows)]
    {
        err.raw_os_error() == Some(145)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = err;
        false
    }
}

/// Rename `path` to `<path>.grex.bak.<unix_ts_nanos>` so multiple rmdir
/// backups across a session never collide.
fn backup_with_timestamp(path: &Path) -> Result<(), ExecError> {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut backup = path.as_os_str().to_owned();
    backup.push(format!(".grex.bak.{ts}"));
    let backup = PathBuf::from(backup);
    std::fs::rename(path, &backup).map_err(|e| io_to_fs("rename", path.to_path_buf(), e))
}

// ---------------------------------------------------------------- require

fn fs_require(spec: &RequireSpec, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
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

fn evaluate_combiner(combiner: &crate::pack::Combiner, ctx: &ExecCtx<'_>) -> bool {
    use crate::pack::Combiner;
    match combiner {
        Combiner::AllOf(list) => list.iter().all(|p| evaluate(p, ctx)),
        Combiner::AnyOf(list) => list.iter().any(|p| evaluate(p, ctx)),
        Combiner::NoneOf(list) => !list.iter().any(|p| evaluate(p, ctx)),
    }
}

fn classify_require(satisfied: bool, on_fail: RequireOnFail) -> Result<ExecResult, ExecError> {
    if satisfied {
        return Ok(ExecResult::AlreadySatisfied);
    }
    match on_fail {
        RequireOnFail::Error => {
            Err(ExecError::RequireFailed { detail: "combiner evaluated to false".to_string() })
        }
        RequireOnFail::Skip => Ok(ExecResult::NoOp),
        RequireOnFail::Warn => {
            tracing::warn!(target: "grex::execute", "require predicate unsatisfied (on_fail=warn)");
            Ok(ExecResult::NoOp)
        }
    }
}

// ---------------------------------------------------------------- when

fn fs_when(exec: &FsExecutor, spec: &WhenSpec, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let branch_taken = evaluate_when_gate(spec, ctx);
    let (result, nested_steps) = if branch_taken {
        let mut out = Vec::with_capacity(spec.actions.len());
        for a in &spec.actions {
            out.push(exec.execute(a, ctx)?);
        }
        (ExecResult::PerformedChange, out)
    } else {
        (ExecResult::NoOp, Vec::new())
    };
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_WHEN),
        result,
        details: StepKind::When { branch_taken, nested_steps },
    })
}

// ---------------------------------------------------------------- exec

fn fs_exec(spec: &ExecSpec, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError> {
    let cwd = match spec.cwd.as_deref() {
        Some(s) => Some(require_path(expand_field(s, ctx.vars, "exec.cwd")?)?),
        None => None,
    };
    let (cmdline, status) = spawn_exec(spec, cwd.as_deref(), ctx.vars)?;
    let result = classify_exec(status, spec.on_fail, &cmdline)?;
    Ok(ExecStep {
        action_name: Cow::Borrowed(ACTION_EXEC),
        result,
        details: StepKind::Exec { cmdline, cwd, on_fail: spec.on_fail, shell: spec.shell },
    })
}

fn spawn_exec(
    spec: &ExecSpec,
    cwd: Option<&Path>,
    vars: &VarEnv,
) -> Result<(String, i32), ExecError> {
    let (mut cmd, display) = build_command(spec, vars)?;
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    if let Some(env_map) = &spec.env {
        for (k, v) in env_map {
            let expanded = expand_field(v, vars, "exec.env")?;
            cmd.env(k, expanded);
        }
    }
    let status = cmd.status().map_err(|e| ExecError::ExecSpawnFailed {
        command: display.clone(),
        detail: e.to_string(),
    })?;
    Ok((display, status.code().unwrap_or(-1)))
}

fn build_command(spec: &ExecSpec, vars: &VarEnv) -> Result<(Command, String), ExecError> {
    match (spec.shell, &spec.cmd, &spec.cmd_shell) {
        (false, Some(argv), None) => build_argv_command(argv, vars),
        (true, None, Some(line)) => build_shell_command(line, vars),
        _ => Err(ExecError::ExecInvalid(
            "exec requires cmd (shell=false) XOR cmd_shell (shell=true)".to_string(),
        )),
    }
}

fn build_argv_command(argv: &[String], vars: &VarEnv) -> Result<(Command, String), ExecError> {
    if argv.is_empty() {
        return Err(ExecError::ExecInvalid("exec.cmd is empty".to_string()));
    }
    let mut expanded = Vec::with_capacity(argv.len());
    for a in argv {
        expanded.push(expand_field(a, vars, "exec.cmd")?);
    }
    let mut cmd = Command::new(&expanded[0]);
    cmd.args(&expanded[1..]);
    Ok((cmd, expanded.join(" ")))
}

fn build_shell_command(line: &str, vars: &VarEnv) -> Result<(Command, String), ExecError> {
    let expanded = expand_field(line, vars, "exec.cmd_shell")?;
    #[cfg(windows)]
    let (program, flag) = ("cmd", "/C");
    #[cfg(not(windows))]
    let (program, flag) = ("sh", "-c");
    let mut cmd = Command::new(program);
    cmd.arg(flag).arg(&expanded);
    Ok((cmd, expanded))
}

fn classify_exec(status: i32, on_fail: ExecOnFail, cmdline: &str) -> Result<ExecResult, ExecError> {
    if status == 0 {
        return Ok(ExecResult::PerformedChange);
    }
    match on_fail {
        ExecOnFail::Error => Err(ExecError::ExecNonZero { status, command: cmdline.to_string() }),
        ExecOnFail::Warn => {
            tracing::warn!(
                target: "grex::execute",
                status,
                command = %cmdline,
                "exec returned non-zero (on_fail=warn)"
            );
            Ok(ExecResult::PerformedChange)
        }
        ExecOnFail::Ignore => Ok(ExecResult::NoOp),
    }
}
