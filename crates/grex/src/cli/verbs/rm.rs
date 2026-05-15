//! `grex rm` — tear down a pack and delete its directory.
//!
//! Loads the manifest at `<path>/.grex/pack.yaml` to enforce the
//! meta-with-children guard, then drives the regular teardown
//! lifecycle (`grex_core::sync::teardown`) to allow per-pack-type
//! teardown actions to fire, then removes the directory.

use crate::cli::args::{GlobalFlags, RmArgs};
use anyhow::Result;
use grex_core::sync::{self, SyncOptions};
use grex_core::tree::{FsPackLoader, PackLoader};
use grex_core::PackType;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

pub fn run(args: RmArgs, global: &GlobalFlags, cancel: &CancellationToken) -> Result<()> {
    let pack_root = PathBuf::from(&args.path);
    if !pack_root.exists() {
        emit_error(global.json, "not_found", &format!("{} does not exist", pack_root.display()));
        std::process::exit(2);
    }
    let manifest = match FsPackLoader::new().load(&pack_root) {
        Ok(m) => m,
        Err(err) => {
            emit_error(global.json, "load_manifest", &err.to_string());
            std::process::exit(3);
        }
    };
    if matches!(manifest.r#type, PackType::Meta) && !manifest.children.is_empty() && !args.force {
        let msg = format!(
            "refusing to remove meta-pack with {} children; pass --force to override",
            manifest.children.len()
        );
        emit_error(global.json, "has_children", &msg);
        std::process::exit(1);
    }
    if !args.force {
        run_teardown(&pack_root, global, cancel);
    }
    if global.dry_run {
        emit_ok(global.json, &pack_root, true);
        return Ok(());
    }
    if let Err(err) = std::fs::remove_dir_all(&pack_root) {
        emit_error(global.json, "rmtree", &format!("remove {}: {err}", pack_root.display()));
        std::process::exit(2);
    }
    emit_ok(global.json, &pack_root, false);
    Ok(())
}

/// Drive the per-pack-type teardown lifecycle. `--force` callers skip
/// this — they take responsibility for cleaning up state the teardown
/// plugin would have unwound. Mirrors the teardown verb's error
/// routing so exit codes stay consistent across `rm` and `teardown`.
fn run_teardown(pack_root: &std::path::Path, global: &GlobalFlags, cancel: &CancellationToken) {
    let opts = SyncOptions::new().with_dry_run(global.dry_run).with_validate(true);
    if let Err(err) = sync::teardown(pack_root, &opts, cancel) {
        match super::sync::classify_sync_err(err, global.json, "rm") {
            super::sync::RunOutcome::Validation => std::process::exit(1),
            super::sync::RunOutcome::Exec => std::process::exit(2),
            super::sync::RunOutcome::Tree | super::sync::RunOutcome::UsageError => {
                std::process::exit(3)
            }
            super::sync::RunOutcome::Ok => {}
        }
    }
}

fn emit_ok(json: bool, path: &std::path::Path, dry_run: bool) {
    if json {
        let doc = serde_json::json!({
            "verb": "rm",
            "status": "ok",
            "path": path.display().to_string(),
            "dry_run": dry_run,
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        let prefix = if dry_run { "DRY-RUN: would remove" } else { "removed" };
        println!("{prefix} {}", path.display());
    }
}

fn emit_error(json: bool, kind: &str, msg: &str) {
    if json {
        let doc = serde_json::json!({
            "verb": "rm",
            "error": { "kind": kind, "message": msg },
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        eprintln!("grex rm: {msg}");
    }
}
