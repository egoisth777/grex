//! Stage 5 test #5.T6 — `tools/list` returns an empty array. Sanity check that
//! the framework wiring routes the request to our handler at all; will be
//! replaced in Stage 6 once the 11 tools land.

use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage, ServerResult},
    transport::IntoTransport,
};

fn raw(s: &str) -> ClientJsonRpcMessage {
    serde_json::from_str(s).unwrap()
}

fn init() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"0.0.1"}}}"#)
}

fn list_tools() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
}

fn initialized() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
}

#[tokio::test]
async fn tools_list_returns_empty_in_stage_5() {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = GrexMcpServer::new(ServerState::for_tests());
    let _server = tokio::spawn(async move { server.run(server_io).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_io);

    use rmcp::transport::Transport;
    client.send(init()).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), client.receive())
        .await
        .unwrap()
        .unwrap();
    client.send(initialized()).await.unwrap();
    client.send(list_tools()).await.unwrap();

    // Skip past any framework notifications (e.g. logging) until we see a
    // response to id=2.
    let response = loop {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.receive())
            .await
            .expect("response within 2s")
            .expect("response not None");
        if matches!(&msg, ServerJsonRpcMessage::Response(r) if matches!(r.result, ServerResult::ListToolsResult(_)))
        {
            break msg;
        }
    };

    let ServerJsonRpcMessage::Response(r) = response else {
        panic!("not a response")
    };
    let ServerResult::ListToolsResult(list) = r.result else {
        panic!("not ListToolsResult")
    };
    assert!(
        list.tools.is_empty(),
        "Stage 5 must advertise zero tools; got {} ({:?})",
        list.tools.len(),
        list.tools.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
}
