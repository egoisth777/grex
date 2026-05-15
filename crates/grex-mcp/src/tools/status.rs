//! `status` tool — drift + installed state.

use crate::error::packop_error;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `status`. Empty.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct StatusParams {}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<StatusParams>,
) -> Result<CallToolResult, McpError> {
    Ok(packop_error("verb `status` is wired CLI-side as of v1.4.0; MCP handler will follow in v1.5.0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn status_params_schema_resolves() {
        let _ = schema_for_type::<StatusParams>();
    }

    #[tokio::test]
    async fn status_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(StatusParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
