//! `import` tool — import packs from a legacy `REPOS.json` meta-repo.
//!
//! Wires through to [`grex_core::import::import_from_repos_json`]. Mirrors
//! the CLI `grex import` surface: a required `from_repos_json` path and
//! a `dry_run` flag that short-circuits before any manifest write.
//!
//! # Workspace confinement (v1 safety invariant)
//!
//! The MCP surface intentionally does NOT accept a `workspace` override
//! (unlike the CLI, which reads `current_dir()`). Both inputs — the
//! source `REPOS.json` path and the target manifest — are resolved
//! relative to `state.workspace` captured at `grex serve` start, then
//! canonicalised and checked with `starts_with(state.workspace)`. An
//! absolute path that escapes the workspace is rejected with a
//! `packop_error` before any I/O reaches `grex_core::import`.
//!
//! This closes the path-traversal surface an untrusted agent could
//! otherwise abuse to read / write outside the server's sandbox.
//!
//! # Return value
//!
//! Returns a structured [`ImportPlan`]
//! as JSON content in a successful [`CallToolResult`]. Missing
//! `from_repos_json`, a workspace-escape attempt, or a core-level
//! [`ImportError`](grex_core::import::ImportError) all surface as
//! [`crate::error::packop_error`] so agents can parse the failure
//! uniformly.

use crate::error::packop_error;
use grex_core::import::{self, ImportOpts, ImportPlan, SkipReason};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use std::path::{Path, PathBuf};

/// Args for `import`. Mirrors CLI `ImportArgs` minus `workspace`; see
/// module doc-comment for the path-traversal rationale.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ImportParams {
    /// Path to a legacy REPOS.json file. Required at the MCP edge — the
    /// CLI's "print usage and exit" branch makes no sense for agents.
    /// Relative paths resolve against the server's workspace; absolute
    /// paths must canonicalise inside it or the call is rejected.
    #[serde(default)]
    pub from_repos_json: Option<PathBuf>,
    /// Plan actions without touching the manifest.
    #[serde(default)]
    pub dry_run: bool,
}

pub(crate) async fn handle(
    state: &crate::ServerState,
    Parameters(p): Parameters<ImportParams>,
) -> Result<CallToolResult, McpError> {
    let Some(from_raw) = p.from_repos_json else {
        return Ok(packop_error("`fromReposJson` is required"));
    };

    // Canonicalise the workspace once; any subsequent path check is
    // `starts_with(ws_canon)`.
    let ws_canon = match std::fs::canonicalize(&*state.workspace) {
        Ok(p) => p,
        Err(e) => {
            return Ok(packop_error(&format!(
                "workspace `{}` could not be canonicalised: {e}",
                state.workspace.display()
            )));
        }
    };

    let from_resolved = match resolve_in_workspace(&from_raw, &ws_canon) {
        Ok(p) => p,
        Err(msg) => return Ok(packop_error(&msg)),
    };

    // Target manifest always lives at `<workspace>/grex.jsonl`.
    let manifest_path = ws_canon.join("grex.jsonl");

    let opts = ImportOpts { dry_run: p.dry_run };

    // `import_from_repos_json` is synchronous filesystem I/O. Push onto a
    // blocking thread so the rmcp reactor stays responsive.
    let from_c = from_resolved.clone();
    let manifest_c = manifest_path.clone();
    let joined = tokio::task::spawn_blocking(move || {
        import::import_from_repos_json(&from_c, &manifest_c, opts)
    })
    .await;

    match joined {
        Ok(Ok(plan)) => Ok(success_envelope(&plan, p.dry_run)),
        Ok(Err(e)) => Ok(packop_error(&format!("{e}"))),
        Err(e) => Ok(packop_error(&format!("internal: blocking task failed: {e}"))),
    }
}

/// Resolve `input` against the canonicalised workspace root and assert
/// the resulting canonical path starts with `ws_canon`. Relative inputs
/// are joined with the workspace first; absolute inputs pass through.
/// The canonicalised result is what gets returned so the core-level
/// `import` sees the fully-resolved path.
///
/// Returns `Err(msg)` (suitable for `packop_error`) when:
/// - canonicalise fails (missing / unreadable file),
/// - the canonical path escapes the workspace.
fn resolve_in_workspace(input: &Path, ws_canon: &Path) -> Result<PathBuf, String> {
    let candidate = if input.is_absolute() { input.to_path_buf() } else { ws_canon.join(input) };
    let canon = std::fs::canonicalize(&candidate)
        .map_err(|e| format!("could not canonicalise `{}`: {e}", candidate.display()))?;
    if !canon.starts_with(ws_canon) {
        return Err(format!(
            "path `{}` escapes workspace `{}`",
            canon.display(),
            ws_canon.display()
        ));
    }
    Ok(canon)
}

fn success_envelope(plan: &ImportPlan, dry_run: bool) -> CallToolResult {
    let body = render_plan_json(plan, dry_run);
    CallToolResult::success(vec![Content::text(body.to_string())])
}

/// Canonical `import` JSON shape. Shared with the CLI `--json` surface;
/// the exact same fields must appear in `docs/src/cli-json.md`.
///
/// Shape: `{dry_run, imported[], skipped[], failed[]}`. No `summary`
/// wrapper — callers derive counts from the three arrays directly.
pub(crate) fn render_plan_json(plan: &ImportPlan, dry_run: bool) -> serde_json::Value {
    let imported: Vec<_> = plan
        .imported
        .iter()
        .map(|e| {
            json!({
                "path": e.path,
                "url": e.url,
                "kind": e.kind.as_str(),
                "would_dispatch": e.would_dispatch,
            })
        })
        .collect();
    let skipped: Vec<_> = plan
        .skipped
        .iter()
        .map(|s| {
            json!({
                "path": s.path,
                "reason": match s.reason {
                    SkipReason::PathCollision => "path_collision",
                    SkipReason::DuplicateInInput => "duplicate_in_input",
                },
            })
        })
        .collect();
    let failed: Vec<_> =
        plan.failed.iter().map(|f| json!({ "path": f.path, "error": f.error })).collect();
    json!({
        "dry_run": dry_run,
        "imported": imported,
        "skipped": skipped,
        "failed": failed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;
    use serde_json::Value;
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
    fn import_params_schema_resolves() {
        let _ = schema_for_type::<ImportParams>();
    }

    /// `workspace` was dropped at the MCP edge. `deny_unknown_fields`
    /// rejects it if sent.
    #[test]
    fn import_params_rejects_workspace() {
        let bad: Result<ImportParams, _> = serde_json::from_value(json!({ "workspace": "/tmp" }));
        assert!(bad.is_err(), "`workspace` must be rejected by the MCP schema");
    }

    /// Missing required `fromReposJson` maps to a `packop_error` envelope.
    #[tokio::test]
    async fn import_missing_from_repos_json_returns_packop_error() {
        let dir = tempdir().unwrap();
        let s = state_rooted_at(dir.path());
        let r = handle(&s, Parameters(ImportParams::default())).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["data"]["kind"], json!("pack_op"));
    }

    /// Happy path: dry-run against a workspace-relative `REPOS.json`
    /// returns the canonical plan envelope.
    #[tokio::test]
    async fn import_relative_path_happy_path() {
        let dir = tempdir().unwrap();
        let repos = dir.path().join("REPOS.json");
        std::fs::write(
            &repos,
            r#"[
                {"url": "https://github.com/a/a.git", "path": "a"},
                {"url": "", "path": "b"}
            ]"#,
        )
        .unwrap();
        let s = state_rooted_at(dir.path());
        let p = ImportParams { from_repos_json: Some(PathBuf::from("REPOS.json")), dry_run: true };
        let r = handle(&s, Parameters(p)).await.unwrap();
        assert_ne!(r.is_error, Some(true), "expected success envelope");
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["dry_run"], json!(true));
        assert_eq!(v["imported"].as_array().unwrap().len(), 2);
        assert!(v.get("summary").is_none(), "summary wrapper must be gone");
    }

    /// Workspace-escape: an absolute path outside the workspace is
    /// rejected before any I/O.
    #[tokio::test]
    async fn import_rejects_escape_via_absolute_path() {
        let outside = tempdir().unwrap();
        let repos = outside.path().join("REPOS.json");
        std::fs::write(&repos, "[]").unwrap();

        let ws = tempdir().unwrap();
        let s = state_rooted_at(ws.path());
        let p = ImportParams { from_repos_json: Some(repos), dry_run: true };
        let r = handle(&s, Parameters(p)).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["data"]["kind"], json!("pack_op"));
        assert!(
            v["message"].as_str().unwrap().contains("escapes workspace"),
            "expected workspace-escape diagnostic, got: {}",
            v["message"]
        );
    }

    /// Workspace-escape: `../` traversal on a relative path is also
    /// caught by canonicalisation.
    #[tokio::test]
    async fn import_rejects_escape_via_dotdot_traversal() {
        let outer = tempdir().unwrap();
        let ws = outer.path().join("ws");
        std::fs::create_dir_all(&ws).unwrap();
        let sibling = outer.path().join("sibling.json");
        std::fs::write(&sibling, "[]").unwrap();

        let s = state_rooted_at(&ws);
        // `../sibling.json` joined with ws → outside ws after canonicalise.
        let p =
            ImportParams { from_repos_json: Some(PathBuf::from("../sibling.json")), dry_run: true };
        let r = handle(&s, Parameters(p)).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(v["message"].as_str().unwrap().contains("escapes workspace"));
    }

    /// Failure path: malformed `REPOS.json` bubbles up as a `packop_error`.
    #[tokio::test]
    async fn import_malformed_input_returns_packop_error() {
        let dir = tempdir().unwrap();
        let repos = dir.path().join("REPOS.json");
        std::fs::write(&repos, "not json at all").unwrap();
        let s = state_rooted_at(dir.path());
        let p = ImportParams { from_repos_json: Some(PathBuf::from("REPOS.json")), dry_run: true };
        let r = handle(&s, Parameters(p)).await.unwrap();
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["data"]["kind"], json!("pack_op"));
    }
}
