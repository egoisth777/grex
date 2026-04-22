//! `exec` tool — execute a command across matching packs.
//!
//! Agent-safety contract (`.omne/cfg/mcp.md` §Tool catalog): `ExecParams`
//! has **NO** `shell` field. The CLI keeps `--shell` as an interactive
//! escape hatch; the MCP surface refuses it because a shell rendition
//! turns trusted-command execution into arbitrary-code execution. Re-
//! introduction is gated on a future per-session capability opt-in.

use crate::error::not_implemented_result;
use rmcp::{ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `exec`. Mirrors CLI `ExecArgs` MINUS `--shell`.
///
/// `deny_unknown_fields` makes any client that sends `{"shell": "..."}`
/// (or any other typo) fail with `-32602 Invalid Params` at the rmcp
/// `Parameters<P>` extraction edge — see test 6.T8.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecParams {
    /// Command and args to execute. First element is the binary; the
    /// rest are passed verbatim. No shell interpretation.
    pub cmd: Vec<String>,
}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<ExecParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("exec"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;
    use serde_json::Value;

    #[test]
    fn exec_params_schema_resolves() {
        let _ = schema_for_type::<ExecParams>();
    }

    /// 6.T4 — the published JSON-Schema for `exec` MUST NOT carry a
    /// `shell` property. Walks the generated schema's top-level
    /// `properties` map and asserts absence. Cheap regression guard
    /// against an accidental `pub shell: bool` field.
    #[test]
    fn exec_tool_schema_has_no_shell_field() {
        let schema = schema_for_type::<ExecParams>();
        let v: Value = serde_json::to_value(&*schema).expect("schema → json");
        let props = v.get("properties").and_then(|p| p.as_object());
        if let Some(p) = props {
            assert!(
                !p.contains_key("shell"),
                "exec MCP schema must not advertise `shell`; got {p:?}"
            );
        }
        // `cmd` MUST be present so the agent knows what to send.
        assert!(
            props.map(|p| p.contains_key("cmd")).unwrap_or(false),
            "exec schema missing required `cmd` field; schema = {v}"
        );
    }

    #[tokio::test]
    async fn exec_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(ExecParams { cmd: vec!["echo".into()] }))
            .await
            .unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
