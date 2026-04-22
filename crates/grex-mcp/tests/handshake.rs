//! Handshake tests for `GrexMcpServer`.
//!
//! Two layers of coverage cohabit this file:
//!
//! 1. **m7-1 Stage 5 tests (#5.T1 – #5.T3)** — the original rmcp-typed
//!    handshake assertions: protocol-version pinning at the typed
//!    `InitializeResult` boundary, batch silent-drop (rmcp 1.5.0
//!    limitation, see spec §"Known limitations"), and clean transport-
//!    close shutdown. They exercise the framework via the rmcp
//!    `Transport` trait directly.
//!
//! 2. **m7-2 Stage 1 tests (5 cases below)** — added by
//!    `feat-m7-2-mcp-test-harness` (Stage 1 RED). They drive the same
//!    duplex transport through the higher-level `common::Client` helper
//!    that the L3 / L4 / L5 suites also share, and assert at the **raw
//!    JSON-RPC envelope** layer (`result.protocolVersion == "2025-06-18"`)
//!    rather than at the rmcp-typed layer. Both layers stay in this file
//!    on purpose: the typed-layer tests guard the framework wiring, the
//!    envelope-layer tests guard the cross-suite helper. They are
//!    complementary, not redundant.
//!
//! All cases run in-process against `tokio::io::duplex(4096)` — no
//! subprocess, no real stdio.

use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage, ServerResult},
    transport::IntoTransport,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn raw(s: &str) -> ClientJsonRpcMessage {
    serde_json::from_str(s).expect("test message must parse as JSON-RPC")
}

fn init_request_2025_06_18() -> ClientJsonRpcMessage {
    raw(r#"{
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "grex-mcp-handshake-test", "version": "0.0.1" }
        }
    }"#)
}

fn build_server() -> GrexMcpServer {
    let state = ServerState::for_tests();
    GrexMcpServer::new(state)
}

/// 5.T1 — server completes the initialize handshake at protocol version
/// `2025-06-18`. The MCP framework negotiates down to the lower of client/server
/// version, so the response carries exactly the version we asked for.
#[tokio::test]
async fn initialize_handshake_accepts_2025_06_18() {
    let (server_tx, client_tx) = tokio::io::duplex(4096);
    let server = build_server();
    let _server_task = tokio::spawn(async move { server.run(server_tx).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_tx);

    use rmcp::transport::Transport;
    client.send(init_request_2025_06_18()).await.expect("send init");

    let response = tokio::time::timeout(Duration::from_secs(2), client.receive())
        .await
        .expect("init response within 2s")
        .expect("init response not None");

    match response {
        ServerJsonRpcMessage::Response(r) => match r.result {
            ServerResult::InitializeResult(init) => {
                assert_eq!(
                    init.protocol_version.to_string(),
                    "2025-06-18",
                    "server must echo client's pinned 2025-06-18, got {}",
                    init.protocol_version
                );
            }
            other => panic!("expected InitializeResult, got {other:?}"),
        },
        other => panic!("expected JSON-RPC response, got {other:?}"),
    }
}

/// 5.T2 — JSON-RPC 2.0 batch arrays must NOT be dispatched to handlers.
///
/// rmcp 1.5.0 silent-drops batches instead of returning -32600 per
/// JSON-RPC 2.0 §6. Acceptable under MCP 2025-06-18 which removes batch
/// support entirely (a conformant MCP client never sends one). See
/// `openspec/changes/feat-m7-1-mcp-server/spec.md` §"Known limitations"
/// for the upstream rmcp follow-up.
///
/// The safety contract this test enforces:
///   1. Server does NOT crash on receipt of a batch.
///   2. Server does NOT dispatch either embedded request (no double-action,
///      no echoed `InitializeResult` for id=1 or id=2).
#[tokio::test]
async fn batch_request_array_is_silently_dropped_no_dispatch() {
    let (mut client_io, server_io) = tokio::io::duplex(4096);

    let server = build_server();
    let server_task = tokio::spawn(async move { server.run(server_io).await });

    let batch = br#"[{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"0.0.1"}}},{"jsonrpc":"2.0","id":2,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"0.0.1"}}}]
"#;
    client_io.write_all(batch).await.expect("write batch");
    client_io.flush().await.expect("flush");

    // Within a 200 ms window, the server must NOT have written anything
    // to the transport — proves no method dispatch happened. (rmcp logs
    // a serde decode error to stderr and keeps the service loop alive.)
    let mut buf = vec![0u8; 4096];
    let read = tokio::time::timeout(Duration::from_millis(200), client_io.read(&mut buf)).await;
    let bytes_emitted = match read {
        Ok(Ok(n)) => n,
        _ => 0,
    };
    assert_eq!(
        bytes_emitted,
        0,
        "server dispatched a batch — wrote {bytes_emitted} bytes: {:?}",
        String::from_utf8_lossy(&buf[..bytes_emitted])
    );

    // Server must exit without panicking (rmcp 1.5.0 closes the input
    // stream after a codec-level decode error; `serve()` itself surfaces
    // `ServerInitializeError::ConnectionClosed` because the batch arrives
    // before initialize completes). Either Ok or Err is acceptable — the
    // safety invariant is "no panic, no dispatch", which is already proven
    // by the bytes-emitted assertion above.
    drop(client_io);
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task)
        .await
        .expect("server task joins within 2s")
        .expect("server task panic-free");
}

/// 5.T3 — closing the client end of the transport must drive the server's
/// `run` future to completion within 500 ms (no hang, returns `Ok`).
/// MCP has no explicit `shutdown` JSON-RPC method — per spec the transport
/// close IS the shutdown signal. We assert the rmcp framework respects that.
#[tokio::test]
async fn shutdown_returns_then_closes() {
    let (server_tx, client_tx) = tokio::io::duplex(4096);
    let server = build_server();
    let server_task = tokio::spawn(async move { server.run(server_tx).await });

    // Drive the handshake.
    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_tx);
    use rmcp::transport::Transport;
    client.send(init_request_2025_06_18()).await.expect("send init");
    let _ = tokio::time::timeout(Duration::from_secs(2), client.receive())
        .await
        .expect("init reply within 2s");

    // Close the transport: this is the MCP shutdown handshake (per
    // 2025-06-18 spec — no `shutdown` JSON-RPC method).
    drop(client);

    let outcome = tokio::time::timeout(Duration::from_millis(500), server_task)
        .await
        .expect("server task joins within 500 ms after transport close")
        .expect("server task panics-free");

    assert!(outcome.is_ok(), "server.run returned Err on clean transport close: {outcome:?}");
}

// ---------------------------------------------------------------------------
// feat-m7-2 Stage 1 — L2 duplex handshake (RED)
// ---------------------------------------------------------------------------
//
// Five cases enumerated in `openspec/changes/feat-m7-2-mcp-test-harness/
// spec.md` §"L2 — E2E handshake". Stage 1 lands them as failing tests
// that compile and panic at the `common::new_duplex_server` /
// `common::Client::*` boundary (intentional `unimplemented!()`). Stage 2
// closes the helpers and these tests flip GREEN with no body churn.
//
// Why duplicate `protocol_version_echoed` / `initialize_handshake_accepts_2025_06_18`?
// The m7-1 case asserts on the rmcp-typed `InitializeResult.protocol_version`
// (framework boundary). The m7-2 case asserts on `result.protocolVersion`
// in the raw JSON envelope returned by `common::Client` (cross-suite
// helper boundary). Different layers; both worth guarding.

#[path = "common/mod.rs"]
mod common;

/// L2.1 — full happy path: `initialize` → `notifications/initialized` →
/// `tools/list` → `shutdown`. Expect `tools/list.tools.len() >=
/// VERBS_EXPOSED.len()` and a clean shutdown.
#[tokio::test]
async fn handshake_ok() {
    let fixture = common::TestFixture::new();
    let mut client = common::new_duplex_server(&fixture);

    let _init = client.initialize().await;
    client
        .notify("initialized", serde_json::json!({}))
        .await;
    let tools = client
        .call("tools/list", serde_json::json!({}))
        .await;

    let len = tools["result"]["tools"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    assert!(
        len >= grex_mcp::VERBS_11_EXPOSED_AS_TOOLS.len(),
        "tools/list must expose at least {} tools, got {}",
        grex_mcp::VERBS_11_EXPOSED_AS_TOOLS.len(),
        len
    );

    client.shutdown().await;
}

/// L2.2 — sending `tools/list` before `initialize` must yield a
/// JSON-RPC error with code `-32002` and `data.kind == "init_state"`,
/// per `.omne/cfg/mcp.md` §"Error codes".
#[tokio::test]
async fn request_before_init_rejected() {
    let fixture = common::TestFixture::new();
    let mut client = common::new_duplex_server(&fixture);

    let resp = client.call("tools/list", serde_json::json!({})).await;

    let err = resp
        .get("error")
        .expect("expected JSON-RPC error envelope, got result");
    assert_eq!(
        err.get("code").and_then(|v| v.as_i64()),
        Some(-32002),
        "expected code -32002, got {err:?}"
    );
    assert_eq!(
        err.pointer("/data/kind").and_then(|v| v.as_str()),
        Some("init_state"),
        "expected data.kind == \"init_state\", got {err:?}"
    );
}

/// L2.3 — a second `initialize` after a successful one must yield
/// `-32002` with `data.kind == "init_state"`.
#[tokio::test]
async fn double_init_rejected() {
    let fixture = common::TestFixture::new();
    let mut client = common::new_duplex_server(&fixture);

    let _ok = client.initialize().await;
    let resp = client
        .call(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "double-init", "version": "0.0.1" }
            }),
        )
        .await;

    let err = resp
        .get("error")
        .expect("second initialize must return error envelope, got result");
    assert_eq!(
        err.get("code").and_then(|v| v.as_i64()),
        Some(-32002),
        "expected code -32002 on double init, got {err:?}"
    );
    assert_eq!(
        err.pointer("/data/kind").and_then(|v| v.as_str()),
        Some("init_state"),
        "expected data.kind == \"init_state\" on double init, got {err:?}"
    );
}

/// L2.4 — `shutdown` (transport close, in MCP terms) must drain any
/// in-flight tool call rather than dropping the response. The fixture
/// spawns a long-running `sleep`-style sentinel via `notify` then
/// closes the transport; the in-flight handler must complete and the
/// drop path must stay panic-free.
///
/// The sentinel hook lands in Stage 2 alongside the helper impl. Stage
/// 1 just pins the test signature so the count is stable.
#[tokio::test]
async fn graceful_shutdown_drains() {
    let fixture = common::TestFixture::new();
    let mut client = common::new_duplex_server(&fixture);
    let _ = client.initialize().await;
    client
        .notify("initialized", serde_json::json!({}))
        .await;

    // Stage 2: launch a long-running tool here, then `client.shutdown()`
    // and assert the response arrives before the transport closes.
    // Stage 1 just panics inside `initialize` via the stub, which is
    // sufficient to count this case as RED.
    client.shutdown().await;
}

/// L2.5 — `initialize` reply must echo the pinned protocol version
/// `2025-06-18` in the raw envelope (`result.protocolVersion`). m7-1
/// already covers this at the rmcp-typed layer; this case guards the
/// `common::Client` helper used by L3 / L4 / L5.
#[tokio::test]
async fn protocol_version_echoed() {
    let fixture = common::TestFixture::new();
    let mut client = common::new_duplex_server(&fixture);

    let init = client.initialize().await;

    let pv = init
        .pointer("/result/protocolVersion")
        .or_else(|| init.get("protocolVersion"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    assert_eq!(
        pv, "2025-06-18",
        "expected protocolVersion == 2025-06-18, got {pv:?} (full init: {init:?})"
    );
}
