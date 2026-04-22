//! `ls` tool — list registered packs.

use crate::error::not_implemented_result;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `ls`. Empty.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LsParams {}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<LsParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("ls"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn ls_params_schema_resolves() {
        let _ = schema_for_type::<LsParams>();
    }

    #[tokio::test]
    async fn ls_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(LsParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
