//! `doctor` tool — manifest + on-disk drift checks.

use crate::error::not_implemented_result;
use rmcp::{ErrorData as McpError, handler::server::wrapper::Parameters, model::CallToolResult};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `doctor`. Empty.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct DoctorParams {}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<DoctorParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("doctor"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn doctor_params_schema_resolves() {
        let _ = schema_for_type::<DoctorParams>();
    }

    #[tokio::test]
    async fn doctor_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(DoctorParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
