//! `import` tool — import packs from a `REPOS.json` meta-repo.

use crate::error::not_implemented_result;
use rmcp::{handler::server::wrapper::Parameters, model::CallToolResult, ErrorData as McpError};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `import`. Mirrors CLI `ImportArgs`: optional path.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct ImportParams {
    /// Path to a legacy REPOS.json file.
    #[serde(default, rename = "fromReposJson")]
    pub from_repos_json: Option<std::path::PathBuf>,
}

pub(crate) async fn handle(
    _state: &crate::ServerState,
    Parameters(_p): Parameters<ImportParams>,
) -> Result<CallToolResult, McpError> {
    Ok(not_implemented_result("import"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn import_params_schema_resolves() {
        let _ = schema_for_type::<ImportParams>();
    }

    #[tokio::test]
    async fn import_happy_path_stub() {
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(ImportParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
    }
}
