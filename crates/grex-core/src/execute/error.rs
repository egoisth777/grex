//! Error taxonomy for the execute phase.

use std::path::PathBuf;

use thiserror::Error;

use crate::vars::VarExpandError;

/// Cap on captured-stderr length stored on
/// [`ExecError::ExecNonZero::stderr`]. A 2 KiB window is enough to surface
/// the tail of a typical shell error while keeping a halt-event log line
/// bounded.
pub const EXEC_STDERR_CAPTURE_MAX: usize = 2048;

/// Errors surfaced by [`crate::execute::ActionExecutor::execute`]
/// implementations.
///
/// Marked `#[non_exhaustive]` so slice 5b can add wet-run-specific variants
/// (`FsIo`, `SymlinkCreate`, `SpawnFailed`, `ChildExit`, ...) without
/// breaking downstream `match` arms.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum ExecError {
    /// Variable expansion failed on a specific field of an action.
    #[error("variable expansion failed in field `{field}`: {source}")]
    VarExpand {
        /// Short field identifier (e.g. `"symlink.dst"`).
        field: &'static str,
        /// Underlying expansion error.
        #[source]
        source: VarExpandError,
    },
    /// An expanded string yielded a path shape grex cannot use (empty,
    /// non-UTF-8 surrogate pair, etc.).
    #[error("invalid path after expansion: `{0}`")]
    InvalidPath(String),
    /// A `require` action evaluated to false with `on_fail: error`.
    #[error("require predicate failed: {detail}")]
    RequireFailed {
        /// Human-readable summary of which predicate(s) did not hold.
        detail: String,
    },
    /// An `exec` action had an internally inconsistent post-expansion shape.
    #[error("exec validation failed: {0}")]
    ExecInvalid(String),
    /// The executor's plugin registry has no entry registered under the
    /// action's name. Emitted by the registry-dispatched
    /// [`super::FsExecutor`] / [`super::PlanExecutor`] when a caller
    /// constructs them with a partial registry that does not cover every
    /// variant present in the pack.
    ///
    /// The stock [`crate::plugin::Registry::bootstrap`] path registers all
    /// seven Tier-1 built-ins, so the default [`super::FsExecutor::new`] /
    /// [`super::PlanExecutor::new`] constructors never surface this variant — it
    /// is only reachable through the explicit `with_registry` entry points
    /// that accept a custom registry.
    #[error("no plugin registered for action `{0}`")]
    UnknownAction(String),
    /// A symlink target path is occupied by a non-symlink entry and
    /// `backup: false`; the wet-run executor refuses to clobber blindly.
    #[error("symlink destination `{}` is occupied; enable `backup: true` to rename it out of the way", dst.display())]
    SymlinkDestOccupied {
        /// Post-expansion destination path.
        dst: PathBuf,
    },
    /// Symlink creation returned OS access-denied. On Windows this usually
    /// means Developer Mode is disabled and the caller lacks
    /// `SeCreateSymbolicLinkPrivilege`.
    #[error("symlink creation denied (Windows: enable Developer Mode or run elevated): {detail}")]
    SymlinkPrivilegeDenied {
        /// Verbatim OS error detail for diagnostics.
        detail: String,
    },
    /// A filesystem path exists in a shape incompatible with the requested
    /// action (e.g. mkdir target is already a regular file).
    #[error("path `{}` conflicts with action: {reason}", path.display())]
    PathConflict {
        /// Post-expansion path that conflicted.
        path: PathBuf,
        /// Stable short reason tag.
        reason: &'static str,
    },
    /// `rmdir` without `force: true` attempted to delete a non-empty dir.
    #[error("rmdir on non-empty directory `{}` without force", path.display())]
    RmdirNotEmpty {
        /// Post-expansion path.
        path: PathBuf,
    },
    /// An `env` action requested a persistence scope this platform does not
    /// implement.
    #[error("env scope `{scope}` persistence not supported on {platform}")]
    EnvPersistenceNotSupported {
        /// Scope tag (`user` / `machine`).
        scope: String,
        /// Target platform tag.
        platform: &'static str,
    },
    /// A predicate probed by the predicate evaluator (internal) cannot
    /// be answered on the current platform (e.g. `reg_key` / `psversion`
    /// evaluated on non-Windows). Replaces the pre-M4-C conservative-false
    /// stub: planners and wet-run executors now surface the limitation as
    /// a typed error instead of silently lying about satisfiability.
    #[error("predicate `{predicate}` not supported on {platform}")]
    PredicateNotSupported {
        /// Predicate kind tag (`reg_key` / `psversion`).
        predicate: &'static str,
        /// Target platform tag (from `std::env::consts::OS`).
        platform: &'static str,
    },
    /// A predicate probe ran on the correct platform but the probe itself
    /// failed in a way that prevents a truthful yes/no answer (e.g. the
    /// `powershell.exe` child exited non-zero, timed out, or a registry
    /// read returned a non-`NOT_FOUND` OS error such as ACL denial).
    /// Distinct from [`ExecError::PredicateNotSupported`]: that variant
    /// says "grex cannot answer here at all"; this variant says "grex
    /// tried but the probe itself broke". M4-C post-review introduced it
    /// so syncs fail loud on a broken probe rather than silently
    /// reporting `false`.
    #[error("predicate `{predicate}` probe failed: {detail}")]
    PredicateProbeFailed {
        /// Predicate kind tag (`reg_key` / `psversion`).
        predicate: &'static str,
        /// Human-readable diagnostic (truncated where appropriate).
        detail: String,
    },
    /// OS rejected an env-persistence write (e.g. HKLM without admin).
    #[error("env scope `{scope}` persistence denied: {detail}")]
    EnvPersistenceDenied {
        /// Scope tag (`user` / `machine`).
        scope: String,
        /// Verbatim OS error detail.
        detail: String,
    },
    /// An `exec` action returned a non-zero exit status under
    /// `on_fail: error`.
    ///
    /// `stderr` contains the captured standard-error stream, truncated to
    /// [`EXEC_STDERR_CAPTURE_MAX`] bytes to keep a halt-event log line at
    /// a bounded size. Empty string if the child produced none. PR E
    /// recovery review: previously `cmd.status()` discarded output, so
    /// debugging non-zero exits was blind.
    #[error("exec exited with status {status}: {command}")]
    ExecNonZero {
        /// Process exit status.
        status: i32,
        /// Display-friendly command line.
        command: String,
        /// Captured stderr (truncated to [`EXEC_STDERR_CAPTURE_MAX`] bytes).
        stderr: String,
    },
    /// An `exec` action failed to spawn (program not found, permissions, ...).
    #[error("exec spawn failed for `{command}`: {detail}")]
    ExecSpawnFailed {
        /// Display-friendly command line.
        command: String,
        /// Verbatim OS error detail.
        detail: String,
    },
    /// Filesystem I/O error attributable to a specific op + path.
    #[error("fs {op} failed on `{}`: {detail}", path.display())]
    FsIo {
        /// Stable op tag (`create_dir`, `remove_dir`, `symlink`, `rename`, ...).
        op: &'static str,
        /// Path involved in the op.
        path: PathBuf,
        /// Verbatim OS error detail.
        detail: String,
    },
    /// Symlink was declared with `kind: auto` but `src` does not exist on
    /// disk, so the Windows executor cannot infer whether to call
    /// `symlink_file` or `symlink_dir`. The two Win32 syscalls are
    /// distinct and picking the wrong one yields a reparse point the
    /// shell will not resolve.
    ///
    /// Pack authors hitting this should set `kind: file` or
    /// `kind: directory` explicitly, or ensure `src` exists before the
    /// action runs (for example via an earlier `mkdir`). Only surfaced on
    /// Windows; Unix's single `symlink(2)` does not require the hint.
    #[error(
        "cannot infer symlink kind for `{}`: `src` does not exist. \
         Specify `kind: file` or `kind: directory` explicitly ({detail}).",
        src.display()
    )]
    SymlinkAutoKindUnresolvable {
        /// Post-expansion `src` path that failed to resolve.
        src: PathBuf,
        /// Human-readable context (typically the OS error from stat).
        detail: String,
    },
    /// A meta pack's recursion re-visited a pack path already active on the
    /// dispatch stack.
    ///
    /// M5-2c guards [`crate::plugin::pack_type::MetaPlugin`]'s registry
    /// dispatch against infinite loops by maintaining a canonicalised
    /// visited-set threaded through [`crate::execute::ExecCtx`]. A cycle
    /// implies either an author bug (pack A directly or transitively
    /// includes A) or a registry misconfiguration (a custom pack-type
    /// plugin re-dispatching into its own root). The tree walker performs
    /// its own structural cycle detection at walk time — this variant is
    /// defence-in-depth for the registry-dispatch path, not a replacement.
    #[error("meta recursion cycle at pack path `{}`", path.display())]
    MetaCycle {
        /// Canonicalised pack directory that the cycle re-entered.
        path: PathBuf,
    },
    /// A pack manifest declared a `type:` value that no [`crate::plugin::PackTypeRegistry`]
    /// entry implements. Surfaced by
    /// [`crate::plugin::pack_type::MetaPlugin`] when it recurses into a
    /// child whose type is not registered on the outer context's
    /// [`crate::execute::ExecCtx::pack_type_registry`] — the top-level
    /// `run_pack_lifecycle` guard catches the same shape for the root
    /// pack, but a misconfigured custom registry can still lose an entry
    /// between root and a deep child.
    #[error("no pack-type plugin registered for `{requested}`")]
    UnknownPackType {
        /// The unknown `type:` discriminator as it appeared in the child
        /// manifest.
        requested: String,
    },
    /// Symlink creation failed *after* an existing `dst` was renamed aside
    /// to the backup slot. The original `dst` no longer exists at the
    /// requested path. Restore attempts also failed, so the backup file is
    /// the only remaining artifact and the user must recover manually.
    ///
    /// Surfaced instead of plain [`ExecError::FsIo`] so callers can
    /// distinguish "symlink create raced" (dst still present) from the
    /// dangerous "backup orphan" state pinned by the M3 recovery review.
    ///
    /// NOTE: Logging backup intent into the event log before the rename is
    /// a separate, related gap tracked for PR E (halt-state persistence);
    /// this variant covers the in-executor rollback shape only.
    #[error(
        "symlink create failed after backup, dst `{}` could not be restored from `{}` (create: {create_error}; restore: {})",
        dst.display(),
        backup.display(),
        restore_error.as_deref().unwrap_or("<none>"),
    )]
    SymlinkCreateAfterBackupFailed {
        /// Original destination path the action targeted.
        dst: PathBuf,
        /// Surviving backup path (`<dst>.grex.bak`).
        backup: PathBuf,
        /// Error returned by the symlink create syscall.
        create_error: String,
        /// `Some(detail)` if the rename-back attempt also failed, else
        /// `None`. When `None`, callers should prefer
        /// [`ExecError::FsIo`] — this variant only fires when restore
        /// also fails.
        restore_error: Option<String>,
    },
}

/// Helper: wrap a [`std::io::Error`] into an [`ExecError::FsIo`] with op +
/// path context. Intentionally not a `From` impl — blanket conversions would
/// let unrelated callsites silently map io errors and obscure the op tag.
pub(crate) fn io_to_fs(op: &'static str, path: PathBuf, err: std::io::Error) -> ExecError {
    ExecError::FsIo { op, path, detail: err.to_string() }
}
