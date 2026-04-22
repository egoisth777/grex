//! `run` tool — execute a declared action across matching packs.

use crate::error::not_implemented_result;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `run`. Mirrors CLI `RunArgs`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RunParams {
    /// Action name to run.
    pub action: String,
}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<RunParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("run"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn run_params_schema_resolves() {
        let _ = schema_for_type::<RunParams>();
    }

    #[tokio::test]
    async fn run_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(RunParams { action: "build".into() })).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
