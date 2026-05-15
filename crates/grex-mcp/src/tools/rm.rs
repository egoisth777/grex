//! `rm` tool — unregister a pack (runs teardown unless skipped).

use crate::error::packop_error;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `rm`. Mirrors CLI `RmArgs`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RmParams {
    /// Local path of the pack to remove.
    pub path: String,
}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<RmParams>,
) -> Result<CallToolResult, McpError> {
    Ok(packop_error("verb `rm` is wired CLI-side as of v1.4.0; MCP handler will follow in v1.5.0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn rm_params_schema_resolves() {
        let _ = schema_for_type::<RmParams>();
    }

    #[tokio::test]
    async fn rm_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(RmParams { path: "p".into() })).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
