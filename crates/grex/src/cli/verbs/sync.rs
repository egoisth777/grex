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
use tokio_util::sync::CancellationToken;

/// Entry point for the `sync` verb.
///
/// # Errors
///
/// Surface the `anyhow::Result` so `main` can render whatever the
/// orchestrator layer emitted; exit codes are set via `std::process::exit`
/// on the halt paths since `anyhow::Error` does not carry them.
pub fn run(args: SyncArgs, global: &GlobalFlags, cancel: &CancellationToken) -> Result<()> {
    let Some(pack_root) = args.pack_root.clone() else {
        // Missing required positional → usage error. `--json` emits the
        // canonical error envelope (`{verb, error: {kind, message}}`);
        // text mode prints a hint to stderr. Both paths exit 2 (the
        // `cli.md` frozen usage-error code), matching how the MCP
        // surface's `packop_error` reports the same failure.
        if global.json {
            emit_json_error(
                "usage",
                "`<pack_root>` is required (directory with `.grex/pack.yaml` or the YAML file)",
                "sync",
            );
        } else {
            eprintln!(
                "grex sync: <pack_root> required (directory with `.grex/pack.yaml` or the YAML file)"
            );
        }
        std::process::exit(2);
    };
    let dry_run = args.dry_run || global.dry_run;
    let only_patterns = if args.only.is_empty() { None } else { Some(args.only.clone()) };
    let opts = SyncOptions::new()
        .with_dry_run(dry_run)
        .with_validate(!args.no_validate)
        .with_workspace(args.workspace.clone())
        .with_ref_override(args.ref_override.clone())
        .with_only_patterns(only_patterns)
        .with_force(args.force);
    match run_impl(&pack_root, &opts, args.quiet, global.json, cancel) {
        RunOutcome::Ok => Ok(()),
        RunOutcome::UsageError => std::process::exit(2),
        RunOutcome::Validation => std::process::exit(1),
        RunOutcome::Exec => std::process::exit(2),
        RunOutcome::Tree => std::process::exit(3),
    }
}

pub(super) enum RunOutcome {
    Ok,
    /// CLI usage error (invalid `--only` glob, etc.). Maps to exit 2 — the
    /// `cli.md` frozen exit code for usage errors.
    UsageError,
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
    match sync::run(pack_root, opts, cancel) {
        Ok(report) => {
            if json {
                emit_json_report(&report, opts.dry_run, "sync");
            } else {
                render_report(&report, opts.dry_run, quiet);
            }
            if report.halted.is_some() {
                return RunOutcome::Exec;
            }
            RunOutcome::Ok
        }
        Err(err) => classify_sync_err(err, json, "sync"),
    }
}

/// Map a [`SyncError`] to a [`RunOutcome`] and emit the human or JSON
/// error block. Extracted from `run_impl` to keep clippy's
/// `too_many_lines` guard happy — the verb identifier is parameterised
/// so `teardown` can reuse the same routing.
pub(super) fn classify_sync_err(err: SyncError, json: bool, verb: &str) -> RunOutcome {
    match err {
        SyncError::Validation { errors } => {
            emit_validation(&errors, json, verb);
            RunOutcome::Validation
        }
        SyncError::Tree(e) => {
            emit_simple("tree", &e.to_string(), "tree walk failed", json, verb);
            RunOutcome::Tree
        }
        SyncError::Exec(e) => {
            emit_simple("exec", &e.to_string(), "execution error", json, verb);
            RunOutcome::Exec
        }
        SyncError::Halted(ctx) => {
            if json {
                emit_json_halted(&ctx, verb);
            } else {
                print_halted_context(&ctx);
            }
            RunOutcome::Exec
        }
        SyncError::InvalidOnlyGlob { pattern, source } => {
            let msg = format!("invalid --only glob `{pattern}`: {source}");
            emit_simple("usage", &msg, "error", json, verb);
            RunOutcome::UsageError
        }
        other => {
            emit_simple("other", &other.to_string(), &format!("{verb} failed"), json, verb);
            RunOutcome::Tree
        }
    }
}

fn emit_validation(errors: &[impl std::fmt::Display], json: bool, verb: &str) {
    if json {
        let joined = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
        emit_json_error("validation", &joined, verb);
    } else {
        eprintln!("validation failed:");
        for e in errors {
            eprintln!("  - {e}");
        }
    }
}

fn emit_simple(kind: &str, message: &str, human_prefix: &str, json: bool, verb: &str) {
    if json {
        emit_json_error(kind, message, verb);
    } else {
        eprintln!("{human_prefix}: {message}");
    }
}

/// Emit the sync/teardown `SyncReport` as a JSON object mirroring the
/// human text path. One `steps` entry per action; plus the halted
/// context (when present) and a summary count block.
pub(super) fn emit_json_report(report: &SyncReport, dry_run: bool, verb: &str) {
    let steps: Vec<serde_json::Value> =
        report.steps.iter().map(|s| step_to_json(s, dry_run)).collect();
    let halted = report.halted.as_ref().and_then(|h| match h {
        SyncError::Halted(ctx) => Some(serde_json::json!({
            "pack": ctx.pack,
            "action": ctx.action_name,
            "idx": ctx.action_idx,
            "error": ctx.error.to_string(),
            "recovery_hint": ctx.recovery_hint,
        })),
        _ => None,
    });
    let migrations: Vec<serde_json::Value> = report
        .workspace_migrations
        .iter()
        .map(|m| {
            serde_json::json!({
                "from": m.from.display().to_string(),
                "to": m.to.display().to_string(),
                "outcome": migration_outcome_tag(&m.outcome),
                "error": match &m.outcome {
                    grex_core::sync::MigrationOutcome::Failed { error } => {
                        serde_json::Value::String(error.clone())
                    }
                    _ => serde_json::Value::Null,
                },
            })
        })
        .collect();
    let doc = serde_json::json!({
        "verb": verb,
        "dry_run": dry_run,
        "steps": steps,
        "halted": halted,
        "event_log_warnings": report.event_log_warnings,
        "workspace_migrations": migrations,
        "summary": {"total_steps": report.steps.len()},
    });
    if let Ok(s) = serde_json::to_string(&doc) {
        println!("{s}");
    }
}

fn step_to_json(s: &SyncStep, dry_run: bool) -> serde_json::Value {
    use grex_core::ExecResult;
    let (result, details) = match &s.exec_step.result {
        ExecResult::PerformedChange => ("performed_change", serde_json::Value::Null),
        ExecResult::WouldPerformChange => {
            if dry_run {
                ("would_perform_change", serde_json::Value::Null)
            } else {
                ("performed_change", serde_json::Value::Null)
            }
        }
        ExecResult::AlreadySatisfied => ("already_satisfied", serde_json::Value::Null),
        ExecResult::NoOp => ("noop", serde_json::Value::Null),
        ExecResult::Skipped { pack_path, actions_hash, .. } => (
            "skipped",
            serde_json::json!({
                "pack_path": pack_path.display().to_string(),
                "actions_hash": actions_hash,
            }),
        ),
        _ => ("other", serde_json::Value::Null),
    };
    serde_json::json!({
        "pack": s.pack,
        "action": s.exec_step.action_name,
        "idx": s.action_idx,
        "result": result,
        "details": details,
    })
}

pub(super) fn emit_json_error(kind: &str, message: &str, verb: &str) {
    let doc = serde_json::json!({
        "verb": verb,
        "error": {
            "kind": kind,
            "message": message,
        },
    });
    if let Ok(s) = serde_json::to_string(&doc) {
        println!("{s}");
    }
}

pub(super) fn emit_json_halted(ctx: &HaltedContext, verb: &str) {
    let doc = serde_json::json!({
        "verb": verb,
        "halted": {
            "pack": ctx.pack,
            "action": ctx.action_name,
            "idx": ctx.action_idx,
            "error": ctx.error.to_string(),
            "recovery_hint": ctx.recovery_hint,
        },
    });
    if let Ok(s) = serde_json::to_string(&doc) {
        println!("{s}");
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
        if !report.workspace_migrations.is_empty() {
            print_workspace_migrations(&report.workspace_migrations);
        }
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

/// Surface the legacy-layout migration outcomes one line each so users
/// see exactly what happened during the v1.0.x → v1.1.0 upgrade. Empty
/// list does not print (the common case for any workspace built fresh
/// on v1.1.0+).
fn print_workspace_migrations(migrations: &[grex_core::sync::WorkspaceMigration]) {
    use grex_core::sync::MigrationOutcome;
    for m in migrations {
        let from = m.from.display();
        let to = m.to.display();
        match &m.outcome {
            MigrationOutcome::Migrated => {
                eprintln!("[migrated] legacy={from} -> new={to}");
            }
            MigrationOutcome::SkippedBothExist => {
                eprintln!(
                    "[skipped]  legacy={from} AND new={to} both exist; resolve manually",
                );
            }
            MigrationOutcome::SkippedDestOccupied => {
                eprintln!("[skipped]  destination={to} occupied; legacy={from} kept");
            }
            MigrationOutcome::Failed { error } => {
                eprintln!("[failed]   legacy={from} -> new={to}: {error}");
            }
            // MigrationOutcome is #[non_exhaustive]; future variants
            // render with a generic tag until they earn dedicated copy.
            other => eprintln!("[unknown]  legacy={from} -> new={to} ({other:?})"),
        }
    }
}

/// Stable string tag per outcome for `--json` consumers. Lowercase
/// snake-case so it matches the rest of the CLI JSON envelope.
fn migration_outcome_tag(o: &grex_core::sync::MigrationOutcome) -> &'static str {
    use grex_core::sync::MigrationOutcome;
    match o {
        MigrationOutcome::Migrated => "migrated",
        MigrationOutcome::SkippedBothExist => "skipped_both_exist",
        MigrationOutcome::SkippedDestOccupied => "skipped_dest_occupied",
        MigrationOutcome::Failed { .. } => "failed",
        // MigrationOutcome is #[non_exhaustive]; future variants stream
        // through a generic tag until they earn a stable name.
        _ => "other",
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
    if let ExecResult::Skipped { pack_path, actions_hash, .. } = &s.exec_step.result {
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

// M4-D post-review fix bundle: `--only` glob compilation moved
// into `grex-core::sync::compile_only_globset` so the `globset`
// crate version does not leak through the public `SyncOptions`
// surface. CLI unit tests for glob parsing were retired alongside
// the `build_only_globset` helper; semantics are exercised end-to-end
// via `crates/grex/tests/sync_e2e.rs` (`e2e_only_*` cases) and the
// `cli_non_empty_string_rejects_whitespace` parse-layer test in
// `cli::args`. Invalid-glob surfacing is covered by the
// `SyncError::InvalidOnlyGlob` routing in `run_impl`.
