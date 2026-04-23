//! `grex import` — ingest a legacy `REPOS.json`.

use crate::cli::args::{GlobalFlags, ImportArgs};
use anyhow::{anyhow, Context, Result};
use grex_core::import::{import_from_repos_json, ImportOpts, ImportPlan, SkipReason};
use tokio_util::sync::CancellationToken;

pub fn run(args: ImportArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let from = args
        .from_repos_json
        .as_deref()
        .ok_or_else(|| anyhow!("--from-repos-json <path> is required"))?;

    let manifest = args
        .manifest
        .clone()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default().join("grex.jsonl"));

    let dry_run = args.dry_run || global.dry_run;

    let plan = import_from_repos_json(from, &manifest, ImportOpts { dry_run })
        .context("grex import failed")?;

    if global.json {
        emit_json(&plan)?;
    } else {
        emit_human(&plan, dry_run);
    }
    Ok(())
}

fn emit_human(plan: &ImportPlan, dry_run: bool) {
    for entry in &plan.imported {
        let prefix = if dry_run { "DRY-RUN: would add" } else { "added" };
        println!(
            "{prefix} {path:<32} {kind:<12} {url}",
            path = entry.path,
            kind = entry.kind.as_str(),
            url = if entry.url.is_empty() { "-" } else { &entry.url },
        );
    }
    for skip in &plan.skipped {
        let reason = match skip.reason {
            SkipReason::PathCollision => "path-collision",
            SkipReason::DuplicateInInput => "duplicate-in-input",
        };
        eprintln!("skip {:<32} {}", skip.path, reason);
    }
    println!(
        "\nsummary: imported={} skipped={} failed={}",
        plan.imported.len(),
        plan.skipped.len(),
        plan.failed.len(),
    );
}

fn emit_json(plan: &ImportPlan) -> Result<()> {
    let imported: Vec<_> = plan
        .imported
        .iter()
        .map(|e| {
            serde_json::json!({
                "path": e.path,
                "url": e.url,
                "kind": e.kind.as_str(),
                "would_dispatch": e.would_dispatch,
            })
        })
        .collect();
    let skipped: Vec<_> = plan
        .skipped
        .iter()
        .map(|s| {
            serde_json::json!({
                "path": s.path,
                "reason": match s.reason {
                    SkipReason::PathCollision => "path_collision",
                    SkipReason::DuplicateInInput => "duplicate_in_input",
                },
            })
        })
        .collect();
    let out = serde_json::json!({
        "imported": imported,
        "skipped": skipped,
        "failed": plan.failed.iter().map(|f| serde_json::json!({
            "path": f.path,
            "error": f.error,
        })).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}
