//! `grex doctor` CLI verb — thin wrapper over `grex_core::doctor`.
//!
//! Renders the report as either a table (default) or a JSON document
//! (`--json`), then exits with the severity-roll-up code.

use crate::cli::args::{DoctorArgs, GlobalFlags};
use anyhow::Result;
use grex_core::doctor::{run_doctor, DoctorOpts, DoctorReport, Severity};
use tokio_util::sync::CancellationToken;

pub fn run(args: DoctorArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let workspace = std::env::current_dir()?;
    let opts = DoctorOpts { fix: args.fix, lint_config: args.lint_config };
    let report = run_doctor(&workspace, &opts)?;

    if global.json {
        println!("{}", render_json(&report));
    } else {
        print_table(&report);
    }

    std::process::exit(report.exit_code());
}

/// Render the report as a table. One row per finding.
fn print_table(report: &DoctorReport) {
    println!("{:<18} {:<8} DETAIL", "CHECK", "STATUS");
    for f in &report.findings {
        let status = match f.severity {
            Severity::Ok => "OK",
            Severity::Warning => "WARN",
            Severity::Error => "ERROR",
        };
        let detail = if f.detail.is_empty() { "-".to_string() } else { f.detail.clone() };
        let pack = f.pack.as_deref().unwrap_or("");
        let label = if pack.is_empty() {
            f.check.label().to_string()
        } else {
            format!("{}[{}]", f.check.label(), pack)
        };
        println!("{label:<18} {status:<8} {detail}");
    }
}

/// Canonical `doctor` JSON shape. Must remain byte-equal to the MCP
/// handler's output (`crates/grex-mcp/src/tools/doctor.rs::render_report_json`)
/// and match `docs/src/cli-json.md §doctor`. Any field rename or
/// addition MUST land in all three places in the same commit.
fn render_json(report: &DoctorReport) -> String {
    let findings: Vec<serde_json::Value> = report
        .findings
        .iter()
        .map(|f| {
            serde_json::json!({
                "check": f.check.label(),
                "severity": severity_label(f.severity),
                "pack": f.pack,
                "detail": f.detail,
                "auto_fixable": f.auto_fixable,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "exit_code": report.exit_code(),
        "worst_severity": severity_label(report.worst()),
        "findings": findings,
    });
    // Compact form so byte-comparison against the MCP surface (which
    // uses `Value::to_string`, also compact) is trivial.
    serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string())
}

fn severity_label(s: Severity) -> &'static str {
    match s {
        Severity::Ok => "ok",
        Severity::Warning => "warning",
        Severity::Error => "error",
    }
}
