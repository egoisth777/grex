//! Stage 6 test #6.T8 — agent-safety contract for `exec`.
//!
//! The CLI's `--shell` escape hatch is **never** exposed as an MCP tool
//! parameter. `tools/exec.rs::ExecParams` derives `Deserialize` with
//! `#[serde(deny_unknown_fields)]`, so any client request that includes
//! a `shell` field MUST fail with JSON-RPC `-32602 Invalid Params` at
//! the `Parameters<P>` extraction edge — never reaching the handler
//! body.
//!
//! This test is the wire-level proof. Sends a real `tools/call` with
//! `arguments.shell` set, then asserts the server replies with a
//! JSON-RPC error response of code `-32602`.

use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage},
    transport::IntoTransport,
};

fn raw(s: &str) -> ClientJsonRpcMessage {
    serde_json::from_str(s).expect("test message must parse as JSON-RPC")
}

fn init() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"agent-safety-test","version":"0.0.1"}}}"#)
}
fn initialized() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
}

/// Sends `tools/call exec` with a forbidden `shell` argument. The
/// server MUST reject at the JSON-RPC envelope layer (`-32602`), not
/// dispatch into the tool body.
fn exec_call_with_shell() -> ClientJsonRpcMessage {
    raw(
        r#"{
        "jsonrpc": "2.0",
        "id": 42,
        "method": "tools/call",
        "params": {
            "name": "exec",
            "arguments": {
                "cmd": ["echo", "hi"],
                "shell": "bash -c 'rm -rf /'"
            }
        }
    }"#,
    )
}

#[tokio::test]
async fn exec_tool_rejects_shell_param_with_minus_32602() {
    let (server_io, client_io) = tokio::io::duplex(8192);
    let server = GrexMcpServer::new(ServerState::for_tests());
    let _server = tokio::spawn(async move { server.run(server_io).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_io);

    use rmcp::transport::Transport;
    client.send(init()).await.expect("send init");
    let _ = tokio::time::timeout(Duration::from_secs(2), client.receive())
        .await
        .expect("init response within 2s");
    client.send(initialized()).await.expect("send initialized");
    client.send(exec_call_with_shell()).await.expect("send exec");

    // Wait for a JSON-RPC Error response to id=42.
    let err = loop {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.receive())
            .await
            .expect("response within 2s")
            .expect("response not None");
        match msg {
            ServerJsonRpcMessage::Error(e) => break e,
            other => {
                // Either an unrelated framework notification or — if
                // rmcp ever decides to wrap the deserialisation failure
                // in `CallToolResult { isError }` — we must catch that
                // shape too. Per the rmcp 1.5 source `Parameters<P>` ext
                // returns `Err(invalid_params)` which becomes a JSON-RPC
                // Error envelope, so we keep looping.
                let _ = other;
            }
        }
    };

    assert_eq!(
        err.error.code.0, -32602,
        "expected -32602 Invalid Params, got {} ({})",
        err.error.code.0, err.error.message
    );
    let msg = err.error.message.to_lowercase();
    assert!(
        msg.contains("shell") || msg.contains("unknown field") || msg.contains("deserialize"),
        "expected error message to mention the rejected `shell` field; got: {}",
        err.error.message
    );
}
