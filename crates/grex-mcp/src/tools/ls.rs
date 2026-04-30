//! `ls` tool — read-only tree listing of the pack graph.
//!
//! Wires through to [`grex_core::build_ls_tree`]. Mirrors the CLI
//! `grex ls --json` surface: walks the workspace from the server's
//! pinned root, surfaces declared children that are on-disk packs,
//! synthesises leaf scripted entries for plain-git children (no
//! `.grex/pack.yaml` but `.git/` present), and returns the same
//! `{workspace, tree[]}` envelope CLI consumers see.
//!
//! `--workspace` is intentionally NOT accepted on the MCP surface:
//! path-traversal / workspace escape is a real hazard when untrusted
//! agents drive the server, so the tool always uses `state.workspace`
//! captured at `grex serve` start. Matches the v1 safety model in
//! `.omne/cfg/mcp.md` (see `doctor.rs` for the same rationale).
//!
//! # Return value
//!
//! Returns a successful [`CallToolResult`] carrying the
//! [`grex_core::LsTree`] payload as JSON. Hard manifest errors (root
//! `pack.yaml` missing / unparsable) surface as
//! [`crate::error::packop_error`] so agents can parse the failure
//! uniformly.

use crate::error::packop_error;
use grex_core::build_ls_tree;
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;

/// Args for `ls`. The MCP surface intentionally accepts no parameters
/// — `pack_root` is pinned to `state.workspace` at server start so
/// agents cannot traverse out of the server's sandbox. See module
/// doc-comment for the rationale.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields)]
pub struct LsParams {}

pub(crate) async fn handle(
    state: &crate::ServerState,
    Parameters(_p): Parameters<LsParams>,
) -> Result<CallToolResult, McpError> {
    // Pin the walk to the server's startup workspace — no overrides,
    // no path-traversal surface. The CLI's "directory or YAML file"
    // dispatch is unnecessary here: `state.workspace` is always a
    // workspace root, never a YAML file.
    let workspace = (*state.workspace).clone();

    // `build_ls_tree` is synchronous filesystem I/O. Push onto a
    // blocking thread so the rmcp reactor stays responsive.
    let joined = tokio::task::spawn_blocking(move || build_ls_tree(&workspace)).await;

    match joined {
        Ok(Ok(tree)) => Ok(success_envelope(&tree)),
        Ok(Err(detail)) => Ok(packop_error(&detail)),
        Err(e) => Ok(packop_error(&format!("internal: blocking task failed: {e}"))),
    }
}

fn success_envelope(tree: &grex_core::LsTree) -> CallToolResult {
    // `LsTree` is `Serialize`; use `serde_json::to_string` so the
    // emitted JSON exactly matches the canonical shape both the CLI
    // `--json` mode and `man/reference/cli-json.md` document.
    let body = serde_json::to_string(tree)
        .unwrap_or_else(|e| format!("{{\"error\":\"serialise LsTree: {e}\"}}"));
    CallToolResult::success(vec![Content::text(body)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;
    use serde_json::Value;
    use std::fs;
    use tempfile::tempdir;

    fn state_rooted_at(root: &std::path::Path) -> crate::ServerState {
        crate::ServerState::new(
            grex_core::Scheduler::new(1),
            grex_core::Registry::default(),
            root.join(".grex").join("events.jsonl"),
            root.to_path_buf(),
        )
    }

    #[test]
    fn ls_params_schema_resolves() {
        let _ = schema_for_type::<LsParams>();
    }

    /// Workspace overrides are rejected at the schema layer — the
    /// MCP surface pins to `state.workspace`.
    #[test]
    fn ls_params_rejects_workspace_override() {
        let bad: Result<LsParams, _> =
            serde_json::from_value(serde_json::json!({ "workspace": "/tmp" }));
        assert!(bad.is_err(), "`workspace` must be rejected by the MCP schema");
    }

    /// Happy path: a meta-pack with one synthesised plain-git child
    /// surfaces in the tree envelope with `synthetic: true` and
    /// `type: scripted`.
    #[tokio::test]
    async fn ls_emits_workspace_envelope_with_synthetic_child() {
        let dir = tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join(".grex")).unwrap();
        fs::write(
            root.join(".grex/pack.yaml"),
            "schema_version: \"1\"\nname: rootp\ntype: meta\nchildren:\n  - url: file:///dev/null\n    path: alpha\n",
        )
        .unwrap();
        fs::create_dir_all(root.join("alpha/.git")).unwrap();

        let s = state_rooted_at(root);
        let r = handle(&s, Parameters(LsParams::default())).await.unwrap();
        assert_ne!(r.is_error, Some(true), "expected success envelope");
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v["workspace"].is_string(), "envelope must carry `workspace`");
        let nodes = v["tree"].as_array().expect("tree array");
        assert_eq!(nodes.len(), 1);
        let root_node = &nodes[0];
        assert_eq!(root_node["name"], Value::from("rootp"));
        assert_eq!(root_node["type"], Value::from("meta"));
        assert_eq!(root_node["synthetic"], Value::from(false));
        let children = root_node["children"].as_array().expect("children array");
        assert_eq!(children.len(), 1);
        let child = &children[0];
        assert_eq!(child["synthetic"], Value::from(true));
        assert_eq!(child["type"], Value::from("scripted"));
    }

    /// Failure-shape path: an empty workspace (no manifest) maps to
    /// a `packop_error` envelope so agents can branch on `data.kind`.
    #[tokio::test]
    async fn ls_missing_manifest_returns_packop_error() {
        let dir = tempdir().unwrap();
        let s = state_rooted_at(dir.path());
        let r = handle(&s, Parameters(LsParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["data"]["kind"], Value::from("pack_op"));
    }
}
