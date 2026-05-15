//! `grex status` — drift vs lockfile.
//!
//! Walks the pack tree and runs the sync pipeline with `dry_run = true`.
//! Per-pack drift is derived from the resulting [`SyncReport`]: packs
//! with zero would-execute steps are reported as `clean`; packs with N
//! steps are reported as `would-update N`. The verb never mutates state
//! — the dry-run guarantee (B4) is preserved at the `SyncOptions` layer.

use crate::cli::args::{GlobalFlags, StatusArgs};
use anyhow::Result;
use grex_core::sync::{self, SyncOptions, SyncReport};
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;

pub fn run(args: StatusArgs, global: &GlobalFlags, cancel: &CancellationToken) -> Result<()> {
    let Some(pack_root) = super::resolve_pack_root_or_cwd(args.pack_root.as_deref()) else {
        emit_error(
            global.json,
            "usage",
            "`<pack_root>` required (directory with `.grex/pack.yaml`)",
        );
        std::process::exit(2);
    };
    let opts = SyncOptions::new().with_dry_run(true).with_validate(true);
    match sync::run(&pack_root, &opts, cancel) {
        Ok(report) => {
            render(&report, global.json);
            if report.halted.is_some() {
                std::process::exit(2);
            }
            Ok(())
        }
        Err(err) => {
            let outcome = super::sync::classify_sync_err(err, global.json, "status");
            match outcome {
                super::sync::RunOutcome::Validation => std::process::exit(1),
                super::sync::RunOutcome::Tree | super::sync::RunOutcome::UsageError => {
                    std::process::exit(2)
                }
                super::sync::RunOutcome::Exec => std::process::exit(2),
                super::sync::RunOutcome::Ok => Ok(()),
            }
        }
    }
}

fn render(report: &SyncReport, json: bool) {
    let mut per_pack: BTreeMap<String, usize> = BTreeMap::new();
    for s in &report.steps {
        *per_pack.entry(s.pack.clone()).or_insert(0) += 1;
    }
    if json {
        let packs: Vec<serde_json::Value> = per_pack
            .iter()
            .map(|(pack, &count)| {
                let state = if count == 0 { "clean" } else { "would_update" };
                serde_json::json!({
                    "path": pack,
                    "state": state,
                    "drift_count": count,
                })
            })
            .collect();
        let clean = per_pack.values().all(|&n| n == 0);
        let doc = serde_json::json!({
            "verb": "status",
            "clean": clean,
            "packs": packs,
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else if per_pack.is_empty() {
        println!("clean");
    } else {
        for (pack, count) in &per_pack {
            if *count == 0 {
                println!("clean    {pack}");
            } else {
                println!("would-update {count:>3} {pack}");
            }
        }
    }
}

fn emit_error(json: bool, kind: &str, msg: &str) {
    if json {
        let doc = serde_json::json!({
            "verb": "status",
            "error": { "kind": kind, "message": msg },
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        eprintln!("grex status: {msg}");
    }
}
