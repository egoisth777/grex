//! grex-mcp — MCP-native server for grex (M7).
//!
//! Stage 5 wires the [`rmcp`] framework: the server speaks the MCP
//! 2025-06-18 wire protocol over any [`rmcp::transport`] (stdio in
//! production, [`tokio::io::duplex`] in tests). The handshake +
//! `tools/list` (returning empty) + transport-close shutdown are live;
//! the 11 tool handlers land in Stage 6, and cancellation in Stage 7.
//!
//! # Stdout discipline
//!
//! The MCP stdio transport multiplexes **only JSON-RPC bytes** on
//! `stdout`. All diagnostics MUST go to `stderr`. [`GrexMcpServer::run`]
//! installs a `tracing_subscriber::fmt` writer pinned to `stderr` —
//! idempotently, so test reuse and `serve`-from-CLI both work. The
//! `no_println_lint.rs` integration test enforces zero `println!` /
//! `print!` macros under `src/` to prevent regressions.

#![deny(unsafe_code)]

use std::{future::Future, sync::Arc};

use grex_core::{Registry, Scheduler};
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
    model::{
        CallToolRequestParams, CallToolResult, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo,
    },
    service::{MaybeSendFuture, RequestContext},
    transport::IntoTransport,
};

pub mod error;
pub mod tools;

pub use error::{CancelledExt, REQUEST_CANCELLED};

/// Re-export the registered-tool name list so `serve` smoke tests +
/// downstream crates have a stable handle on the surface.
pub use tools::VERBS_11_EXPOSED_AS_TOOLS;

/// Shared, immutable-after-build state every tool handler reads.
///
/// Stage-5 surface only — Stages 6/7 will widen the type to hold the
/// in-flight cancellation token map (`DashMap<RequestId, CancellationToken>`)
/// and any `ExecCtx` plumbing the verbs require. Fields are `Arc`-wrapped so
/// `ServerHandler::call_tool` can clone cheaply onto each spawn.
#[derive(Clone)]
pub struct ServerState {
    /// Bounded permit pool the verbs use for `--parallel N` semantics.
    pub scheduler: Arc<Scheduler>,
    /// Plugin registry resolving manifest verbs to plugin impls.
    pub registry: Arc<Registry>,
    /// Path to the `grex.jsonl` event-log manifest. Captured at server
    /// launch and immutable for the session (per spec §"Manifest binding").
    pub manifest_path: Arc<std::path::PathBuf>,
    /// Workspace root the server resolves relative paths against.
    pub workspace: Arc<std::path::PathBuf>,
}

impl ServerState {
    /// Build a `ServerState` from already-constructed core components.
    /// Stage 8 (`grex serve` CLI) will call this from `verbs/serve.rs`.
    pub fn new(
        scheduler: Scheduler,
        registry: Registry,
        manifest_path: std::path::PathBuf,
        workspace: std::path::PathBuf,
    ) -> Self {
        Self {
            scheduler: Arc::new(scheduler),
            registry: Arc::new(registry),
            manifest_path: Arc::new(manifest_path),
            workspace: Arc::new(workspace),
        }
    }

    /// Build a state suitable for in-process integration tests:
    /// single-permit scheduler, empty registry, current-dir paths. Used
    /// only by Stage 5 handshake / discipline tests where no tool body
    /// actually runs.
    pub fn for_tests() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Self::new(
            Scheduler::new(1),
            Registry::default(),
            cwd.join("grex.jsonl"),
            cwd,
        )
    }
}

/// The grex MCP server. One instance per `grex serve` invocation; one
/// instance per integration test. Cheap to construct; all state behind
/// `Arc` so `ServerHandler` impls can clone onto handler tasks for free.
#[derive(Clone)]
pub struct GrexMcpServer {
    pub(crate) state: ServerState,
}

impl GrexMcpServer {
    pub fn new(state: ServerState) -> Self {
        Self { state }
    }

    /// Drive the server against the given transport until it closes.
    ///
    /// Side effects:
    ///   1. Installs a `tracing_subscriber::fmt` writer pinned to `stderr`
    ///      (idempotent — repeat calls in tests are tolerated).
    ///   2. Hands ownership of `self` to rmcp's `ServiceExt::serve`, which
    ///      runs the JSON-RPC loop on the current Tokio runtime.
    ///   3. Returns when the transport closes or an unrecoverable framing
    ///      error occurs.
    ///
    /// # Errors
    /// Surfaces any `ServerInitializeError` from rmcp during the handshake.
    pub async fn run<T, E, A>(self, transport: T) -> Result<(), rmcp::service::ServerInitializeError>
    where
        T: IntoTransport<RoleServer, E, A>,
        E: std::error::Error + Send + Sync + 'static,
    {
        init_stderr_tracing();
        // Per-request cancellation is handled by rmcp's internal local_ct_pool
        // (see service.rs:766 / :948 / :989-991) — surfaced to handlers via
        // FromContextPart<CancellationToken>. We do NOT need serve_with_ct here;
        // that's a server-shutdown surface, not per-request. Stage 5 wiring note
        // #4 conflated the two — this comment supersedes that note for Stage 7.
        let running = self.serve(transport).await?;
        // Wait for transport close. `waiting()` returns once the service loop
        // exits cleanly (drop of peer / EOF on transport).
        let _quit_reason = running.waiting().await;
        Ok(())
    }
}

/// Pin `tracing` output to `stderr`, ensuring `stdout` carries only
/// JSON-RPC bytes. Idempotent: `set_global_default` is allowed to fail
/// with "already set" (test re-entry, daemon restart, embedded use).
fn init_stderr_tracing() {
    use tracing::subscriber::set_global_default;
    use tracing_subscriber::{EnvFilter, fmt};

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let subscriber = fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .finish();

    // Ignore "already set" — tests + repeat invocations both reach here.
    let _ = set_global_default(subscriber);
}

impl ServerHandler for GrexMcpServer {
    fn get_info(&self) -> ServerInfo {
        let mut info = ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_protocol_version(rmcp::model::ProtocolVersion::V_2025_06_18);
        let mut implementation = Implementation::default();
        implementation.name = "grex".into();
        implementation.title = Some("grex MCP server".into());
        implementation.version = env!("CARGO_PKG_VERSION").into();
        info.server_info = implementation;
        info.instructions = Some(
            "grex pack-orchestrator MCP surface. 11 tools reachable via tools/call; \
             cancellation via notifications/cancelled. See `.omne/cfg/mcp.md`."
                .into(),
        );
        info
    }

    /// Stage 6: return all 11 tools assembled from the
    /// `#[tool_router]`-generated `Self::tool_router()` aggregator.
    /// The router is rebuilt per-call for now (cheap — just a few
    /// hashmap inserts of `Arc`-cloned `Tool` values); Stage 7 may
    /// memoize it onto `ServerState` if profiling shows it matters.
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + MaybeSendFuture + '_ {
        let tools = Self::tool_router().list_all();
        std::future::ready(Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        }))
    }

    /// Stage 6: dispatch `tools/call` into the per-verb handler matching
    /// `params.name` via the `#[tool_router]`-generated aggregator. Per-
    /// tool argument deserialisation is handled by rmcp's `Parameters<P>`
    /// extractor; bad params yield `-32602`. Unknown tool names also
    /// yield `-32602` (rmcp's router default).
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResult, McpError>> + MaybeSendFuture + '_ {
        let tcc = rmcp::handler::server::tool::ToolCallContext::new(self, request, context);
        async move { Self::tool_router().call(tcc).await }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_state_for_tests_constructs() {
        let s = ServerState::for_tests();
        assert!(s.scheduler.max_parallelism() >= 1);
    }

    #[test]
    fn server_constructs() {
        let _ = GrexMcpServer::new(ServerState::for_tests());
    }
}
