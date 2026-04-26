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
        emit_json(&plan, dry_run)?;
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

/// Canonical `import` JSON shape. Must remain byte-equal to the MCP
/// handler's output (`crates/grex-mcp/src/tools/import.rs::render_plan_json`)
/// and match `man/reference/cli-json.md §import`. Any field rename or
/// addition MUST land in all three places in the same commit.
///
/// Shape: `{dry_run, imported[], skipped[], failed[]}`. No `summary`
/// wrapper — readers derive counts from the arrays directly.
fn emit_json(plan: &ImportPlan, dry_run: bool) -> Result<()> {
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
    let failed: Vec<_> = plan
        .failed
        .iter()
        .map(|f| {
            serde_json::json!({
                "path": f.path,
                "error": f.error,
            })
        })
        .collect();
    let out = serde_json::json!({
        "dry_run": dry_run,
        "imported": imported,
        "skipped": skipped,
        "failed": failed,
    });
    // Compact form so byte-comparison against the MCP surface (which
    // uses `Value::to_string`, also compact) is trivial.
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
