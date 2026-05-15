//! `update` tool — sync + reinstall on lock change.

use crate::error::packop_error;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `update`. Mirrors CLI `UpdateArgs`.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct UpdateParams {
    /// Optional pack path; if omitted, update all.
    #[serde(default)]
    pub pack: Option<String>,
}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<UpdateParams>,
) -> Result<CallToolResult, McpError> {
    Ok(packop_error("verb `update` is wired CLI-side as of v1.4.0; MCP handler will follow in v1.5.0"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn update_params_schema_resolves() {
        let _ = schema_for_type::<UpdateParams>();
    }

    #[tokio::test]
    async fn update_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(UpdateParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
