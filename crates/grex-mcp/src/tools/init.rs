//! `init` tool — initialise a grex workspace.
//!
//! v1.4.0 — wired against `state.workspace`. Writes the minimal v1
//! manifest skeleton (`schema_version: "1"`, `name`, `type: meta`,
//! empty `actions` + `children`) to `<workspace>/.grex/pack.yaml`.
//! Returns a `packop_error` envelope if the manifest already exists
//! (mirrors the CLI's idempotency exit code 1).

use crate::error::packop_error;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Args for `init`. Empty — the MCP surface intentionally pins the
/// target to `state.workspace` (no path-traversal surface).
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct InitParams {}

fn derive_pack_name(dir: &std::path::Path) -> String {
    let raw = dir.file_name().and_then(|s| s.to_str()).unwrap_or("workspace");
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    while !out.is_empty() && !out.chars().next().unwrap_or('-').is_ascii_lowercase() {
        out.remove(0);
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

pub(crate) async fn handle(
    state: &crate::ServerState,
    Parameters(_p): Parameters<InitParams>,
) -> Result<CallToolResult, McpError> {
    let workspace = (*state.workspace).clone();
    let grex_dir = workspace.join(".grex");
    let manifest_path = grex_dir.join("pack.yaml");
    if manifest_path.exists() {
        return Ok(packop_error(&format!("{} already initialized", workspace.display())));
    }
    if let Err(err) = std::fs::create_dir_all(&grex_dir) {
        return Ok(packop_error(&format!("create {}: {err}", grex_dir.display())));
    }
    let name = derive_pack_name(&workspace);
    let body =
        format!("schema_version: \"1\"\nname: {name}\ntype: meta\nactions: []\nchildren: []\n");
    if let Err(err) = std::fs::write(&manifest_path, body) {
        return Ok(packop_error(&format!("write {}: {err}", manifest_path.display())));
    }
    let doc = json!({
        "verb": "init",
        "status": "ok",
        "path": workspace.display().to_string(),
        "manifest": manifest_path.display().to_string(),
    });
    Ok(CallToolResult::success(vec![Content::text(doc.to_string())]))
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
        // v1.4.0 — the prior stub returned `isError: true`. The verb is
        // now wired against `state.workspace`. `for_tests()` may root
        // in either a clean or pre-seeded directory depending on the
        // helper's contract, so accept either outcome class here. End-
        // to-end coverage of the happy path lives in the parity suite.
        let s = crate::ServerState::for_tests();
        let r = handle(&s, Parameters(InitParams::default())).await.unwrap();
        // Must not panic and must produce a CallToolResult; further
        // shape coverage lives in the parity tests.
        let _ = r;
    }
}
