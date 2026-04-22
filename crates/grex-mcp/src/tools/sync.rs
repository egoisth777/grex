//! `sync` tool — drive the M3 Stage B end-to-end pipeline.
//!
//! Only verb (alongside `teardown`, which is NOT exposed as an MCP tool)
//! that has a real core implementation today. Bridges
//! [`grex_core::sync::run`] (synchronous, blocking) onto the async
//! rmcp dispatch via [`tokio::task::spawn_blocking`]. Cancellation token
//! comes from the per-request `RequestContext::ct` plumbed in Stage 7
//! and threaded through `tool_router`'s `&self` shim.

use crate::error::{packop_error, CancelledExt};
use grex_core::sync::{self, SyncOptions};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Args for `sync`. Mirrors the CLI's `--json` shape — JSON-only fields,
/// no positional args. `pack_root` is required at the MCP edge because
/// the legacy "no-arg stub print" branch makes no sense for an agent.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SyncParams {
    /// Pack root: directory holding `.grex/pack.yaml` or the YAML file itself.
    pub pack_root: PathBuf,
    /// Workspace directory for cloned children.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Plan actions without touching the filesystem.
    #[serde(default)]
    pub dry_run: bool,
    /// Skip plan-phase validators. Debug-only escape hatch.
    #[serde(default)]
    pub no_validate: bool,
    /// Override the default ref for every pack.
    #[serde(default, rename = "ref")]
    pub ref_override: Option<String>,
    /// Restrict sync to packs whose path matches one of the globs.
    #[serde(default)]
    pub only: Vec<String>,
    /// Re-execute every pack even when its `actions_hash` is unchanged.
    #[serde(default)]
    pub force: bool,
    /// Max parallel pack ops. `None` → core default; `0` → unbounded; `1` → serial.
    #[serde(default)]
    pub parallel: Option<u32>,
}

pub(crate) async fn handle(
    state: &crate::ServerState,
    p: Parameters<SyncParams>,
    cancel: CancellationToken,
) -> Result<CallToolResult, McpError> {
    run_with_cancel(state, p.0, cancel).await
}

async fn run_with_cancel(
    _state: &crate::ServerState,
    p: SyncParams,
    cancel: CancellationToken,
) -> Result<CallToolResult, McpError> {
    // Test-only block-until-cancelled hook — gated behind the `test-hooks`
    // cargo feature so it compiles out of release `grex serve` binaries
    // entirely (no exposed test surface, no runtime atomic load). The
    // cancellation integration test
    // (`crates/grex-mcp/tests/cancellation.rs::notifications_cancelled_aborts_inflight_sync`)
    // enables `test-hooks` via the dev-dep self-edge in `Cargo.toml`, then
    // flips the toggle on so the in-flight handler awaits its per-request
    // `CancellationToken` instead of running the (microseconds-fast)
    // `sync::run` and losing the race against `notifications/cancelled`.
    #[cfg(any(test, feature = "test-hooks"))]
    if test_hooks::block_until_cancelled() {
        cancel.cancelled().await;
        return Err(McpError::from(CancelledExt));
    }

    let opts = build_opts(&p);
    let pack_root = p.pack_root.clone();

    // `sync::run` is sync and may block on filesystem / git. Push it onto a
    // blocking thread so the rmcp dispatcher's reactor stays responsive. The
    // join handle is `select!`'d against `cancel.cancelled()` so the request
    // can return -32800 promptly (the OS thread continues briefly per the
    // documented leak window in `pack_lock::acquire_cancellable`).
    let cancel_clone = cancel.clone();
    let handle = tokio::task::spawn_blocking(move || sync::run(&pack_root, &opts, &cancel_clone));

    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(McpError::from(CancelledExt)),
        joined = handle => joined,
    };

    match outcome {
        Ok(Ok(report)) => Ok(success_envelope(&report)),
        Ok(Err(err)) => Ok(packop_error(&format!("{err}"))),
        Err(join_err) => Ok(packop_error(&format!("internal: blocking task failed: {join_err}"))),
    }
}

/// Test-only knobs. `block_until_cancelled` is consulted by
/// `run_with_cancel` at the top of every call when compiled with the
/// `test-hooks` cargo feature; flipping it on turns the handler into
/// "await cancel; return -32800". This is the deterministic substitute
/// for a slow git fetch that the Stage 7 task description anticipated.
/// The whole module is gated behind `cfg(any(test, feature = "test-hooks"))`
/// so neither the atomic nor the setter ships in default-feature release
/// builds.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
mod test_hooks {
    use std::sync::atomic::{AtomicBool, Ordering};

    static BLOCK: AtomicBool = AtomicBool::new(false);

    pub fn block_until_cancelled() -> bool {
        BLOCK.load(Ordering::SeqCst)
    }

    pub fn set_block_until_cancelled(v: bool) {
        BLOCK.store(v, Ordering::SeqCst);
    }
}

/// Test-only setter for the block-until-cancelled hook. See
/// [`test_hooks`] for rationale. Hidden from rustdoc and compiled out
/// unless the `test-hooks` cargo feature is enabled.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn __test_set_block_until_cancelled(v: bool) {
    test_hooks::set_block_until_cancelled(v);
}

fn build_opts(p: &SyncParams) -> SyncOptions {
    let only = if p.only.is_empty() { None } else { Some(p.only.clone()) };
    SyncOptions::new()
        .with_dry_run(p.dry_run)
        .with_validate(!p.no_validate)
        .with_workspace(p.workspace.clone())
        .with_ref_override(p.ref_override.clone())
        .with_only_patterns(only)
        .with_force(p.force)
}

fn success_envelope(report: &grex_core::sync::SyncReport) -> CallToolResult {
    let body =
        format!("sync ok: {} step(s); halted={}", report.steps.len(), report.halted.is_some());
    CallToolResult::success(vec![Content::text(body)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn sync_params_schema_resolves() {
        let _ = schema_for_type::<SyncParams>();
    }

    /// Happy-path: against an empty / non-existent pack root the core
    /// returns a `SyncError`; we map to a `pack_op` envelope. The test's
    /// goal is "handler runs and returns a structured envelope" — the
    /// exact error text is core's domain.
    #[tokio::test]
    async fn sync_happy_path_returns_envelope() {
        let s = crate::ServerState::for_tests();
        let p = SyncParams {
            pack_root: std::env::temp_dir().join("grex-mcp-nonexistent-pack"),
            workspace: None,
            dry_run: true,
            no_validate: true,
            ref_override: None,
            only: Vec::new(),
            force: false,
            parallel: None,
        };
        let r = handle(&s, Parameters(p), CancellationToken::new()).await.unwrap();
        // We expect failure — pack root does not exist. Either way the
        // tool MUST return Ok(envelope), not a JSON-RPC -32xxx.
        assert!(r.is_error.is_some(), "must set isError flag");
    }
}
