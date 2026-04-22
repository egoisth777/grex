//! Stage 5 test #5.T4 — every byte the server writes to its transport must
//! parse as JSON-RPC. Tracing emitted from inside a handler must NOT leak
//! onto the wire (the subscriber in `GrexMcpServer::run` pins the writer to
//! stderr).
//!
//! Stage 5 has no tool handlers yet, so we drive only the `initialize`
//! handshake and assert the framing. Stage 6 will tighten this with a real
//! handler-side `tracing::info!` once tools are wired.

use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState};
use rmcp::model::ServerJsonRpcMessage;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[tokio::test]
async fn stdout_has_no_tracing_leaks() {
    let (server_io, client_io) = tokio::io::duplex(8192);
    let server = GrexMcpServer::new(ServerState::for_tests());
    let _server = tokio::spawn(async move { server.run(server_io).await });

    // Emit tracing both before and after `run` installs its subscriber.
    // Either path must not pollute the duplex (subscriber writer is pinned
    // to `std::io::stderr`).
    tracing::info!("test-emitted tracing line — must not appear on transport");

    let (mut r, mut w) = tokio::io::split(client_io);

    let init = br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"0.0.1"}}}
"#;
    w.write_all(init).await.unwrap();
    w.flush().await.unwrap();

    let mut buf = vec![0u8; 8192];
    let n = tokio::time::timeout(Duration::from_secs(2), r.read(&mut buf))
        .await
        .expect("read within 2s")
        .expect("read ok");
    assert!(n > 0, "server wrote no bytes");
    let captured = &buf[..n];

    let mut parsed_any = false;
    for (i, line) in captured.split(|&b| b == b'\n').enumerate() {
        if line.is_empty() {
            continue;
        }
        let _: ServerJsonRpcMessage = serde_json::from_slice(line).unwrap_or_else(|e| {
            panic!(
                "stdout line {i} is not JSON-RPC — tracing leak suspected.\n\
                 line: {line_str:?}\nerror: {e}",
                line_str = String::from_utf8_lossy(line)
            )
        });
        parsed_any = true;
    }
    assert!(parsed_any, "no parseable JSON-RPC line received");
}
