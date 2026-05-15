//! `grex run` — execute a single named action across one pack.
//!
//! v1.4.0 ships the single-pack form: load the manifest at the
//! resolved pack root, filter `actions:` to entries whose action
//! kind equals `<action>` (per [`grex_core::Action::name`]), execute
//! them through the standard executor with the registered plugins.
//! Recursive walks over child packs are deferred to v1.5.0.

use crate::cli::args::{GlobalFlags, RunArgs};
use anyhow::Result;
use grex_core::execute::{ActionExecutor, ExecCtx, ExecStep, Platform};
use grex_core::tree::{FsPackLoader, PackLoader};
use grex_core::vars::VarEnv;
use grex_core::{register_builtins, FsExecutor, PlanExecutor, Registry};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

pub fn run(args: RunArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let Some(pack_root) = super::resolve_pack_root_or_cwd(args.pack_root.as_deref()) else {
        emit_error(
            global.json,
            "usage",
            "`<pack_root>` required (directory with `.grex/pack.yaml`)",
        );
        std::process::exit(2);
    };
    let manifest = match FsPackLoader::new().load(&pack_root) {
        Ok(m) => m,
        Err(err) => {
            emit_error(global.json, "load_manifest", &err.to_string());
            std::process::exit(3);
        }
    };
    let target = args.action.as_str();
    let matched: Vec<(usize, &grex_core::Action)> =
        manifest.actions.iter().enumerate().filter(|(_, a)| a.name() == target).collect();
    if matched.is_empty() {
        emit_no_match(global.json, target);
        return Ok(());
    }
    let (steps, had_err) = dispatch_actions(&matched, &pack_root, global.dry_run, global.json);
    emit_report(global.json, target, &pack_root, &steps, global.dry_run);
    if had_err {
        std::process::exit(2);
    }
    Ok(())
}

/// Execute each filtered action in order through `PlanExecutor`
/// (dry-run) or `FsExecutor` (wet-run). Stops on the first failure
/// so partial state is bounded by the first-failure index. Returns
/// the accumulated steps + a flag indicating whether execution
/// halted on an error.
fn dispatch_actions(
    matched: &[(usize, &grex_core::Action)],
    pack_root: &std::path::Path,
    dry_run: bool,
    json: bool,
) -> (Vec<(usize, ExecStep)>, bool) {
    let mut registry = Registry::new();
    register_builtins(&mut registry);
    let registry = Arc::new(registry);
    let vars = VarEnv::new();
    let plan = PlanExecutor::with_registry(registry.clone());
    let fs = FsExecutor::with_registry(registry);
    let mut steps: Vec<(usize, ExecStep)> = Vec::new();
    for (idx, action) in matched {
        let ctx = ExecCtx::new(&vars, pack_root, pack_root).with_platform(Platform::current());
        let result = if dry_run { plan.execute(action, &ctx) } else { fs.execute(action, &ctx) };
        match result {
            Ok(step) => steps.push((*idx, step)),
            Err(err) => {
                emit_error(json, "exec", &format!("action[{idx}] failed: {err}"));
                return (steps, true);
            }
        }
    }
    (steps, false)
}

fn emit_no_match(json: bool, action: &str) {
    if json {
        let doc = serde_json::json!({
            "verb": "run",
            "action": action,
            "matched_packs": 0,
            "steps": [],
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        println!("grex run: no packs declare action `{action}`");
    }
}

fn emit_report(
    json: bool,
    action: &str,
    pack_root: &std::path::Path,
    steps: &[(usize, ExecStep)],
    dry_run: bool,
) {
    if json {
        let step_docs: Vec<serde_json::Value> = steps
            .iter()
            .map(|(idx, _s)| {
                serde_json::json!({
                    "action_idx": idx,
                    "action": action,
                })
            })
            .collect();
        let doc = serde_json::json!({
            "verb": "run",
            "action": action,
            "pack_root": pack_root.display().to_string(),
            "dry_run": dry_run,
            "matched_packs": if steps.is_empty() { 0 } else { 1 },
            "steps": step_docs,
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        let prefix = if dry_run { "DRY-RUN: would run" } else { "ran" };
        for (idx, _s) in steps {
            println!("{prefix} action[{idx}] `{action}` in {}", pack_root.display());
        }
    }
}

fn emit_error(json: bool, kind: &str, msg: &str) {
    if json {
        let doc = serde_json::json!({
            "verb": "run",
            "error": { "kind": kind, "message": msg },
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        eprintln!("grex run: {msg}");
    }
}
