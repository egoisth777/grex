//! Stage 2 of `feat-m7-1-mcp-server`: signature-only refactor that
//! threads `cancel: &tokio_util::sync::CancellationToken` as the FINAL
//! parameter through every core-verb `run()` entry point.
//!
//! This file is the RED for stage 2. It pins:
//!
//! 1. `grex_core::sync::run` accepts a borrowed `CancellationToken` as
//!    its final argument (compile-time check via the `as fn(...)` cast).
//! 2. A never-cancelled sentinel — i.e. `CancellationToken::new()` —
//!    flows through a no-op sync against an empty workspace and remains
//!    `is_cancelled() == false` afterwards. Stages 3-4 wire the actual
//!    `is_cancelled()` checks; stage 2 only proves the wiring compiles
//!    and is preserved.
//!
//! Per `openspec/changes/feat-m7-1-mcp-server/tasks.md` 2.T2.

use std::path::Path;

use grex_core::sync::{self, SyncOptions};
use tokio_util::sync::CancellationToken;

/// 2.T2: a freshly-constructed `CancellationToken` survives a no-op
/// `sync::run` against an empty pack root unchanged.
///
/// "Empty workspace" here means a tempdir with no pack manifest at the
/// root. `sync::run` returns a validation error because no pack is
/// discoverable — that is fine: the assertion we care about is that the
/// sentinel token is not flipped by anything in the call path. A real
/// cancel signal can only come from stages 3/4.
#[test]
fn cli_sentinel_never_cancels() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let pack_root: &Path = tmp.path();
    let opts = SyncOptions::new()
        .with_dry_run(true)
        .with_validate(false)
        .with_workspace(Some(tmp.path().join("workspace")));

    let cancel = CancellationToken::new();
    // We don't care whether the sync succeeds — only that the sentinel
    // remains un-cancelled across the call. `let _ = ` swallows the
    // Result so any future SyncError variant added by M7-3+ doesn't
    // break this proof.
    let _ = sync::run(pack_root, &opts, &cancel);

    assert!(
        !cancel.is_cancelled(),
        "CLI sentinel must never flip to cancelled — stage 2 is a wiring-only refactor",
    );
}
