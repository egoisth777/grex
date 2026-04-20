//! Resolved-state lockfile (`grex.lock.jsonl`).
//!
//! Unlike the manifest, the lockfile is **not** an event log. It is a flat
//! JSONL snapshot of the current state of each pack — one line per pack —
//! rewritten atomically on every update.

pub mod entry;
pub mod hash;
pub mod io;

pub use entry::{LockEntry, LockfileError};
pub use hash::compute_actions_hash;
pub use io::{read_lockfile, write_lockfile};
