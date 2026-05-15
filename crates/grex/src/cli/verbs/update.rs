//! `grex update` — refresh + re-install across packs.
//!
//! v1.4.0 ships `update` as a thin alias of `sync` because the existing
//! sync pipeline already re-runs install actions on lockfile delta
//! (`grex_core::sync::run`). The alias keeps the verb discoverable in
//! `--help` and reserves a dedicated dispatch slot for v2 if pull-then-
//! install ever needs distinct lifecycle hooks.

use crate::cli::args::{GlobalFlags, SyncArgs, UpdateArgs};
use anyhow::Result;
use tokio_util::sync::CancellationToken;

pub fn run(args: UpdateArgs, global: &GlobalFlags, cancel: &CancellationToken) -> Result<()> {
    let sync_args = SyncArgs {
        recursive: true,
        pack_root: args.pack.as_ref().map(std::path::PathBuf::from),
        pack: None,
        dry_run: false,
        quiet: false,
        no_validate: false,
        ref_override: None,
        only: Vec::new(),
        force: false,
        force_prune: false,
        force_prune_with_ignored: false,
        quarantine: false,
        parallel: None,
        retain_days: None,
    };
    super::sync::run(sync_args, global, cancel)
}
