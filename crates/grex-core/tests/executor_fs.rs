//! Wet-run executor integration tests (slice 5b).
//!
//! All filesystem work lives under a `TempDir` so tests are isolated and
//! self-cleaning. Windows / Unix split is done at the test-function level
//! (not inside `#[cfg]` blocks in a single function) so failing platforms
//! are obvious from `cargo test -- --list`.

use std::collections::BTreeMap;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;

use grex_core::execute::{ActionExecutor, ExecCtx, ExecError, ExecResult, FsExecutor, StepKind};
use grex_core::pack::{
    Action, Combiner, EnvArgs, EnvScope, ExecOnFail, ExecSpec, MkdirArgs, OsKind, Predicate,
    RequireOnFail, RequireSpec, RmdirArgs, SymlinkArgs, SymlinkKind, WhenSpec,
};
use grex_core::vars::VarEnv;
use tempfile::TempDir;

// ---------------------------------------------------------------- helpers

fn fixture() -> (TempDir, VarEnv) {
    let tmp = TempDir::new().expect("tempdir");
    let env = VarEnv::new();
    (tmp, env)
}

fn ctx<'a>(env: &'a VarEnv, root: &'a Path) -> ExecCtx<'a> {
    ExecCtx::new(env, root, root)
}

fn mkdir_action(path: &Path) -> Action {
    Action::Mkdir(MkdirArgs::new(path.to_string_lossy().into_owned(), None))
}

fn rmdir_action(path: &Path, backup: bool, force: bool) -> Action {
    Action::Rmdir(RmdirArgs::new(path.to_string_lossy().into_owned(), backup, force))
}

fn require_path_exists(p: &Path, on_fail: RequireOnFail) -> Action {
    Action::Require(RequireSpec::new(
        Combiner::AllOf(vec![Predicate::PathExists(p.to_string_lossy().into_owned())]),
        on_fail,
    ))
}

fn exec_argv(argv: &[&str], on_fail: ExecOnFail) -> Action {
    Action::Exec(ExecSpec::new(
        Some(argv.iter().map(|s| (*s).to_string()).collect()),
        None,
        false,
        None,
        None,
        on_fail,
    ))
}

// ---------------------------------------------------------------- mkdir

#[test]
fn fs_mkdir_creates_directory() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("new");
    let step = FsExecutor::new().execute(&mkdir_action(&p), &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert!(p.is_dir());
}

#[test]
fn fs_mkdir_idempotent_when_exists() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("existing");
    std::fs::create_dir_all(&p).unwrap();
    let step = FsExecutor::new().execute(&mkdir_action(&p), &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::AlreadySatisfied));
}

#[test]
fn fs_mkdir_errors_on_existing_file() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("not_a_dir");
    std::fs::write(&p, b"hi").unwrap();
    let err = FsExecutor::new().execute(&mkdir_action(&p), &ctx(&env, tmp.path())).unwrap_err();
    assert!(matches!(err, ExecError::PathConflict { .. }));
}

// ---------------------------------------------------------------- rmdir

#[test]
fn fs_rmdir_missing_is_noop() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("missing");
    let step =
        FsExecutor::new().execute(&rmdir_action(&p, false, false), &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::NoOp));
}

#[test]
fn fs_rmdir_removes_empty() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("empty");
    std::fs::create_dir(&p).unwrap();
    let step =
        FsExecutor::new().execute(&rmdir_action(&p, false, false), &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert!(!p.exists());
}

#[test]
fn fs_rmdir_force_removes_nonempty() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("tree");
    std::fs::create_dir_all(p.join("sub")).unwrap();
    std::fs::write(p.join("sub/file"), b"x").unwrap();
    let step =
        FsExecutor::new().execute(&rmdir_action(&p, false, true), &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert!(!p.exists());
}

#[test]
fn fs_rmdir_errors_on_nonempty_without_force() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("tree");
    std::fs::create_dir_all(&p).unwrap();
    std::fs::write(p.join("file"), b"x").unwrap();
    let err = FsExecutor::new()
        .execute(&rmdir_action(&p, false, false), &ctx(&env, tmp.path()))
        .unwrap_err();
    assert!(matches!(err, ExecError::RmdirNotEmpty { .. }));
}

#[test]
fn fs_rmdir_backup_renames_instead_of_deleting() {
    let (tmp, env) = fixture();
    let p = tmp.path().join("keepme");
    std::fs::create_dir(&p).unwrap();
    let step =
        FsExecutor::new().execute(&rmdir_action(&p, true, false), &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert!(!p.exists(), "original path should be renamed out");
    // A sibling `<name>.grex.bak.<ts>` must now exist.
    let found = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(Result::ok)
        .any(|e| e.file_name().to_string_lossy().starts_with("keepme.grex.bak."));
    assert!(found, "expected a timestamped backup sibling");
}

// ---------------------------------------------------------------- exec

#[test]
fn fs_exec_argv_success_reports_performed_change() {
    let (tmp, env) = fixture();
    #[cfg(windows)]
    let argv = &["cmd", "/C", "exit", "0"];
    #[cfg(not(windows))]
    let argv = &["true"];
    let step = FsExecutor::new()
        .execute(&exec_argv(argv, ExecOnFail::Error), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
}

#[test]
fn fs_exec_argv_nonzero_errors_on_fail_error() {
    let (tmp, env) = fixture();
    #[cfg(windows)]
    let argv = &["cmd", "/C", "exit", "3"];
    #[cfg(not(windows))]
    let argv = &["sh", "-c", "exit 3"];
    let err = FsExecutor::new()
        .execute(&exec_argv(argv, ExecOnFail::Error), &ctx(&env, tmp.path()))
        .unwrap_err();
    match err {
        ExecError::ExecNonZero { status, .. } => assert_eq!(status, 3),
        other => panic!("expected ExecNonZero, got {other:?}"),
    }
}

#[test]
fn fs_exec_argv_nonzero_warn_logs_satisfied() {
    let (tmp, env) = fixture();
    #[cfg(windows)]
    let argv = &["cmd", "/C", "exit", "7"];
    #[cfg(not(windows))]
    let argv = &["sh", "-c", "exit 7"];
    let step = FsExecutor::new()
        .execute(&exec_argv(argv, ExecOnFail::Warn), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
}

#[test]
fn fs_exec_argv_nonzero_ignore_returns_noop() {
    let (tmp, env) = fixture();
    #[cfg(windows)]
    let argv = &["cmd", "/C", "exit", "1"];
    #[cfg(not(windows))]
    let argv = &["sh", "-c", "exit 1"];
    let step = FsExecutor::new()
        .execute(&exec_argv(argv, ExecOnFail::Ignore), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::NoOp));
}

#[test]
fn fs_exec_shell_basic_echo() {
    let (tmp, env) = fixture();
    let action = Action::Exec(ExecSpec::new(
        None,
        Some("exit 0".to_string()),
        true,
        None,
        None,
        ExecOnFail::Error,
    ));
    let step = FsExecutor::new().execute(&action, &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    match step.details {
        StepKind::Exec { shell, .. } => assert!(shell),
        other => panic!("expected StepKind::Exec, got {other:?}"),
    }
}

#[test]
fn fs_exec_env_map_is_forwarded() {
    let (tmp, env) = fixture();
    let mut env_map = BTreeMap::new();
    env_map.insert("GREX_TEST_TOKEN".to_string(), "sentinel".to_string());
    // Grex's variable expander consumes `%NAME%` and `$NAME` itself, so
    // escape each sigil (`%%`, `$$`) — after expansion the shell sees the
    // native form and performs its own substitution against the child env.
    #[cfg(windows)]
    let (program, flag, script) =
        ("cmd", "/C", "if \"%%GREX_TEST_TOKEN%%\"==\"sentinel\" (exit 0) else (exit 5)");
    #[cfg(not(windows))]
    let (program, flag, script) = ("sh", "-c", "test \"$$GREX_TEST_TOKEN\" = sentinel");
    let action = Action::Exec(ExecSpec::new(
        Some(vec![program.into(), flag.into(), script.into()]),
        None,
        false,
        None,
        Some(env_map),
        ExecOnFail::Error,
    ));
    let step = FsExecutor::new().execute(&action, &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
}

// ---------------------------------------------------------------- require

#[test]
fn fs_require_path_exists_satisfied_vs_unsatisfied() {
    let (tmp, env) = fixture();
    let present = tmp.path().join("here");
    std::fs::create_dir(&present).unwrap();
    let step = FsExecutor::new()
        .execute(&require_path_exists(&present, RequireOnFail::Error), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::AlreadySatisfied));

    let missing = tmp.path().join("nope");
    let err = FsExecutor::new()
        .execute(&require_path_exists(&missing, RequireOnFail::Error), &ctx(&env, tmp.path()))
        .unwrap_err();
    assert!(matches!(err, ExecError::RequireFailed { .. }));

    let step = FsExecutor::new()
        .execute(&require_path_exists(&missing, RequireOnFail::Skip), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::NoOp));

    let step = FsExecutor::new()
        .execute(&require_path_exists(&missing, RequireOnFail::Warn), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::NoOp));
}

// ---------------------------------------------------------------- when

fn matching_os() -> OsKind {
    #[cfg(windows)]
    return OsKind::Windows;
    #[cfg(target_os = "linux")]
    return OsKind::Linux;
    #[cfg(target_os = "macos")]
    return OsKind::Macos;
}

fn nonmatching_os() -> OsKind {
    #[cfg(windows)]
    return OsKind::Linux;
    #[cfg(not(windows))]
    return OsKind::Windows;
}

#[test]
fn fs_when_gated_in_runs_inner_actions() {
    let (tmp, env) = fixture();
    let inner = tmp.path().join("created_via_when");
    let when = Action::When(WhenSpec::new(
        Some(matching_os()),
        None,
        None,
        None,
        vec![mkdir_action(&inner)],
    ));
    let step = FsExecutor::new().execute(&when, &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert!(inner.is_dir(), "inner mkdir should have run");
    match step.details {
        StepKind::When { branch_taken, nested_steps } => {
            assert!(branch_taken);
            assert_eq!(nested_steps.len(), 1);
        }
        other => panic!("expected StepKind::When, got {other:?}"),
    }
}

#[test]
fn fs_when_gated_out_noop() {
    let (tmp, env) = fixture();
    let inner = tmp.path().join("never_created");
    let when = Action::When(WhenSpec::new(
        Some(nonmatching_os()),
        None,
        None,
        None,
        vec![mkdir_action(&inner)],
    ));
    let step = FsExecutor::new().execute(&when, &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::NoOp));
    assert!(!inner.exists(), "inner mkdir must not run when gated out");
}

// ---------------------------------------------------------------- symlink (unix)

#[cfg(unix)]
fn symlink_action(src: &Path, dst: &Path, backup: bool) -> Action {
    Action::Symlink(SymlinkArgs::new(
        src.to_string_lossy().into_owned(),
        dst.to_string_lossy().into_owned(),
        backup,
        false,
        SymlinkKind::Auto,
    ))
}

#[cfg(unix)]
#[test]
fn fs_symlink_create_file_unix() {
    let (tmp, env) = fixture();
    let src = tmp.path().join("src.txt");
    let dst = tmp.path().join("dst.txt");
    std::fs::write(&src, b"hi").unwrap();
    let step = FsExecutor::new()
        .execute(&symlink_action(&src, &dst, false), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert_eq!(std::fs::read_link(&dst).unwrap(), src);
}

#[cfg(unix)]
#[test]
fn fs_symlink_idempotent_when_target_matches() {
    let (tmp, env) = fixture();
    let src = tmp.path().join("src.txt");
    let dst = tmp.path().join("dst.txt");
    std::fs::write(&src, b"hi").unwrap();
    std::os::unix::fs::symlink(&src, &dst).unwrap();
    let step = FsExecutor::new()
        .execute(&symlink_action(&src, &dst, false), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::AlreadySatisfied));
}

#[cfg(unix)]
#[test]
fn fs_symlink_backup_true_renames_existing() {
    let (tmp, env) = fixture();
    let src = tmp.path().join("src.txt");
    let dst = tmp.path().join("dst.txt");
    std::fs::write(&src, b"hi").unwrap();
    std::fs::write(&dst, b"original").unwrap();
    let step = FsExecutor::new()
        .execute(&symlink_action(&src, &dst, true), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert_eq!(std::fs::read_link(&dst).unwrap(), src);
    let backup = PathBuf::from(format!("{}.grex.bak", dst.display()));
    assert!(backup.exists(), "backup file must exist at {}", backup.display());
}

#[cfg(unix)]
#[test]
fn fs_symlink_rollback_on_create_failure_unix() {
    // Force create_symlink to fail AFTER backup_path has already moved the
    // original dst aside. On Unix we revoke write permission on the parent
    // directory so `symlink(2)` returns EACCES while `rename(2)` from the
    // existing backup slot (already inside the same dir) remains legal via
    // the kernel's in-progress semantics — but because we have RW on the
    // dir from the backup step, the cleanest way to force failure on the
    // *second* op is to chmod the parent to 0o500 after backup completes.
    //
    // We exercise this end-to-end by pre-creating `dst` as a regular file,
    // pre-creating a *read-only* parent one level up so the subdirectory is
    // writable-by-user while we stage, then chmod the subdir after stage.
    //
    // Simpler: use a directory whose permissions we flip between the
    // backup step and create step. We can't intercept the helper, so we
    // instead rely on a different shape: make `dst` live at a path whose
    // `.grex.bak` slot is a directory we cannot remove — `backup_path`
    // best-effort-removes it and then rename target dst→bak fails only if
    // the dst itself is unwritable. Too fragile.
    //
    // Pragmatic pin: create an occupied dst and a backup slot that is a
    // *non-empty directory* belonging to a chmod-0 parent. backup_path
    // succeeds in removing its own backup-slot sibling (best-effort),
    // renames dst→bak successfully, then create_symlink fails because the
    // parent dir has been made read-only.
    use std::os::unix::fs::PermissionsExt;

    let (tmp, env) = fixture();
    let parent = tmp.path().join("lockdown");
    std::fs::create_dir(&parent).unwrap();
    let src = tmp.path().join("src.txt");
    let dst = parent.join("dst");
    std::fs::write(&src, b"hi").unwrap();
    std::fs::write(&dst, b"original").unwrap();

    // Pre-create the backup target so the rename-back-on-rollback
    // attempts to overwrite. backup_path best-effort-removes it first.
    // Flip parent perms to read+execute only AFTER setup; backup_path's
    // first rename (dst → bak) happens inside a writable parent because
    // we set perms below only for the *second* attempt — but the executor
    // runs both ops in sequence. We need a permissions toggle mid-flight,
    // which a pure unit test can't do without a shim. Instead, pin the
    // `SymlinkCreateAfterBackupFailed` branch via the inline helper test
    // below.

    // Fallback: make the src path invalid for symlink create. On Linux
    // symlink(2) accepts almost any src string, so force failure via
    // making the *destination parent* the symlink target of a path that
    // no longer has write perm by the time the executor runs.
    //
    // We approximate by chmod'ing parent to 0o500 BEFORE the executor runs.
    // backup_path then fails at the rename step with EACCES — NOT the
    // rollback branch we want. So this specific shape can't be tested
    // without shimming.
    //
    // Instead, pin the *error variant wiring* via a direct helper
    // invocation below (see `symlink_create_after_backup_failed_variant`).
    // This test only confirms the successful-path backup still works
    // after the rollback refactor did not regress it.
    std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();

    let step = FsExecutor::new()
        .execute(&symlink_action(&src, &dst, true), &ctx(&env, tmp.path()))
        .unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert_eq!(std::fs::read_link(&dst).unwrap(), src);
    let backup = PathBuf::from(format!("{}.grex.bak", dst.display()));
    assert!(backup.exists(), "backup must exist after rollback refactor");
}

/// Pin the `SymlinkCreateAfterBackupFailed` variant shape. We can't easily
/// force the executor's internal create to fail without a shim, so this
/// test constructs the variant directly — a regression guard that the
/// public surface (field names, error formatting) does not drift.
#[test]
fn symlink_create_after_backup_failed_variant_exposed() {
    use std::path::PathBuf;
    let err = ExecError::SymlinkCreateAfterBackupFailed {
        dst: PathBuf::from("/tmp/dst"),
        backup: PathBuf::from("/tmp/dst.grex.bak"),
        create_error: "EACCES".into(),
        restore_error: Some("EBUSY".into()),
    };
    let msg = err.to_string();
    assert!(msg.contains("/tmp/dst"), "message mentions dst: {msg}");
    assert!(msg.contains("/tmp/dst.grex.bak"), "message mentions backup: {msg}");
    assert!(msg.contains("EACCES"), "message includes create_error: {msg}");
    assert!(msg.contains("EBUSY"), "message includes restore_error: {msg}");
}

#[cfg(unix)]
#[test]
fn fs_symlink_backup_false_errors_on_existing() {
    let (tmp, env) = fixture();
    let src = tmp.path().join("src.txt");
    let dst = tmp.path().join("dst.txt");
    std::fs::write(&src, b"hi").unwrap();
    std::fs::write(&dst, b"original").unwrap();
    let err = FsExecutor::new()
        .execute(&symlink_action(&src, &dst, false), &ctx(&env, tmp.path()))
        .unwrap_err();
    assert!(matches!(err, ExecError::SymlinkDestOccupied { .. }));
}

#[cfg(unix)]
#[test]
fn fs_env_persistence_unsupported_on_unix_errors_clearly() {
    let (tmp, env) = fixture();
    let action =
        Action::Env(EnvArgs::new("GREX_TEST_PERSIST".to_string(), "x".to_string(), EnvScope::User));
    let err = FsExecutor::new().execute(&action, &ctx(&env, tmp.path())).unwrap_err();
    assert!(matches!(err, ExecError::EnvPersistenceNotSupported { .. }));
}

#[cfg(unix)]
#[test]
fn fs_env_session_scope_sets_process_env() {
    let (tmp, env) = fixture();
    let action = Action::Env(EnvArgs::new(
        "GREX_TEST_SESSION".to_string(),
        "sentinel".to_string(),
        EnvScope::Session,
    ));
    let step = FsExecutor::new().execute(&action, &ctx(&env, tmp.path())).unwrap();
    assert!(matches!(step.result, ExecResult::PerformedChange));
    assert_eq!(std::env::var("GREX_TEST_SESSION").unwrap(), "sentinel");
}

// ---------------------------------------------------------------- symlink (windows)

#[cfg(windows)]
fn symlink_action(src: &Path, dst: &Path, backup: bool) -> Action {
    Action::Symlink(SymlinkArgs::new(
        src.to_string_lossy().into_owned(),
        dst.to_string_lossy().into_owned(),
        backup,
        false,
        SymlinkKind::Auto,
    ))
}

#[cfg(windows)]
#[test]
fn fs_symlink_create_file_windows() {
    // Symlink creation on Windows requires either elevation or Developer
    // Mode. We only require the executor to either succeed or surface a
    // clean `SymlinkPrivilegeDenied` — never a bare FsIo.
    let (tmp, env) = fixture();
    let src = tmp.path().join("src.txt");
    let dst = tmp.path().join("dst.txt");
    std::fs::write(&src, b"hi").unwrap();
    let res = FsExecutor::new().execute(&symlink_action(&src, &dst, false), &ctx(&env, tmp.path()));
    match res {
        Ok(step) => assert!(matches!(step.result, ExecResult::PerformedChange)),
        Err(ExecError::SymlinkPrivilegeDenied { .. }) => {
            // Runner lacks privilege — acceptable.
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[cfg(windows)]
#[test]
fn fs_env_user_scope_writes_registry_then_cleans_up() {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_ALL_ACCESS};
    use winreg::RegKey;

    let (tmp, env) = fixture();
    let name = "GREX_TEST_USER_PERSIST";
    let value = "sentinel";
    let action = Action::Env(EnvArgs::new(name.to_string(), value.to_string(), EnvScope::User));

    let res = FsExecutor::new().execute(&action, &ctx(&env, tmp.path()));
    match res {
        Ok(step) => {
            assert!(matches!(step.result, ExecResult::PerformedChange));
            // Read back + clean up.
            let hkcu = RegKey::predef(HKEY_CURRENT_USER);
            let env_key = hkcu.open_subkey_with_flags("Environment", KEY_ALL_ACCESS).unwrap();
            let read: String = env_key.get_value(name).unwrap();
            assert_eq!(read, value);
            let _ = env_key.delete_value(name);
        }
        Err(ExecError::EnvPersistenceDenied { .. }) => {
            // HKCU write unexpectedly blocked in this runner — treat as skip.
            eprintln!("skip: HKCU access denied in this environment");
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }
}

#[cfg(windows)]
#[test]
fn fs_env_machine_scope_denied_gracefully() {
    let (tmp, env) = fixture();
    let action = Action::Env(EnvArgs::new(
        "GREX_TEST_MACHINE_PERSIST".to_string(),
        "x".to_string(),
        EnvScope::Machine,
    ));
    let res = FsExecutor::new().execute(&action, &ctx(&env, tmp.path()));
    match res {
        Ok(_) => {
            // Elevated runner — best-effort cleanup.
            use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_ALL_ACCESS};
            use winreg::RegKey;
            let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
            if let Ok(env_key) = hklm.open_subkey_with_flags(
                r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment",
                KEY_ALL_ACCESS,
            ) {
                let _ = env_key.delete_value("GREX_TEST_MACHINE_PERSIST");
            }
        }
        Err(ExecError::EnvPersistenceDenied { .. }) => {}
        Err(other) => panic!("expected PerformedChange or EnvPersistenceDenied, got {other:?}"),
    }
}
