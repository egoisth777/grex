//! `init` tool — initialise a grex workspace.
//!
//! Stub at Stage 6: core has no `init` impl yet (planned for M7-4). The
//! handler advertises full schema + annotations so agents discover the
//! verb, but every call returns `CallToolResult { isError: true }` with
//! a "not yet implemented" notice. Body is a 2-line dispatch into
//! [`crate::error::not_implemented_result`] so the M7-4 swap is local.

use crate::error::not_implemented_result;
use rmcp::{ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `init`. Empty for now (matches CLI's `InitArgs`).
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct InitParams {}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<InitParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("init"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn init_params_schema_resolves() {
        // schema_for_type asserts the type satisfies the rmcp tool input
        // contract (root JSON-Schema is an object). Panics if the derive
        // ever drifts to a non-object schema.
        let _ = schema_for_type::<InitParams>();
    }

    #[tokio::test]
    async fn init_happy_path_returns_not_implemented_envelope() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(InitParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true), "stub must mark isError");
    }
}
