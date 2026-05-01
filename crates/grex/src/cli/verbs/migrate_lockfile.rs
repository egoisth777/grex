//! `grex migrate-lockfile` — opt-in v1.1.x → v1.2.0 lockfile migrator.
//!
//! v1.2.1 item 2 (per `openspec/feat-v1.2.1/spec.md` §"CLI `grex
//! migrate-lockfile` dispatcher"). Thin shim over the v1.2.0 Stage 1.h
//! library migrator [`grex_core::lockfile::migrate_v1_1_1::migrate_v1_1_1_lockfile`],
//! which is held under the `pub mod migrate_v1_1_1` long path so that
//! walker / sync / ls / doctor / add / rm cannot reach it (Stage 0
//! LOCKED decision #5 — removable-module isolation).
//!
//! # Behaviour
//!
//! * `--workspace <path>` selects the meta whose
//!   `<workspace>/.grex/grex.lock.jsonl` is migrated. Defaults to the
//!   current working directory.
//! * `--dry-run` (`-n`) inspects the on-disk shape and reports what
//!   would happen without writing. Lockfile bytes are unchanged.
//! * Without `--dry-run`, the migrator rewrites the lockfile in place
//!   (atomic temp+rename via the library's existing
//!   `write_meta_lockfile`).
//! * Idempotent: running twice on a v1.2.0+ lockfile is a no-op.
//! * Exits 0 on success (including the no-lockfile and already-migrated
//!   no-op paths). Exits 1 on IO / corruption errors surfaced by the
//!   migrator.

use crate::cli::args::{GlobalFlags, MigrateLockfileArgs};
use anyhow::Result;
use grex_core::lockfile::migrate_v1_1_1::migrate_v1_1_1_lockfile;
use grex_core::lockfile::{detect_legacy_lockfile, meta_lockfile_path};
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn run(
    args: MigrateLockfileArgs,
    global: &GlobalFlags,
    _cancel: &CancellationToken,
) -> Result<()> {
    let workspace = match args.workspace.clone() {
        Some(p) => p,
        None => std::env::current_dir()?,
    };
    let dry_run = args.dry_run || global.dry_run;

    if dry_run {
        run_dry_run(&workspace, global.json)
    } else {
        run_migrate(&workspace, global.json)
    }
}

/// `--dry-run` path. Inspects the lockfile shape via the library's
/// `detect_legacy_lockfile` predicate (which short-circuits on missing
/// or already-v1.2.0 files) and reports the would-be outcome without
/// touching the bytes.
fn run_dry_run(workspace: &std::path::Path, json: bool) -> Result<()> {
    let lockfile = meta_lockfile_path(workspace);
    if !lockfile.exists() {
        emit_outcome(json, &lockfile, Outcome::NoLockfile, true);
        return Ok(());
    }
    let legacy = detect_legacy_lockfile(workspace)?;
    let outcome = if legacy { Outcome::WouldMigrate } else { Outcome::AlreadyMigrated };
    emit_outcome(json, &lockfile, outcome, true);
    Ok(())
}

/// Wet-run path. Calls the library migrator and reports its
/// [`grex_core::lockfile::migrate_v1_1_1::MigrationReport`] outcome.
fn run_migrate(workspace: &std::path::Path, json: bool) -> Result<()> {
    let report = migrate_v1_1_1_lockfile(workspace)?;
    let outcome = if report.no_lockfile {
        Outcome::NoLockfile
    } else if report.already_migrated {
        Outcome::AlreadyMigrated
    } else {
        Outcome::Migrated { entries: report.migrated_entries }
    };
    emit_outcome(json, &report.lockfile, outcome, false);
    Ok(())
}

#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// Meta has no lockfile; nothing to migrate.
    NoLockfile,
    /// Lockfile already in v1.2.0 shape; no rewrite needed.
    AlreadyMigrated,
    /// `--dry-run` only: legacy v1.1.x shape detected; would migrate.
    WouldMigrate,
    /// Wet-run only: legacy v1.1.x shape was rewritten to v1.2.0.
    Migrated { entries: usize },
}

fn emit_outcome(json: bool, lockfile: &Path, outcome: Outcome, dry_run: bool) {
    if json {
        let (status, entries) = match outcome {
            Outcome::NoLockfile => ("no_lockfile", None),
            Outcome::AlreadyMigrated => ("already_migrated", None),
            Outcome::WouldMigrate => ("would_migrate", None),
            Outcome::Migrated { entries } => ("migrated", Some(entries)),
        };
        let doc = serde_json::json!({
            "verb": "migrate-lockfile",
            "lockfile": lockfile.display().to_string(),
            "status": status,
            "entries": entries,
            "dry_run": dry_run,
        });
        if let Ok(s) = serde_json::to_string(&doc) {
            println!("{s}");
        }
    } else {
        let path = lockfile.display();
        match outcome {
            Outcome::NoLockfile => {
                println!("no lockfile at {path}; nothing to migrate");
            }
            Outcome::AlreadyMigrated => {
                println!("{path}: already on v1.2.0 schema; no-op");
            }
            Outcome::WouldMigrate => {
                println!("{path}: v1.1.x → v1.2.0 (would migrate; --dry-run, no write)");
            }
            Outcome::Migrated { entries } => {
                println!("{path}: migrated {entries} entries v1.1.x → v1.2.0");
            }
        }
    }
}
