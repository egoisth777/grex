//! Intent event log (`grex.jsonl`).
//!
//! The manifest is an **append-only JSONL log** of [`Event`]s. The current
//! state of each pack is derived by folding the log into a
//! `HashMap<PackId, PackState>` via [`fold::fold`].
//!
//! # Why a log, not a snapshot?
//!
//! * Intent is preserved (every `add`/`update`/`rm` is traceable).
//! * Append writes are atomic at the syscall level for small lines.
//! * Crash recovery: a torn trailing line can be discarded.
//!
//! Compaction ([`compact::compact`]) periodically rewrites the log from the
//! folded state to bound its size; this is an atomic temp+rename.

pub mod append;
pub mod compact;
pub mod error;
pub mod event;
pub mod fold;

pub use append::{append_event, read_all};
pub use compact::compact;
pub use error::ManifestError;
pub use event::{Event, PackId, PackState, SCHEMA_VERSION};
pub use fold::fold;
