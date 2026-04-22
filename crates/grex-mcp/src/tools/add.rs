//! `add` tool — register and clone a pack.

use crate::error::not_implemented_result;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `add`. Mirrors CLI `AddArgs`: required `url`, optional `path`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddParams {
    /// Git URL of the pack repo.
    pub url: String,
    /// Optional local path (defaults to repo name).
    #[serde(default)]
    pub path: Option<String>,
}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<AddParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("add"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn add_params_schema_resolves() {
        let _ = schema_for_type::<AddParams>();
    }

    #[tokio::test]
    async fn add_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let p = AddParams { url: "https://x/y.git".into(), path: None };
        let r = handle(&s, Parameters(p)).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
