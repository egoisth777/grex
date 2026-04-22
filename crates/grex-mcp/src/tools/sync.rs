//! `sync` tool — drive the M3 Stage B end-to-end pipeline.
//!
//! Only verb (alongside `teardown`, which is NOT exposed as an MCP tool)
//! that has a real core implementation today. Bridges
//! [`grex_core::sync::run`] (synchronous, blocking) onto the async
//! rmcp dispatch via [`tokio::task::spawn_blocking`]. Cancellation token
//! is captured from the request context (Stage 7 wires it through;
//! Stage 6 plumbs the call site).

use crate::error::{CancelledExt, packop_error};
use grex_core::sync::{self, SyncOptions};
use rmcp::{
    ErrorData as McpError,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
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
    Parameters(p): Parameters<SyncParams>,
) -> Result<CallToolResult, McpError> {
    // Stage 7 will replace this fresh token with the per-request token from
    // `RequestContext::ct`. Stage 6's contract is "tool body runs to completion
    // and maps result"; cancellation plumbing is deferred per Stage 7.1.
    let cancel = CancellationToken::new();
    run_with_cancel(state, p, cancel).await
}

async fn run_with_cancel(
    _state: &crate::ServerState,
    p: SyncParams,
    cancel: CancellationToken,
) -> Result<CallToolResult, McpError> {
    let opts = build_opts(&p);
    let pack_root = p.pack_root.clone();

    // `sync::run` is sync and may block on filesystem / git. Push it onto a
    // blocking thread so the rmcp dispatcher's reactor stays responsive. The
    // join handle is `select!`'d against `cancel.cancelled()` so the request
    // can return -32800 promptly (the OS thread continues briefly per the
    // documented leak window in `pack_lock::acquire_cancellable`).
    let cancel_clone = cancel.clone();
    let handle = tokio::task::spawn_blocking(move || {
        sync::run(&pack_root, &opts, &cancel_clone)
    });

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
    let body = format!(
        "sync ok: {} step(s); halted={}",
        report.steps.len(),
        report.halted.is_some()
    );
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
        let r = handle(&s, Parameters(p)).await.unwrap();
        // We expect failure — pack root does not exist. Either way the
        // tool MUST return Ok(envelope), not a JSON-RPC -32xxx.
        assert!(r.is_error.is_some(), "must set isError flag");
    }
}
