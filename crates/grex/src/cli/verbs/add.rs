use crate::cli::args::{AddArgs, GlobalFlags};
use anyhow::{Context, Result};
use grex_core::add::{add_pack, infer_path_from_url, AddOpts, AddReport, AddRequest};
use grex_core::import::classify;
use grex_core::manifest::{ensure_event_log_migrated, find_workspace_root};
use tokio_util::sync::CancellationToken;

pub fn run(args: AddArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let path = args.path.unwrap_or_else(|| infer_path_from_url(&args.url));
    let pack_type = classify(&args.url).as_str().to_string();
    // Resolve the workspace root by walking up from cwd looking for a
    // `.grex/` marker. Falls back to cwd when no marker is found, so a
    // fresh workspace just uses the user's directory. This fixes the
    // v1.x cwd-relative bug where running `grex add` from a subdir
    // created a stray event log in the subdir instead of writing to
    // the parent workspace's log.
    let cwd = std::env::current_dir().context("resolve cwd for workspace root")?;
    let workspace = find_workspace_root(&cwd);
    let manifest = ensure_event_log_migrated(&workspace).context("migrate v1.x event log")?;
    let report = add_pack(
        &manifest,
        AddRequest::new(args.url, path, pack_type),
        AddOpts::new(global.dry_run),
    )
    .context("grex add failed")?;

    if global.json {
        emit_json(&report)?;
    } else {
        emit_human(&report);
    }
    Ok(())
}

fn emit_human(report: &AddReport) {
    let prefix = if report.dry_run { "DRY-RUN: would add" } else { "added" };
    println!(
        "{prefix} {path:<32} {kind:<12} {url}",
        path = report.path,
        kind = report.pack_type,
        url = if report.url.is_empty() { "-" } else { &report.url },
    );
}

fn emit_json(report: &AddReport) -> Result<()> {
    let out = serde_json::json!({
        "dry_run": report.dry_run,
        "id": report.id,
        "url": report.url,
        "path": report.path,
        "type": report.pack_type,
        "appended": report.appended,
    });
    println!("{}", serde_json::to_string(&out)?);
    Ok(())
}
