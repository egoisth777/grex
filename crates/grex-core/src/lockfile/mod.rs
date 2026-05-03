//! Resolved-state lockfile (`grex.lock.jsonl`).
//!
//! Unlike the manifest, the lockfile is **not** an event log. It is a flat
//! JSONL snapshot of the current state of each pack — one line per pack —
//! rewritten atomically on every update.

pub mod distributed;
pub mod entry;
pub mod hash;
pub mod io;
// v1.3.1 — pure model-level lockfile writer (B14 branch-carry fix).
// Co-located with the existing IO surface so the per-entry data
// transform sits next to the file-level atomic write. The Lean
// theorem `Grex.Lockfile.lockfile_branch_mirrors_manifest_ref`
// formalises the contract this module realises.
pub mod writer;

// v1.2.0 Stage 1.h migrator — `pub` so the CLI's `--migrate-lockfile`
// dispatcher and the dedicated `grex migrate-lockfile` subcommand can
// reach it via the long path `grex_core::lockfile::migrate_v1_1_1::*`.
// Deliberately **not re-exported** below: walker / sync / ls / doctor /
// add / rm must NOT call the migrator (Stage 0 LOCKED decision #5,
// removable-module isolation rule). The module-isolation lint at
// `crates/grex-core/tests/lockfile_migrator_isolation.rs` enforces the
// rule mechanically.
pub mod migrate_v1_1_1;

pub use distributed::{
    detect_legacy_lockfile, meta_lockfile_path, read_lockfile_tree, read_meta_lockfile,
    write_meta_lockfile,
};
pub use entry::{LockEntry, LockfileError};
pub use hash::compute_actions_hash;
pub use io::{read_lockfile, write_lockfile};
// v1.3.1 — `write_entry_from_child` is the per-entry data-transform
// the W2+W4 sync-side plumbing invokes to map a manifest `ChildRef`
// into a fresh `LockEntry` with the manifest `ref:` carried into the
// `branch` field (B14 fix). The existing file-level `write_lockfile`
// stays the canonical lockfile-write surface; the two functions are
// orthogonal (one is per-entry, the other is per-file).
pub use writer::{branch_of, write_entry as write_entry_from_child};
