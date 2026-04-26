//! `doctor` tool — manifest + gitignore + on-disk drift checks.
//!
//! Wires through to [`grex_core::doctor::run_doctor`]. Mirrors the CLI
//! `grex doctor` surface EXCEPT `--fix`: MCP `doctor` is read-only
//! inspection only. `--fix` remains available on the CLI for interactive
//! operators who want single-command gitignore healing. Dropping `fix`
//! from the MCP surface keeps the advertised annotations
//! (`read_only_hint = true, destructive_hint = false`) honest — an agent
//! that needs healing can call the CLI or drive the manifest-reconcile
//! operations directly.
//!
//! `--workspace` is intentionally NOT accepted on the MCP surface either:
//! path traversal / workspace escape is a real hazard when untrusted
//! agents drive the server, so the tool always uses `state.workspace`
//! captured at `grex serve` start. This matches the v1 safety model in
//! `.omne/cfg/mcp.md`.
//!
//! # Return value
//!
//! Always returns a successful [`CallToolResult`] carrying the full
//! [`DoctorReport`] as JSON, REGARDLESS of severity. An agent decides
//! what to do with warnings/errors by inspecting the `exit_code` +
//! `findings` fields. The only `isError: true` paths are hard I/O errors
//! on the manifest, wrapped as [`grex_core::doctor::DoctorError`] via
//! [`crate::error::packop_error`].

use crate::error::packop_error;
use grex_core::doctor::{self, DoctorOpts, DoctorReport, Severity};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

/// Args for `doctor`. Mirrors the CLI `DoctorArgs` minus `--fix` and
/// minus `--workspace`; see module doc-comment for the safety rationale.
#[derive(Debug, Deserialize, JsonSchema, Default)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct DoctorParams {
    /// Run the opt-in config-lint check (`openspec/config.yaml` +
    /// `.omne/cfg/*.md`).
    #[serde(default)]
    pub lint_config: bool,
}

pub(crate) async fn handle(
    state: &crate::ServerState,
    Parameters(p): Parameters<DoctorParams>,
) -> Result<CallToolResult, McpError> {
    // Always use the server's startup workspace — no overrides, no path
    // traversal surface. See module doc-comment.
    let workspace = (*state.workspace).clone();
    // `fix: false` is pinned on the MCP surface. Heals are a CLI-only
    // affordance.
    let opts = DoctorOpts { fix: false, lint_config: p.lint_config };

    // `run_doctor` is synchronous fs I/O. Push onto a blocking thread.
    let ws_c = workspace.clone();
    let joined = tokio::task::spawn_blocking(move || doctor::run_doctor(&ws_c, &opts)).await;

    match joined {
        Ok(Ok(report)) => Ok(report_envelope(&report)),
        Ok(Err(e)) => Ok(packop_error(&format!("{e}"))),
        Err(e) => Ok(packop_error(&format!("internal: blocking task failed: {e}"))),
    }
}

/// Render the report as a success envelope. Findings with `Error`
/// severity do NOT flip the envelope to `isError` — the agent reads
/// the report, identical to how the CLI prints the table and exits
/// with a severity-derived code.
fn report_envelope(report: &DoctorReport) -> CallToolResult {
    CallToolResult::success(vec![Content::text(render_report_json(report).to_string())])
}

/// Canonical `doctor` JSON shape. Shared with the CLI `--json` surface;
/// the exact same fields must appear in `man/reference/cli-json.md`.
pub(crate) fn render_report_json(report: &DoctorReport) -> serde_json::Value {
    let findings: Vec<_> = report
        .findings
        .iter()
        .map(|f| {
            json!({
                "check": f.check.label(),
                "severity": severity_label(f.severity),
                "pack": f.pack,
                "detail": f.detail,
                "auto_fixable": f.auto_fixable,
            })
        })
        .collect();
    json!({
        "exit_code": report.exit_code(),
        "worst_severity": severity_label(report.worst()),
        "findings": findings,
    })
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Ok => "ok",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;
    use serde_json::Value;
    use tempfile::tempdir;

    #[test]
    fn doctor_params_schema_resolves() {
        let _ = schema_for_type::<DoctorParams>();
    }

    /// `fix` + `workspace` are NOT in the param schema (both dropped at
    /// the MCP edge). `deny_unknown_fields` rejects either if sent.
    #[test]
    fn doctor_params_rejects_fix_and_workspace() {
        let bad_fix: Result<DoctorParams, _> = serde_json::from_value(json!({ "fix": true }));
        assert!(bad_fix.is_err(), "`fix` must be rejected by the MCP schema");
        let bad_ws: Result<DoctorParams, _> =
            serde_json::from_value(json!({ "workspace": "/tmp" }));
        assert!(bad_ws.is_err(), "`workspace` must be rejected by the MCP schema");
    }

    /// Happy path: empty workspace → all-OK report. Uses the server's
    /// `ServerState::for_tests()` workspace (cwd) rather than a custom
    /// one — MCP `doctor` never honours a param-supplied workspace.
    #[tokio::test]
    async fn doctor_empty_workspace_returns_ok_report() {
        // Route through a tempdir-rooted ServerState so the result is
        // deterministic regardless of what's in cwd.
        let dir = tempdir().unwrap();
        let state = crate::ServerState::new(
            grex_core::Scheduler::new(1),
            grex_core::Registry::default(),
            dir.path().join("grex.jsonl"),
            dir.path().to_path_buf(),
        );
        let r = handle(&state, Parameters(DoctorParams::default())).await.unwrap();
        assert_ne!(r.is_error, Some(true), "expected success envelope");
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["exit_code"], json!(0));
        assert_eq!(v["worst_severity"], json!("ok"));
    }

    /// Failure-shape path: a corrupt manifest line surfaces as an
    /// `Error`-severity finding in the report (exit_code == 2).
    #[tokio::test]
    async fn doctor_corrupt_manifest_surfaces_error_severity() {
        let dir = tempdir().unwrap();
        // Line 1 garbage + a valid line 2 → M3's reader reports Corruption.
        let m = dir.path().join("grex.jsonl");
        std::fs::write(
            &m,
            "not-json\n{\"schema_version\":\"1\",\"kind\":\"add\",\"ts\":\"2026-04-22T10:00:00Z\",\"id\":\"x\",\"url\":\"u\",\"path\":\"x\",\"pack_type\":\"declarative\"}\n",
        )
        .unwrap();
        let state = crate::ServerState::new(
            grex_core::Scheduler::new(1),
            grex_core::Registry::default(),
            dir.path().join("grex.jsonl"),
            dir.path().to_path_buf(),
        );
        let r = handle(&state, Parameters(DoctorParams::default())).await.unwrap();
        // Envelope stays Ok — the report itself carries the severity.
        assert_ne!(r.is_error, Some(true));
        let text = r.content.first().unwrap().as_text().unwrap().text.clone();
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["exit_code"], json!(2));
        assert_eq!(v["worst_severity"], json!("error"));
        assert!(v["findings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["check"] == json!("manifest-schema") && f["severity"] == json!("error")));
    }
}
