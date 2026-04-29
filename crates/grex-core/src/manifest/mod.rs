//! Intent event log (`<workspace>/.grex/events.jsonl`).
//!
//! The event log is an **append-only JSONL log** of [`Event`]s. The current
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
//!
//! # Path convention (v2.0.0)
//!
//! The canonical event-log path is `<workspace>/.grex/events.jsonl`. This
//! supersedes the v1.x `<workspace>/grex.jsonl` location. v1.x event logs
//! are auto-migrated by [`ensure_event_log_migrated`] on first access.

pub mod append;
pub mod compact;
pub mod error;
pub mod event;
pub mod fold;
pub mod path;

pub use append::{append_event, read_all};
pub use compact::compact;
pub use error::ManifestError;
pub use event::{Event, PackId, PackState, ACTION_ERROR_SUMMARY_MAX, SCHEMA_VERSION};
pub use fold::fold;
pub use path::{
    ensure_event_log_migrated, event_log_path, find_workspace_root, EVENT_LOG_REL,
    LEGACY_EVENT_LOG_REL,
};
