//! Stage 6 replacement for the Stage 5 sanity test.
//!
//! Stage 5's `tools_list_returns_empty_in_stage_5` asserted the registry
//! was empty pending Stage 6. Stage 6 lights up the 11-tool registry, so
//! this file flips the assertion: `tools/list` over the wire returns
//! exactly the spec-mandated 11 verbs with the correct annotation set.
//!
//! File name preserved (avoids a churny `git mv`); this file is now
//! misnamed and will be renamed in a Stage 8 cleanup commit.

use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState, VERBS_EXPOSED};
use rmcp::{
    model::{ClientJsonRpcMessage, ServerJsonRpcMessage, ServerResult},
    transport::IntoTransport,
};

fn raw(s: &str) -> ClientJsonRpcMessage {
    serde_json::from_str(s).unwrap()
}

fn init() -> ClientJsonRpcMessage {
    raw(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"x","version":"0.0.1"}}}"#,
    )
}

fn list_tools() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
}

fn initialized() -> ClientJsonRpcMessage {
    raw(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
}

#[tokio::test]
async fn tools_list_returns_eleven_with_annotations() {
    let list = drive_list_tools().await;
    assert_count_and_annotations(list);
}

async fn drive_list_tools() -> rmcp::model::ListToolsResult {
    let (server_io, client_io) = tokio::io::duplex(8192);
    let server = GrexMcpServer::new(ServerState::for_tests());
    let _server = tokio::spawn(async move { server.run(server_io).await });

    let mut client = IntoTransport::<rmcp::RoleClient, _, _>::into_transport(client_io);
    use rmcp::transport::Transport;
    client.send(init()).await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(2), client.receive()).await.unwrap().unwrap();
    client.send(initialized()).await.unwrap();
    client.send(list_tools()).await.unwrap();

    loop {
        let msg = tokio::time::timeout(Duration::from_secs(2), client.receive())
            .await
            .expect("response within 2s")
            .expect("response not None");
        if let ServerJsonRpcMessage::Response(r) = msg {
            if let ServerResult::ListToolsResult(list) = r.result {
                return list;
            }
        }
    }
}

fn assert_count_and_annotations(list: rmcp::model::ListToolsResult) {
    assert_eq!(
        list.tools.len(),
        VERBS_EXPOSED.len(),
        "tools/list must advertise exactly {} tools, got {} ({:?})",
        VERBS_EXPOSED.len(),
        list.tools.len(),
        list.tools.iter().map(|t| &t.name).collect::<Vec<_>>(),
    );
    for t in &list.tools {
        let a = t
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("tool `{}` over the wire missing annotations", t.name));
        assert!(a.read_only_hint.is_some(), "wire: `{}` lacks read_only_hint", t.name);
        assert!(a.destructive_hint.is_some(), "wire: `{}` lacks destructive_hint", t.name);
    }
}
