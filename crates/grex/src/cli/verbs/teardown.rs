//! `grex teardown` — drive the M5-2b pack-type teardown lifecycle.
//!
//! Thin CLI glue mirroring [`crate::cli::verbs::sync`]. Walks the pack
//! tree, then invokes [`grex_core::sync::teardown`] which dispatches
//! `PackTypePlugin::teardown` on every pack in reverse post-order.
//!
//! Exit codes are shared with `sync` (1 validation, 2 exec, 3 tree).
//! See `crates/grex/src/cli/verbs/sync.rs` for the rationale — the
//! teardown verb reuses the same halted-context renderer so operators
//! see a consistent post-mortem regardless of which lifecycle halted.

use crate::cli::args::{GlobalFlags, TeardownArgs};
use anyhow::Result;
use grex_core::sync::{self, HaltedContext, SyncError, SyncOptions, SyncReport, SyncStep};
use tokio_util::sync::CancellationToken;

/// Entry point for the `teardown` verb.
///
/// # Errors
///
/// Surfaces `anyhow::Result` so `main` can render the orchestrator's
/// output; exit codes are set via `std::process::exit` on halt paths
/// (same pattern as `sync` since `anyhow::Error` does not carry them).
pub fn run(args: TeardownArgs, global: &GlobalFlags, cancel: &CancellationToken) -> Result<()> {
    let Some(pack_root) = args.pack_root.clone() else {
        // Missing required positional → usage error. Mirrors `sync`'s
        // fall-through (see that verb for the rationale).
        if global.json {
            super::sync::emit_json_error(
                "usage",
                "`<pack_root>` is required (directory with `.grex/pack.yaml` or the YAML file)",
                "teardown",
            );
        } else {
            eprintln!(
                "grex teardown: <pack_root> required (directory with `.grex/pack.yaml` or the YAML file)"
            );
        }
        std::process::exit(2);
    };
    let opts = SyncOptions::new()
        .with_dry_run(global.dry_run)
        .with_validate(!args.no_validate)
        .with_workspace(args.workspace.clone());
    match run_impl(&pack_root, &opts, args.quiet, global.json, cancel) {
        RunOutcome::Ok => Ok(()),
        RunOutcome::Validation => std::process::exit(1),
        RunOutcome::Exec => std::process::exit(2),
        RunOutcome::Tree => std::process::exit(3),
    }
}

enum RunOutcome {
    Ok,
    Validation,
    Exec,
    Tree,
}

fn run_impl(
    pack_root: &std::path::Path,
    opts: &SyncOptions,
    quiet: bool,
    json: bool,
    cancel: &CancellationToken,
) -> RunOutcome {
    match sync::teardown(pack_root, opts, cancel) {
        Ok(report) => {
            if json {
                super::sync::emit_json_report(&report, opts.dry_run, "teardown");
            } else {
                render_report(&report, quiet);
            }
            if report.halted.is_some() {
                return RunOutcome::Exec;
            }
            RunOutcome::Ok
        }
        Err(err) => map_sync_outcome(super::sync::classify_sync_err(err, json, "teardown")),
    }
}

/// Narrow `sync`'s [`super::sync::RunOutcome`] (which carries a
/// `UsageError` variant for `--only` glob errors) down to teardown's
/// smaller enum. Teardown doesn't accept `--only`, so `UsageError`
/// would be unreachable; we collapse it into `Tree` defensively.
fn map_sync_outcome(o: super::sync::RunOutcome) -> RunOutcome {
    match o {
        super::sync::RunOutcome::Ok => RunOutcome::Ok,
        super::sync::RunOutcome::Validation => RunOutcome::Validation,
        super::sync::RunOutcome::Exec => RunOutcome::Exec,
        super::sync::RunOutcome::Tree | super::sync::RunOutcome::UsageError => RunOutcome::Tree,
    }
}

fn print_halted_context(ctx: &HaltedContext) {
    eprintln!(
        "teardown halted at pack `{}` action #{} ({}):",
        ctx.pack, ctx.action_idx, ctx.action_name
    );
    eprintln!("  error: {}", ctx.error);
    if let Some(hint) = &ctx.recovery_hint {
        eprintln!("  hint:  {hint}");
    }
}

fn render_report(report: &SyncReport, quiet: bool) {
    if !quiet {
        for s in &report.steps {
            print_step(s);
        }
    }
    for w in &report.event_log_warnings {
        eprintln!("warning: {w}");
    }
    if let Some(err) = &report.halted {
        match err {
            SyncError::Halted(ctx) => print_halted_context(ctx),
            other => eprintln!("halted: {other}"),
        }
    }
}

fn print_step(s: &SyncStep) {
    use grex_core::ExecResult;
    let tag = match &s.exec_step.result {
        ExecResult::PerformedChange => "ok",
        ExecResult::WouldPerformChange => "would",
        ExecResult::AlreadySatisfied => "skipped",
        ExecResult::NoOp => "noop",
        _ => "other",
    };
    println!(
        "[{tag}] pack={pack} action={kind} idx={idx}",
        pack = s.pack,
        kind = s.exec_step.action_name,
        idx = s.action_idx,
    );
}
