//! Integration tests for M3 Stage B slice 5a — [`PlanExecutor`].
//!
//! Covers:
//! * framework trait bounds (`Send + Sync`)
//! * per-action dry-run semantics (expansion, idempotency, error paths)
//! * predicate evaluator coverage (incl. 5b-stubbed variants)

use std::path::{Path, PathBuf};

use grex_core::pack::{Combiner, RequireOnFail};
use grex_core::{
    Action, ActionExecutor, EnvArgs, EnvScope, ExecCtx, ExecError, ExecOnFail, ExecResult,
    ExecSpec, MkdirArgs, OsKind, PlanExecutor, Platform, Predicate, PredicateOutcome, RequireSpec,
    RmdirArgs, StepKind, SymlinkArgs, SymlinkKind, VarEnv, WhenSpec,
};
use tempfile::TempDir;

// ---------- helpers ----------

fn ctx_with<'a>(vars: &'a VarEnv, pack: &'a Path, ws: &'a Path, platform: Platform) -> ExecCtx<'a> {
    ExecCtx::new(vars, pack, ws).with_platform(platform)
}

fn empty_ctx<'a>(vars: &'a VarEnv, tmp: &'a Path) -> ExecCtx<'a> {
    ExecCtx::new(vars, tmp, tmp)
}

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn mk_symlink_action(src: &str, dst: &str) -> Action {
    Action::Symlink(SymlinkArgs::new(
        src.to_string(),
        dst.to_string(),
        false,
        true,
        SymlinkKind::Auto,
    ))
}

fn mk_env_action(name: &str, value: &str) -> Action {
    Action::Env(EnvArgs::new(name.to_string(), value.to_string(), EnvScope::Session))
}

fn mk_mkdir(path: &str) -> Action {
    Action::Mkdir(MkdirArgs::new(path.to_string(), None))
}

fn mk_rmdir(path: &str) -> Action {
    Action::Rmdir(RmdirArgs::new(path.to_string(), false, false))
}

fn mk_require(combiner: Combiner, on_fail: RequireOnFail) -> Action {
    Action::Require(RequireSpec::new(combiner, on_fail))
}

fn mk_when(os: Option<OsKind>, actions: Vec<Action>) -> Action {
    Action::When(WhenSpec::new(os, None, None, None, actions))
}

// ---------- framework ----------

#[test]
fn exec_ctx_is_send_sync() {
    fn assert_bounds<T: Send + Sync>() {}
    assert_bounds::<PlanExecutor>();
    // ExecCtx itself is not required to be Send/Sync across threads (it's a
    // borrow-only holder) but the planner is.
}

#[test]
fn platform_matches_os_token() {
    assert!(Platform::Linux.matches_os_token("linux"));
    assert!(Platform::Linux.matches_os_token("unix"));
    assert!(!Platform::Linux.matches_os_token("windows"));
    assert!(Platform::MacOs.matches_os_token("macos"));
    assert!(Platform::MacOs.matches_os_token("unix"));
    assert!(Platform::Windows.matches_os_token("windows"));
    assert!(!Platform::Windows.matches_os_token("unix"));
    assert!(!Platform::Other("redox").matches_os_token("linux"));
    assert!(!Platform::Linux.matches_os_token("bogus"));
}

// ---------- symlink ----------

#[test]
fn plan_symlink_expands_vars() {
    let mut vars = VarEnv::new();
    vars.insert("HOME", "/home/user");
    let t = tmp();
    let action = mk_symlink_action("$HOME/src", "$HOME/dst");
    let step = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap();
    match step.details {
        StepKind::Symlink { src, dst, .. } => {
            assert_eq!(src, PathBuf::from("/home/user/src"));
            assert_eq!(dst, PathBuf::from("/home/user/dst"));
        }
        other => panic!("expected Symlink, got {other:?}"),
    }
    assert_eq!(step.result, ExecResult::WouldPerformChange);
}

#[test]
fn plan_symlink_missing_var_errors() {
    let vars = VarEnv::new();
    let t = tmp();
    let action = mk_symlink_action("$UNKNOWN/src", "/tmp/dst");
    let err = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap_err();
    assert!(matches!(err, ExecError::VarExpand { field: "symlink.src", .. }));
}

#[cfg(unix)]
#[test]
fn plan_symlink_already_exists_reports_satisfied() {
    let t = tmp();
    let src = t.path().join("source");
    std::fs::write(&src, b"hello").unwrap();
    let dst = t.path().join("link");
    std::os::unix::fs::symlink(&src, &dst).unwrap();

    let vars = VarEnv::new();
    let action = mk_symlink_action(src.to_str().unwrap(), dst.to_str().unwrap());
    let step = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::AlreadySatisfied);
}

#[cfg(unix)]
#[test]
fn plan_symlink_wrong_target_reports_would_change() {
    let t = tmp();
    let real_src = t.path().join("real");
    let other_src = t.path().join("other");
    std::fs::write(&real_src, b"real").unwrap();
    std::fs::write(&other_src, b"other").unwrap();
    let dst = t.path().join("link");
    std::os::unix::fs::symlink(&other_src, &dst).unwrap();

    let vars = VarEnv::new();
    let action = mk_symlink_action(real_src.to_str().unwrap(), dst.to_str().unwrap());
    let step = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::WouldPerformChange);
}

// ---------- env ----------

#[test]
fn plan_env_reports_would_perform() {
    let vars = VarEnv::new();
    let t = tmp();
    let step =
        PlanExecutor.execute(&mk_env_action("FOO", "bar"), &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::WouldPerformChange);
}

#[test]
fn plan_env_already_matches_reports_satisfied() {
    let mut vars = VarEnv::new();
    vars.insert("FOO", "bar");
    let t = tmp();
    let step =
        PlanExecutor.execute(&mk_env_action("FOO", "bar"), &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::AlreadySatisfied);
}

#[test]
fn plan_env_value_expansion() {
    let mut vars = VarEnv::new();
    vars.insert("BASE", "/opt");
    let t = tmp();
    let step = PlanExecutor
        .execute(&mk_env_action("FOO", "$BASE/bin"), &empty_ctx(&vars, t.path()))
        .unwrap();
    match step.details {
        StepKind::Env { value, .. } => assert_eq!(value, "/opt/bin"),
        other => panic!("expected Env, got {other:?}"),
    }
}

// ---------- mkdir / rmdir ----------

#[test]
fn plan_mkdir_idempotent() {
    let t = tmp();
    let existing = t.path().join("already");
    std::fs::create_dir(&existing).unwrap();
    let vars = VarEnv::new();
    let step = PlanExecutor
        .execute(&mk_mkdir(existing.to_str().unwrap()), &empty_ctx(&vars, t.path()))
        .unwrap();
    assert_eq!(step.result, ExecResult::AlreadySatisfied);
}

#[test]
fn plan_mkdir_needed() {
    let t = tmp();
    let vars = VarEnv::new();
    let missing = t.path().join("new-dir");
    let step = PlanExecutor
        .execute(&mk_mkdir(missing.to_str().unwrap()), &empty_ctx(&vars, t.path()))
        .unwrap();
    assert_eq!(step.result, ExecResult::WouldPerformChange);
}

#[test]
fn plan_rmdir_noop_when_absent() {
    let t = tmp();
    let vars = VarEnv::new();
    let missing = t.path().join("not-there");
    let step = PlanExecutor
        .execute(&mk_rmdir(missing.to_str().unwrap()), &empty_ctx(&vars, t.path()))
        .unwrap();
    assert_eq!(step.result, ExecResult::AlreadySatisfied);
}

#[test]
fn plan_rmdir_present_would_perform() {
    let t = tmp();
    let existing = t.path().join("gone-soon");
    std::fs::create_dir(&existing).unwrap();
    let vars = VarEnv::new();
    let step = PlanExecutor
        .execute(&mk_rmdir(existing.to_str().unwrap()), &empty_ctx(&vars, t.path()))
        .unwrap();
    assert_eq!(step.result, ExecResult::WouldPerformChange);
}

// ---------- require ----------

#[test]
fn plan_require_satisfied() {
    let t = tmp();
    let vars = VarEnv::new();
    let existing = t.path().join("real");
    std::fs::write(&existing, b"x").unwrap();
    let pred = Predicate::PathExists(existing.to_str().unwrap().to_string());
    let action = mk_require(Combiner::AllOf(vec![pred]), RequireOnFail::Error);
    let step = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::AlreadySatisfied);
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("expected Require, got {other:?}"),
    }
}

#[test]
fn plan_require_unsatisfied_errors() {
    let t = tmp();
    let vars = VarEnv::new();
    let pred = Predicate::PathExists("/definitely/not/here-xyz".to_string());
    let action = mk_require(Combiner::AllOf(vec![pred]), RequireOnFail::Error);
    let err = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap_err();
    assert!(matches!(err, ExecError::RequireFailed { .. }));
}

#[test]
fn plan_require_unsatisfied_skip_is_noop() {
    let t = tmp();
    let vars = VarEnv::new();
    let pred = Predicate::PathExists("/definitely/not/here-xyz".to_string());
    let action = mk_require(Combiner::AllOf(vec![pred]), RequireOnFail::Skip);
    let step = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::NoOp);
    match step.details {
        StepKind::Require { outcome, on_fail } => {
            assert_eq!(outcome, PredicateOutcome::Unsatisfied);
            assert_eq!(on_fail, RequireOnFail::Skip);
        }
        other => panic!("expected Require, got {other:?}"),
    }
}

#[test]
fn plan_require_unsatisfied_warn_is_noop() {
    let t = tmp();
    let vars = VarEnv::new();
    let pred = Predicate::PathExists("/definitely/not/here-xyz".to_string());
    let action = mk_require(Combiner::AllOf(vec![pred]), RequireOnFail::Warn);
    let step = PlanExecutor.execute(&action, &empty_ctx(&vars, t.path())).unwrap();
    assert_eq!(step.result, ExecResult::NoOp);
}

// ---------- when ----------

#[test]
fn plan_when_matching_os_recurses() {
    let t = tmp();
    let vars = VarEnv::new();
    let nested = mk_mkdir(t.path().join("linux-only").to_str().unwrap());
    let action = mk_when(Some(OsKind::Linux), vec![nested]);
    let ctx = ctx_with(&vars, t.path(), t.path(), Platform::Linux);
    let step = PlanExecutor.execute(&action, &ctx).unwrap();
    match step.details {
        StepKind::When { branch_taken, nested_steps } => {
            assert!(branch_taken);
            assert_eq!(nested_steps.len(), 1);
            assert_eq!(nested_steps[0].action_name, "mkdir");
        }
        other => panic!("expected When, got {other:?}"),
    }
}

#[test]
fn plan_when_non_matching_os_no_branch() {
    let t = tmp();
    let vars = VarEnv::new();
    let nested = mk_mkdir(t.path().join("wat").to_str().unwrap());
    let action = mk_when(Some(OsKind::Windows), vec![nested]);
    let ctx = ctx_with(&vars, t.path(), t.path(), Platform::Linux);
    let step = PlanExecutor.execute(&action, &ctx).unwrap();
    assert_eq!(step.result, ExecResult::NoOp);
    match step.details {
        StepKind::When { branch_taken, nested_steps } => {
            assert!(!branch_taken);
            assert!(nested_steps.is_empty());
        }
        other => panic!("expected When, got {other:?}"),
    }
}

#[test]
fn plan_when_conjunctive_os_plus_allof() {
    let t = tmp();
    let vars = VarEnv::new();
    let existing = t.path().join("probe");
    std::fs::write(&existing, b"").unwrap();
    let spec = WhenSpec::new(
        Some(OsKind::Linux),
        Some(vec![Predicate::PathExists(existing.to_str().unwrap().to_string())]),
        None,
        None,
        vec![mk_mkdir(t.path().join("out").to_str().unwrap())],
    );
    let action = Action::When(spec);
    let ctx = ctx_with(&vars, t.path(), t.path(), Platform::Linux);
    let step = PlanExecutor.execute(&action, &ctx).unwrap();
    match step.details {
        StepKind::When { branch_taken, .. } => assert!(branch_taken),
        other => panic!("expected When, got {other:?}"),
    }
}

// ---------- exec ----------

#[test]
fn plan_exec_builds_cmdline_argv() {
    let t = tmp();
    let mut vars = VarEnv::new();
    vars.insert("BIN", "echo");
    let spec = ExecSpec::new(
        Some(vec!["$BIN".into(), "hello".into()]),
        None,
        false,
        None,
        None,
        ExecOnFail::Error,
    );
    let step = PlanExecutor.execute(&Action::Exec(spec), &empty_ctx(&vars, t.path())).unwrap();
    match step.details {
        StepKind::Exec { cmdline, shell, .. } => {
            assert_eq!(cmdline, "echo hello");
            assert!(!shell);
        }
        other => panic!("expected Exec, got {other:?}"),
    }
    assert_eq!(step.result, ExecResult::WouldPerformChange);
}

#[test]
fn plan_exec_builds_cmdline_shell() {
    let t = tmp();
    let mut vars = VarEnv::new();
    vars.insert("X", "there");
    let spec =
        ExecSpec::new(None, Some("echo hi $X".to_string()), true, None, None, ExecOnFail::Warn);
    let step = PlanExecutor.execute(&Action::Exec(spec), &empty_ctx(&vars, t.path())).unwrap();
    match step.details {
        StepKind::Exec { cmdline, shell, on_fail, .. } => {
            assert_eq!(cmdline, "echo hi there");
            assert!(shell);
            assert_eq!(on_fail, ExecOnFail::Warn);
        }
        other => panic!("expected Exec, got {other:?}"),
    }
}

// ---------- predicate coverage ----------

#[test]
fn predicate_path_exists_true_false() {
    let t = tmp();
    let vars = VarEnv::new();
    let real = t.path().join("there");
    std::fs::write(&real, b"").unwrap();
    let ctx = empty_ctx(&vars, t.path());
    let true_pred = Predicate::PathExists(real.to_str().unwrap().to_string());
    let false_pred = Predicate::PathExists("/nope/nowhere-xyz".to_string());
    let action_t = mk_require(Combiner::AllOf(vec![true_pred]), RequireOnFail::Skip);
    let action_f = mk_require(Combiner::AllOf(vec![false_pred]), RequireOnFail::Skip);
    let step_t = PlanExecutor.execute(&action_t, &ctx).unwrap();
    let step_f = PlanExecutor.execute(&action_f, &ctx).unwrap();
    match step_t.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("unexpected {other:?}"),
    }
    match step_f.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Unsatisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_cmd_available_checks_path() {
    let t = tmp();
    // Synthesize a fake PATH dir containing a present fake command.
    let bin_dir = t.path().join("bin");
    std::fs::create_dir(&bin_dir).unwrap();
    #[cfg(windows)]
    let cmd_path = bin_dir.join("faketool.exe");
    #[cfg(not(windows))]
    let cmd_path = bin_dir.join("faketool");
    std::fs::write(&cmd_path, b"").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(&cmd_path).unwrap().permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(&cmd_path, perm).unwrap();
    }
    let mut vars = VarEnv::new();
    vars.insert("PATH", bin_dir.to_str().unwrap());
    let ctx = empty_ctx(&vars, t.path());

    let present = Predicate::CmdAvailable("faketool".to_string());
    let absent = Predicate::CmdAvailable("definitely-not-there-xyz".to_string());
    let step_p = PlanExecutor
        .execute(&mk_require(Combiner::AllOf(vec![present]), RequireOnFail::Skip), &ctx)
        .unwrap();
    let step_a = PlanExecutor
        .execute(&mk_require(Combiner::AllOf(vec![absent]), RequireOnFail::Skip), &ctx)
        .unwrap();
    match step_p.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("unexpected {other:?}"),
    }
    match step_a.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Unsatisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_os_matches_current_platform() {
    let t = tmp();
    let vars = VarEnv::new();
    let ctx = ctx_with(&vars, t.path(), t.path(), Platform::Linux);
    let action =
        mk_require(Combiner::AllOf(vec![Predicate::Os(OsKind::Linux)]), RequireOnFail::Skip);
    let step = PlanExecutor.execute(&action, &ctx).unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_combiner_all_of() {
    let t = tmp();
    let vars = VarEnv::new();
    let real = t.path().join("a");
    std::fs::write(&real, b"").unwrap();
    let ctx = empty_ctx(&vars, t.path());
    let mixed = vec![
        Predicate::PathExists(real.to_str().unwrap().to_string()),
        Predicate::PathExists("/nope-xyz".to_string()),
    ];
    let step = PlanExecutor
        .execute(&mk_require(Combiner::AllOf(mixed), RequireOnFail::Skip), &ctx)
        .unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Unsatisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_combiner_any_of() {
    let t = tmp();
    let vars = VarEnv::new();
    let real = t.path().join("a");
    std::fs::write(&real, b"").unwrap();
    let ctx = empty_ctx(&vars, t.path());
    let mixed = vec![
        Predicate::PathExists("/nope-xyz".to_string()),
        Predicate::PathExists(real.to_str().unwrap().to_string()),
    ];
    let step = PlanExecutor
        .execute(&mk_require(Combiner::AnyOf(mixed), RequireOnFail::Skip), &ctx)
        .unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_combiner_none_of_inverts() {
    let t = tmp();
    let vars = VarEnv::new();
    let ctx = empty_ctx(&vars, t.path());
    let all_missing = vec![Predicate::PathExists("/no-xyz".to_string())];
    let step = PlanExecutor
        .execute(&mk_require(Combiner::NoneOf(all_missing), RequireOnFail::Skip), &ctx)
        .unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_nested_combiner_depth_2() {
    let t = tmp();
    let vars = VarEnv::new();
    let real = t.path().join("deep");
    std::fs::write(&real, b"").unwrap();
    let ctx = empty_ctx(&vars, t.path());
    let inner_any =
        Predicate::AnyOf(vec![Predicate::PathExists(real.to_str().unwrap().to_string())]);
    let outer = Combiner::AllOf(vec![inner_any]);
    let step = PlanExecutor.execute(&mk_require(outer, RequireOnFail::Skip), &ctx).unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Satisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_reg_key_defaults_false_stage5a() {
    let t = tmp();
    let vars = VarEnv::new();
    let ctx = empty_ctx(&vars, t.path());
    let pred = Predicate::RegKey {
        path: "HKCU\\Software\\Grex\\Probe".to_string(),
        name: Some("V".to_string()),
    };
    let step = PlanExecutor
        .execute(&mk_require(Combiner::AllOf(vec![pred]), RequireOnFail::Skip), &ctx)
        .unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Unsatisfied),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn predicate_ps_version_defaults_false_stage5a() {
    let t = tmp();
    let vars = VarEnv::new();
    let ctx = empty_ctx(&vars, t.path());
    let pred = Predicate::PsVersion(">=5.1".to_string());
    let step = PlanExecutor
        .execute(&mk_require(Combiner::AllOf(vec![pred]), RequireOnFail::Skip), &ctx)
        .unwrap();
    match step.details {
        StepKind::Require { outcome, .. } => assert_eq!(outcome, PredicateOutcome::Unsatisfied),
        other => panic!("unexpected {other:?}"),
    }
}
