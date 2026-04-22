//! `#[tool]` + `#[tool_router]` registration for [`crate::GrexMcpServer`].
//!
//! Per `tasks.md` Stage 6 §6.1.v.b and `spec.md` rmcp-wiring note #4, the
//! 11 verbs are registered via rmcp's proc-macro pair on a single
//! inherent impl block. Each method here is a thin shim that:
//!
//! 1. Carries the spec-frozen `name`, `description`, and
//!    `annotations(read_only_hint, destructive_hint)` attribute payload.
//! 2. Forwards `(state, Parameters<P>)` to the verb module's free
//!    `handle()` function — the unit of business logic and the unit
//!    that's directly testable without the macro machinery in scope.
//!
//! The `#[tool_router]` macro inspects this block and emits
//! `GrexMcpServer::tool_router() -> ToolRouter<Self>`. We deliberately
//! omit the macro's `server_handler` flag because the existing
//! `impl ServerHandler for GrexMcpServer` block in `lib.rs` carries our
//! custom `get_info()` (Stage 5 protocol-version pin + custom
//! Implementation metadata). We hand-wire `call_tool` + `list_tools`
//! against `Self::tool_router()` from the trait impl.

use rmcp::{
    ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult, tool,
    tool_router,
};
use tokio_util::sync::CancellationToken;

use super::{
    add::AddParams, doctor::DoctorParams, exec::ExecParams, import::ImportParams,
    init::InitParams, ls::LsParams, rm::RmParams, run::RunParams, status::StatusParams,
    sync::SyncParams, update::UpdateParams,
};
use crate::GrexMcpServer;

#[tool_router(router = tool_router, vis = "pub(crate)")]
impl GrexMcpServer {
    /// Initialise a grex workspace.
    #[tool(
        name = "init",
        description = "Initialise a grex workspace.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn tool_init(
        &self,
        params: Parameters<InitParams>,
    ) -> Result<CallToolResult, McpError> {
        super::init::handle(&self.state, params).await
    }

    /// Register and clone a pack.
    #[tool(
        name = "add",
        description = "Register and clone a pack.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn tool_add(
        &self,
        params: Parameters<AddParams>,
    ) -> Result<CallToolResult, McpError> {
        super::add::handle(&self.state, params).await
    }

    /// Unregister a pack (runs teardown unless `--skip-teardown`).
    #[tool(
        name = "rm",
        description = "Unregister a pack (runs teardown unless --skip-teardown).",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn tool_rm(&self, params: Parameters<RmParams>) -> Result<CallToolResult, McpError> {
        super::rm::handle(&self.state, params).await
    }

    /// List registered packs.
    #[tool(
        name = "ls",
        description = "List registered packs.",
        annotations(read_only_hint = true, destructive_hint = false)
    )]
    async fn tool_ls(&self, params: Parameters<LsParams>) -> Result<CallToolResult, McpError> {
        super::ls::handle(&self.state, params).await
    }

    /// Report drift + installed state.
    #[tool(
        name = "status",
        description = "Report drift + installed state.",
        annotations(read_only_hint = true, destructive_hint = false)
    )]
    async fn tool_status(
        &self,
        params: Parameters<StatusParams>,
    ) -> Result<CallToolResult, McpError> {
        super::status::handle(&self.state, params).await
    }

    /// Sync all packs recursively.
    ///
    /// `cancel` is a `tokio_util::sync::CancellationToken` injected by
    /// rmcp's `#[tool]` machinery (see `FromContextPart for
    /// CancellationToken` in rmcp 1.5.0). It is the per-request
    /// `RequestContext::ct` clone, automatically fired by rmcp's service
    /// loop when a `notifications/cancelled` for this request id arrives
    /// (see `crates/grex-mcp/tests/cancellation.rs` and Stage 7 of
    /// `openspec/changes/feat-m7-1-mcp-server/tasks.md`).
    #[tool(
        name = "sync",
        description = "Sync all packs recursively.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn tool_sync(
        &self,
        params: Parameters<SyncParams>,
        cancel: CancellationToken,
    ) -> Result<CallToolResult, McpError> {
        super::sync::handle(&self.state, params, cancel).await
    }

    /// Update one or more packs (re-resolve refs, reinstall).
    #[tool(
        name = "update",
        description = "Update one or more packs (re-resolve refs, reinstall).",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn tool_update(
        &self,
        params: Parameters<UpdateParams>,
    ) -> Result<CallToolResult, McpError> {
        super::update::handle(&self.state, params).await
    }

    /// Check manifest + gitignore + on-disk drift.
    #[tool(
        name = "doctor",
        description = "Check manifest + gitignore + on-disk drift.",
        annotations(read_only_hint = true, destructive_hint = false)
    )]
    async fn tool_doctor(
        &self,
        params: Parameters<DoctorParams>,
    ) -> Result<CallToolResult, McpError> {
        super::doctor::handle(&self.state, params).await
    }

    /// Import packs from a `REPOS.json` meta-repo index.
    #[tool(
        name = "import",
        description = "Import packs from a REPOS.json meta-repo index.",
        annotations(read_only_hint = false, destructive_hint = false)
    )]
    async fn tool_import(
        &self,
        params: Parameters<ImportParams>,
    ) -> Result<CallToolResult, McpError> {
        super::import::handle(&self.state, params).await
    }

    /// Run a declared action across matching packs.
    #[tool(
        name = "run",
        description = "Run a declared action across matching packs.",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn tool_run(&self, params: Parameters<RunParams>) -> Result<CallToolResult, McpError> {
        super::run::handle(&self.state, params).await
    }

    /// Execute a command across matching packs (no shell — see
    /// `agent_safety.rs` for the rationale and proof).
    #[tool(
        name = "exec",
        description = "Execute a command across matching packs (no shell).",
        annotations(read_only_hint = false, destructive_hint = true)
    )]
    async fn tool_exec(
        &self,
        params: Parameters<ExecParams>,
    ) -> Result<CallToolResult, McpError> {
        super::exec::handle(&self.state, params).await
    }
}
