//! `add` tool - register and clone a pack.

use crate::error::packop_error;
use grex_core::add::{add_pack, infer_path_from_url, AddOpts, AddRequest};
use grex_core::import::classify;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

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
    state: &crate::ServerState,
    Parameters(p): Parameters<AddParams>,
) -> Result<CallToolResult, McpError> {
    let path = p.path.unwrap_or_else(|| infer_path_from_url(&p.url));
    let pack_type = classify(&p.url).as_str().to_string();
    let manifest_path = (*state.manifest_path).clone();
    let request = AddRequest::new(p.url, path, pack_type);

    let joined =
        tokio::task::spawn_blocking(move || add_pack(&manifest_path, request, AddOpts::new(false)))
            .await;

    match joined {
        Ok(Ok(report)) => {
            let body = json!({
                "dry_run": report.dry_run,
                "id": report.id,
                "url": report.url,
                "path": report.path,
                "type": report.pack_type,
                "appended": report.appended,
            });
            Ok(CallToolResult::success(vec![Content::text(body.to_string())]))
        }
        Ok(Err(e)) => Ok(packop_error(&format!("{e}"))),
        Err(e) => Ok(packop_error(&format!("internal: blocking task failed: {e}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;
    use tempfile::tempdir;

    fn state_rooted_at(root: &std::path::Path) -> crate::ServerState {
        crate::ServerState::new(
            grex_core::Scheduler::new(1),
            grex_core::Registry::default(),
            root.join("grex.jsonl"),
            root.to_path_buf(),
        )
    }

    #[test]
    fn add_params_schema_resolves() {
        let _ = schema_for_type::<AddParams>();
    }

    #[tokio::test]
    async fn add_happy_path_writes_manifest() {
        let dir = tempdir().unwrap();
        let s = state_rooted_at(dir.path());
        let p = AddParams { url: "https://x/y.git".into(), path: None };
        let r = handle(&s, Parameters(p)).await.unwrap();
        assert_ne!(r.is_error, Some(true));
        assert!(dir.path().join("grex.jsonl").exists());
    }
}
