# rmcp Research for grex M7 MCP Server Tests

Research compiled 2026-04-21 via context7 (`/websites/rs_rmcp`) and GitHub
`modelcontextprotocol/rust-sdk`.

## 1. Version recommendation

- **Latest stable: `rmcp = "1.5.0"` (published 2026-04-16).**
- Pre-1.0 line (`0.15` – `0.17`, Feb 2026) still exists for backward-compat.
- For a greenfield grex M7 server, **pin `rmcp = "1.5"`** and enable feature
  flags: `["server", "client", "macros", "transport-io"]`
  (add `"transport-child-process"` only if integration-testing a spawned
  binary; not needed for unit/integration tests that use in-memory duplex).
- Spec protocol version we target: `2025-11-25` (current MCP spec revision).
  `2025-06-18` and `2025-03-26` are still accepted by `rmcp`.

## 2. Key types / traits

| Purpose                         | Item                                                       |
| ------------------------------- | ---------------------------------------------------------- |
| Server trait                    | `rmcp::ServerHandler`                                      |
| Client trait                    | `rmcp::ClientHandler`                                      |
| Tool registry                   | `rmcp::handler::server::tool::ToolRouter`                  |
| Tool param wrapper              | `rmcp::handler::server::wrapper::Parameters<T>`            |
| Tool return wrapper             | `rmcp::handler::server::wrapper::Json<T>`                  |
| Macros                          | `#[tool_router]`, `#[tool]`, `#[tool_handler]`             |
| Start server/client             | `ServiceExt::serve(transport)` / `serve_with_ct(tr, ct)`   |
| Stdio transport                 | `rmcp::transport::io::stdio()` → `(Stdin, Stdout)`         |
| Cancellation per-request        | `RequestContext::ct: CancellationToken`                    |
| Client-side cancel              | `RequestHandle::cancel(Some("reason"))`                    |
| JSON-RPC errors                 | `rmcp::model::{ErrorData, ErrorCode}`                      |
| Tool result                     | `rmcp::model::CallToolResult` (`is_error`, `structured_content`) |

`ErrorCode` constants: `METHOD_NOT_FOUND (-32601)`, `INVALID_PARAMS (-32602)`,
`INTERNAL_ERROR (-32603)`, `INVALID_REQUEST (-32600)`, `PARSE_ERROR (-32700)`,
plus MCP-specific `RESOURCE_NOT_FOUND`, `URL_ELICITATION_REQUIRED`.

## 3. Minimal server (stdio)

```rust
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::{Parameters, Json},
    schemars, tool, tool_router, tool_handler,
    transport::io::stdio,
};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, schemars::JsonSchema)]
struct EchoArgs { msg: String }
#[derive(Serialize, schemars::JsonSchema)]
struct EchoOut { echoed: String }

pub struct GrexServer;

#[tool_router(server_handler)]
impl GrexServer {
    #[tool(description = "Echo a message")]
    async fn echo(&self, Parameters(a): Parameters<EchoArgs>) -> Json<EchoOut> {
        Json(EchoOut { echoed: a.msg })
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let running = GrexServer.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
```

`schemars::JsonSchema` auto-generates the `inputSchema` / `outputSchema` on the
wire — no hand-written JSON schema.

## 4. In-process test scaffolding (duplex, no subprocess)

Pattern taken verbatim from `crates/rmcp/tests/test_notification.rs` and
`test_message_protocol.rs` in the upstream repo — `tokio::io::duplex(4096)`
gives paired `AsyncRead + AsyncWrite` halves that both sides hand to
`ServiceExt::serve`.

```rust
use rmcp::ServiceExt;

#[tokio::test]
async fn echo_roundtrip() -> anyhow::Result<()> {
    let (server_io, client_io) = tokio::io::duplex(4096);

    // Spawn server; handshake runs inside serve()
    let server = tokio::spawn(async move {
        GrexServer.serve(server_io).await?.waiting().await
    });

    // Client = bare `()` handler (no sampling/roots needed)
    let client = ().serve(client_io).await?;

    let tools = client.peer().list_tools(Default::default()).await?;
    assert!(tools.tools.iter().any(|t| t.name == "echo"));

    let out = client.peer().call_tool(
        rmcp::model::CallToolRequestParams::new("echo")
            .with_arguments(serde_json::json!({ "msg": "hi" })),
    ).await?;
    assert!(!out.is_error.unwrap_or(false));

    client.cancel().await?;            // triggers graceful shutdown
    server.abort();
    Ok(())
}
```

## 5. Cancellation test pattern

Server tools receive `RequestContext` whose `ct: CancellationToken` is tripped
when the peer sends `notifications/cancelled` (the MCP spec method; `rmcp` maps
the older `$/cancelRequest` name to this).

```rust
#[tool(description = "long-running")]
async fn slow(&self, ctx: RequestContext<RoleServer>) -> Result<Json<()>, ErrorData> {
    tokio::select! {
        _ = ctx.ct.cancelled() => Err(ErrorData::new(
            ErrorCode::INTERNAL_ERROR, "cancelled by client", None)),
        _ = tokio::time::sleep(std::time::Duration::from_secs(60)) => Ok(Json(())),
    }
}
```

Test side — `peer().call_tool()` returns a future; to cancel, use
`peer().send_request(...)` which yields a `RequestHandle`, then
`handle.cancel(Some("test".into())).await`. Assert the awaited result is an
error and the server-side observed `ct.cancelled()`.

## 6. Transport / framing customization

- `ServiceExt::serve` accepts anything implementing `IntoTransport`:
  `(R, W)` tuples where `R: AsyncRead`, `W: AsyncWrite`; `Sink/Stream` pairs;
  raw `Transport` impls; `TokioChildProcess`.
- Line-delimited JSON-RPC framing is built into the `AsyncRead/AsyncWrite`
  adapter — no extra `Framed<LinesCodec>` needed.
- For tests, always prefer `tokio::io::duplex(N)` over `TokioChildProcess`:
  no zombie processes, deterministic, ~instant.

## 7. Known pitfalls / breakage

1. **`CallToolRequestParam` (singular) deprecated since 0.13.0** — use
   `CallToolRequestParams`.
2. **Task-based tools** (new post-0.14): if a tool declares
   `TaskSupport::Required`, non-task invocations get `-32601`. Leave default
   (`Forbidden`) unless we explicitly opt into long-running tasks in M7.
3. **`#[tool_router(server_handler)]` sugar** (recent) collapses
   `#[tool_router]` + `#[tool_handler]`; can only be used when the server has
   tools only — for mixed tools+prompts keep them separate.
4. **HTTP transport** validates `MCP-Protocol-Version` header against
   `KNOWN_VERSIONS`; if grex ever adds HTTP, pin the header to our negotiated
   version in tests.
5. **Client MUST NOT cancel `initialize`** (spec rule, enforced by `rmcp`).
6. Features matter: `stdio()` needs `transport-io`; macros need `macros`; the
   bare `()` test-client handler needs `client`.

## 8. MCP spec testing requirements (grex conformance checklist)

- Initialize handshake: send `initialize` → receive `InitializeResult` →
  send `notifications/initialized`. `rmcp::serve_client_with_ct_inner` does
  all three for us; tests just `await serve()`.
- Capability negotiation: `ServerCapabilities::builder().enable_tools()` etc.
  Tests assert `client.peer().peer_info()` contains the expected caps.
- Shutdown: drop / `cancel()` the `RunningService`; no explicit `shutdown`
  RPC in MCP — transport close is the signal.
- Cancellation: `notifications/cancelled { requestId, reason? }`; must not
  target the initialize request.
- Officially published conformance vectors: **none yet** as of 2026-04;
  `modelcontextprotocol/inspector` is the closest to a conformance harness.
  Roll our own vectors against the TS schema at
  `modelcontextprotocol/modelcontextprotocol/schema/2025-11-25/schema.json`.

---

**For grex M7 MCP server tests, bootstrap with these imports and this scaffolding:**

```rust
use rmcp::{
    ServerHandler, ServiceExt, RoleServer,
    handler::server::wrapper::{Parameters, Json},
    model::{CallToolRequestParams, ErrorCode, ErrorData},
    service::RequestContext,
    schemars, tool, tool_router, tool_handler,
};
use tokio_util::sync::CancellationToken;

// Cargo.toml:
// rmcp = { version = "1.5", features = ["server","client","macros","transport-io"] }
// tokio = { version = "1", features = ["full"] }
// schemars = "0.8"; serde = { version = "1", features = ["derive"] }

#[tokio::test]
async fn grex_tool_roundtrip() -> anyhow::Result<()> {
    let (s, c) = tokio::io::duplex(4096);
    let server = tokio::spawn(async move { GrexServer.serve(s).await?.waiting().await });
    let client = ().serve(c).await?;
    // ... peer().list_tools / call_tool / cancel assertions ...
    client.cancel().await?;
    server.abort();
    Ok(())
}
```
