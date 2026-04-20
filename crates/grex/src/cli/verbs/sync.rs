//! `grex sync` — drive the M3 Stage B end-to-end pipeline.
//!
//! Thin CLI glue: parse args → build [`grex_core::sync::SyncOptions`] → call
//! [`grex_core::sync::run`] → format the resulting [`grex_core::sync::SyncReport`].
//!
//! Exit codes:
//! * `0` — success.
//! * `1` — plan-phase validation errors (manifest + graph).
//! * `2` — action execution error (wet-run or dry-run executor).
//! * `3` — unrecoverable orchestrator failure (tree walk, workspace setup).
//!
//! When `pack_root` is not provided, the legacy M1 stub behaviour is
//! preserved: print `"grex sync: unimplemented"` and exit 0. This keeps the
//! smoke / property tests in `tests/` passing without adding per-test
//! fixtures.

use crate::cli::args::{GlobalFlags, SyncArgs};
use anyhow::Result;
use grex_core::sync::{self, HaltedContext, SyncError, SyncOptions, SyncReport, SyncStep};

/// Entry point for the `sync` verb.
///
/// # Errors
///
/// Surface the `anyhow::Result` so `main` can render whatever the
/// orchestrator layer emitted; exit codes are set via `std::process::exit`
/// on the halt paths since `anyhow::Error` does not carry them.
pub fn run(args: SyncArgs, global: &GlobalFlags) -> Result<()> {
    let Some(pack_root) = args.pack_root.clone() else {
        println!("grex sync: unimplemented (M1 scaffold)");
        return Ok(());
    };
    let dry_run = args.dry_run || global.dry_run;
    let opts =
        SyncOptions { dry_run, validate: !args.no_validate, workspace: args.workspace.clone() };
    match run_impl(&pack_root, &opts, args.quiet) {
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

fn run_impl(pack_root: &std::path::Path, opts: &SyncOptions, quiet: bool) -> RunOutcome {
    match sync::run(pack_root, opts) {
        Ok(report) => {
            render_report(&report, opts.dry_run, quiet);
            if report.halted.is_some() {
                return RunOutcome::Exec;
            }
            RunOutcome::Ok
        }
        Err(SyncError::Validation { errors }) => {
            eprintln!("validation failed:");
            for e in &errors {
                eprintln!("  - {e}");
            }
            RunOutcome::Validation
        }
        Err(SyncError::Tree(e)) => {
            eprintln!("tree walk failed: {e}");
            RunOutcome::Tree
        }
        Err(SyncError::Exec(e)) => {
            eprintln!("execution error: {e}");
            RunOutcome::Exec
        }
        Err(SyncError::Halted(ctx)) => {
            print_halted_context(&ctx);
            RunOutcome::Exec
        }
        // SyncError is `#[non_exhaustive]`; future variants route to the
        // generic unrecoverable bucket until they get dedicated mapping.
        Err(other) => {
            eprintln!("sync failed: {other}");
            RunOutcome::Tree
        }
    }
}

/// Format a [`HaltedContext`] to stderr with pack + action context and,
/// when available, a human recovery hint.
fn print_halted_context(ctx: &HaltedContext) {
    eprintln!(
        "sync halted at pack `{}` action #{} ({}):",
        ctx.pack, ctx.action_idx, ctx.action_name
    );
    eprintln!("  error: {}", ctx.error);
    if let Some(hint) = &ctx.recovery_hint {
        eprintln!("  hint:  {hint}");
    }
}

fn render_report(report: &SyncReport, dry_run: bool, quiet: bool) {
    if !quiet {
        if let Some(rec) = &report.pre_run_recovery {
            print_recovery_report(rec);
        }
        for s in &report.steps {
            print_step(s, dry_run);
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

/// Emit a short, informational block listing any crash-recovery
/// artifacts found before this sync started. Does not block the run.
fn print_recovery_report(rec: &grex_core::sync::RecoveryReport) {
    let total = rec.orphan_backups.len() + rec.orphan_tombstones.len() + rec.dangling_starts.len();
    if total == 0 {
        return;
    }
    eprintln!("warning: pre-run recovery scan found {total} artifact(s) from prior sync:");
    for p in &rec.orphan_backups {
        eprintln!("  orphan backup:    {}", p.display());
    }
    for p in &rec.orphan_tombstones {
        eprintln!("  orphan tombstone: {}", p.display());
    }
    for d in &rec.dangling_starts {
        eprintln!(
            "  dangling start:   pack `{}` action #{} ({}) at {}",
            d.pack, d.action_idx, d.action_name, d.started_at
        );
    }
}

fn print_step(s: &SyncStep, dry_run: bool) {
    use grex_core::ExecResult;
    // `ExecResult::Skipped { pack_path, actions_hash }` gets a dedicated
    // line so the pack path + matched hash surface verbatim instead of
    // being lost into a single tag. M4-B S2 reshaped the variant from
    // `{ reason }` to carry the richer lockfile context. Every other
    // variant renders via the single-token tag path. The wildcard arm
    // at the end is required because `ExecResult` is `#[non_exhaustive]`;
    // future variants route to a generic `other` tag until they earn
    // dedicated rendering.
    if let ExecResult::Skipped { pack_path, actions_hash } = &s.exec_step.result {
        println!(
            "[skipped] pack={pack} path={path} hash={hash}",
            pack = s.pack,
            path = pack_path.display(),
            hash = actions_hash,
        );
        return;
    }
    let tag = match (&s.exec_step.result, dry_run) {
        (ExecResult::PerformedChange, _) => "ok",
        (ExecResult::WouldPerformChange, true) => "would",
        (ExecResult::WouldPerformChange, false) => "ok",
        (ExecResult::AlreadySatisfied, _) => "skipped",
        (ExecResult::NoOp, _) => "noop",
        // ExecResult is #[non_exhaustive]; Skipped is handled above, but
        // future variants land here until they earn a dedicated tag.
        _ => "other",
    };
    println!(
        "[{tag}] pack={pack} action={kind} idx={idx}",
        pack = s.pack,
        kind = s.exec_step.action_name,
        idx = s.action_idx,
    );
}
