//! Stage 5 handshake tests for `GrexMcpServer`.
//!
//! These exercise the rmcp framework wiring end-to-end against an in-process
//! `tokio::io::duplex` transport — proving the server speaks JSON-RPC 2.0
//! with protocol-version pinning, batch silent-drop (rmcp 1.5.0 limitation —
//! see spec §"Known limitations"), and a clean transport-close shutdown.
//! No real stdio is touched.
//!
//! Stage 5 tests #5.T1, #5.T2, #5.T3.

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
        bytes_emitted, 0,
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

    assert!(
        outcome.is_ok(),
        "server.run returned Err on clean transport close: {outcome:?}"
    );
}
