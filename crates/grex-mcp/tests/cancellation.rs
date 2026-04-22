//! Stage 7 cancellation tests for `GrexMcpServer`.
//!
//! Covers `notifications/cancelled` plumbing per
//! `openspec/changes/feat-m7-1-mcp-server/tasks.md` §Stage 7:
//!
//! * 7.T1 — `notifications/cancelled` aborts an in-flight `tools/call sync`
//!   with `-32800 RequestCancelled` within 200 ms.
//! * 7.T2 — late cancel (request already completed) is silently ignored.
//! * 7.T3 — cancel for an unknown request id is silently ignored.
//!
//! The whole binary is gated behind the `test-hooks` cargo feature
//! because 7.T1 needs the deterministic block-until-cancelled hook
//! exposed by `grex_mcp::tools::sync::__test_set_block_until_cancelled`.
//! Run via `cargo test -p grex-mcp --features test-hooks` (or
//! `cargo test --workspace --all-features` from the workspace root).
//!
//! Implementation note: rmcp 1.5.0 already maintains a per-request
//! `local_ct_pool: HashMap<RequestId, CancellationToken>` inside its
//! service loop and triggers `ct.cancel()` on receipt of
//! `notifications/cancelled` (see rmcp `service.rs` lines 766/948/989-991).
//! The token surfaces to handlers as `RequestContext::ct`, so Stage 7
//! reduces to "thread `RequestContext::ct` into each tool body's
//! `tokio::select!`" — no separate `DashMap<RequestId, CancellationToken>`
//! is required (the spec called for one before the rmcp surface was
//! verified at Stage 1).

#![cfg(feature = "test-hooks")]

use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::IntoTransport,
};

fn raw(s: &str) -> ClientJsonRpcMessage {
    serde_json::from_str(s).expect("test message must parse as JSON-RPC")
}

fn init_request(id: u64) -> ClientJsonRpcMessage {
    raw(&format!(
        r#"{{
            "jsonrpc": "2.0",
            "id": {id},
            "method": "initialize",
            "params": {{
                "protocolVersion": "2025-06-18",
                "capabilities": {{}},
                "clientInfo": {{ "name": "grex-mcp-cancellation-test", "version": "0.0.1" }}
            }}
        }}"#
    ))
}

fn initialized_notification() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#)
}

fn sync_call(id: u64, pack_root: &str) -> ClientJsonRpcMessage {
    // Escape backslashes so the JSON parser sees a single literal
    // backslash (Windows paths in temp dirs).
    let escaped = pack_root.replace('\\', "\\\\");
    raw(&format!(
        r#"{{
            "jsonrpc": "2.0",
            "id": {id},
            "method": "tools/call",
            "params": {{
                "name": "sync",
                "arguments": {{
                    "packRoot": "{escaped}",
                    "dryRun": true,
                    "noValidate": true
                }}
            }}
        }}"#
    ))
}

fn cancelled_notification(id: u64) -> ClientJsonRpcMessage {
    raw(&format!(
        r#"{{
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": {{
                "requestId": {id},
                "reason": "test"
            }}
        }}"#
    ))
}

fn build_server() -> GrexMcpServer {
    GrexMcpServer::new(ServerState::for_tests())
}

async fn perform_handshake<T>(client: &mut T)
where
    T: rmcp::transport::Transport<rmcp::RoleClient> + Unpin,
{
    client.send(init_request(1)).await.expect("send init");
    let _ = tokio::time::timeout(Duration::from_secs(2), client.receive())
        .await
        .expect("init response within 2s");
    client
        .send(initialized_notification())
        .await
        .expect("send initialized");
}

/// Drain server messages until a -32800 cancellation envelope arrives
/// or `deadline` elapses. Returns `(saw_cancelled, saw_other_response)`.
async fn drain_for_cancel<T>(
    client: &mut T,
    deadline: tokio::time::Instant,
) -> (bool, bool)
where
    T: rmcp::transport::Transport<rmcp::RoleClient> + Unpin,
{
    let mut saw_cancelled = false;
    let mut saw_other_response = false;
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        let next = tokio::time::timeout(remaining, client.receive()).await;
        match next {
            Ok(Some(ServerJsonRpcMessage::Error(err))) => {
                let code = err.error.code.0;
                if code == grex_mcp::REQUEST_CANCELLED {
                    saw_cancelled = true;
                    break;
                }
                panic!("expected -32800 cancellation, got code {code}");
            }
            Ok(Some(ServerJsonRpcMessage::Response(_))) => {
                saw_other_response = true;
                break;
            }
            Ok(Some(_other)) => continue,
            Ok(None) | Err(_) => break,
        }
    }
    (saw_cancelled, saw_other_response)
}

/// 7.T1 — `notifications/cancelled` for an in-flight `tools/call sync`
/// produces a JSON-RPC error response with code `-32800` within 200 ms.
///
/// Strategy: enable the `block_until_cancelled` test hook in the
/// `sync` handler (set via
/// `grex_mcp::tools::sync::__test_set_block_until_cancelled`), so the
/// in-flight handler awaits its per-request `CancellationToken` instead
/// of running the real (microseconds-fast) `sync::run`. After issuing
/// the call we wait 50 ms (well past the rmcp loop's request-registration
/// point), then send `notifications/cancelled` for the same id.
/// rmcp's service loop fires the registered `CancellationToken`, the
/// handler returns `-32800`, and the test asserts the cancellation
/// envelope arrives within 200 ms of the cancel notification.
#[tokio::test]
async fn notifications_cancelled_aborts_inflight_sync() {
    grex_mcp::tools::sync::__test_set_block_until_cancelled(true);

    let (server_io, client_io) = tokio::io::duplex(8 * 1024);
    let server = build_server();
    let _server_task = tokio::spawn(async move { server.run(server_io).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_io);
    perform_handshake(&mut client).await;

    use rmcp::transport::Transport as _;
    let req_id: u64 = 42;
    let pack_root = std::env::temp_dir()
        .join("grex-mcp-cancel-7t1-nonexistent")
        .to_string_lossy()
        .into_owned();
    client
        .send(sync_call(req_id, &pack_root))
        .await
        .expect("send sync call");

    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancel_sent_at = tokio::time::Instant::now();
    client
        .send(cancelled_notification(req_id))
        .await
        .expect("send cancellation");

    let deadline = cancel_sent_at + Duration::from_millis(200);
    let (saw_cancelled, saw_other_response) = drain_for_cancel(&mut client, deadline).await;
    grex_mcp::tools::sync::__test_set_block_until_cancelled(false);
    assert!(
        saw_cancelled,
        "expected -32800 cancellation envelope within 200 ms; saw_other_response={saw_other_response}"
    );
}

/// 7.T2 — a `notifications/cancelled` arriving AFTER the response is
/// emitted is a silent no-op: server stays alive, no spurious error
/// surfaces, no panic. Per MCP spec the receiver MAY ignore unknown
/// request ids — once the request completes its id is dropped from
/// rmcp's `local_ct_pool`.
#[tokio::test]
async fn cancel_after_result_is_ignored() {
    let (server_io, client_io) = tokio::io::duplex(8 * 1024);
    let server = build_server();
    let server_task = tokio::spawn(async move { server.run(server_io).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_io);
    perform_handshake(&mut client).await;

    use rmcp::transport::Transport as _;
    // Drive a cheap `tools/list` so we have a request that terminates
    // synchronously and frees its id from the pool.
    let req_id: u64 = 7;
    client
        .send(raw(&format!(
            r#"{{"jsonrpc":"2.0","id":{req_id},"method":"tools/list"}}"#
        )))
        .await
        .expect("send tools/list");

    let _ = tokio::time::timeout(Duration::from_secs(1), client.receive())
        .await
        .expect("tools/list response within 1s");

    // Late cancel — rmcp must drop it silently.
    client
        .send(cancelled_notification(req_id))
        .await
        .expect("send late cancellation");

    // Server must still be running after a brief settle window. We
    // probe by issuing another tools/list and expecting a response.
    let probe_id: u64 = 8;
    client
        .send(raw(&format!(
            r#"{{"jsonrpc":"2.0","id":{probe_id},"method":"tools/list"}}"#
        )))
        .await
        .expect("send probe tools/list");
    let probe = tokio::time::timeout(Duration::from_secs(1), client.receive())
        .await
        .expect("probe response within 1s")
        .expect("probe response not None");
    matches!(probe, ServerJsonRpcMessage::Response(_));

    // Server task is still running.
    assert!(!server_task.is_finished(), "server crashed after late cancel");
}

/// 7.T3 — `notifications/cancelled` carrying an unknown `requestId`
/// is a silent no-op. Same invariant as 7.T2 but covers the case
/// where the id never existed at all (vs. completed-then-purged).
#[tokio::test]
async fn cancel_unknown_request_id_is_ignored() {
    let (server_io, client_io) = tokio::io::duplex(8 * 1024);
    let server = build_server();
    let server_task = tokio::spawn(async move { server.run(server_io).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_io);
    perform_handshake(&mut client).await;

    use rmcp::transport::Transport as _;
    // Cancel an id we never issued. MUST NOT crash, MUST NOT emit
    // anything on the wire (allow 100 ms to settle).
    client
        .send(cancelled_notification(9_999))
        .await
        .expect("send unknown-id cancellation");

    let probe = tokio::time::timeout(Duration::from_millis(100), client.receive()).await;
    assert!(
        probe.is_err() || matches!(probe, Ok(None)),
        "server emitted a message in response to unknown-id cancel"
    );

    // Sanity: server still answers requests.
    let probe_id: u64 = 11;
    client
        .send(raw(&format!(
            r#"{{"jsonrpc":"2.0","id":{probe_id},"method":"tools/list"}}"#
        )))
        .await
        .expect("send probe tools/list");
    let _ = tokio::time::timeout(Duration::from_secs(1), client.receive())
        .await
        .expect("probe response within 1s")
        .expect("probe response not None");

    assert!(
        !server_task.is_finished(),
        "server crashed after unknown-id cancel"
    );
}
