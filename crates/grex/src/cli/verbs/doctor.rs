//! `grex doctor` CLI verb — thin wrapper over `grex_core::doctor`.
//!
//! Renders the report as either a table (default) or a JSON document
//! (`--json`), then exits with the severity-roll-up code.

use crate::cli::args::{DoctorArgs, GlobalFlags};
use anyhow::Result;
use grex_core::doctor::{
    run_doctor, scan_undeclared, DoctorOpts, DoctorReport, Severity, UndeclaredRepo,
};
use tokio_util::sync::CancellationToken;

pub fn run(args: DoctorArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let workspace = std::env::current_dir()?;
    let opts = DoctorOpts { fix: args.fix, lint_config: args.lint_config, shallow: args.shallow };
    let report = run_doctor(&workspace, &opts)?;

    if global.json {
        println!("{}", render_json(&report));
    } else {
        print_table(&report);
    }

    // v1.2.1 item 4 — opt-in full-filesystem scan for `.git/` dirs that
    // are NOT registered in the manifest tree. Report-only — does not
    // alter the doctor exit code semantics; `report.exit_code()` already
    // captures every check the scan complements.
    if args.scan_undeclared {
        let undeclared = scan_undeclared(&workspace, args.depth)?;
        if global.json {
            println!("{}", render_undeclared_json(&workspace, args.depth, &undeclared));
        } else {
            print_undeclared(&workspace, args.depth, &undeclared);
        }
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
/// and match `man/reference/cli-json.md §doctor`. Any field rename or
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
                "synthetic": f.synthetic,
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

/// Render the `--scan-undeclared` report block to stdout. Always
/// printed AFTER the standard doctor table so the existing output
/// remains the first thing operators see.
fn print_undeclared(workspace: &std::path::Path, depth: Option<usize>, found: &[UndeclaredRepo]) {
    let depth_str = depth.map_or_else(|| "unlimited".to_string(), |d| d.to_string());
    println!();
    println!("Scanning {} for undeclared git repos (depth: {})...", workspace.display(), depth_str,);
    if found.is_empty() {
        println!("No undeclared git repos found below {}.", workspace.display());
        return;
    }
    println!();
    println!(
        "Found {} undeclared git repo{}:",
        found.len(),
        if found.len() == 1 { "" } else { "s" },
    );
    for repo in found {
        let url = match &repo.inferred_url {
            Some(u) => format!("<{u}>"),
            None => "[unknown]  (no remote.origin.url)".to_string(),
        };
        // Use forward slashes for cross-platform consistency in output.
        let path = repo.path.to_string_lossy().replace('\\', "/");
        println!("  ./{path:<40} {url}");
    }
    println!();
    println!("To register: grex add <url> <path>");
}

/// JSON twin of [`print_undeclared`]. Emits a single-line object
/// matching the human format's structure so machine consumers can
/// parse it directly.
fn render_undeclared_json(
    workspace: &std::path::Path,
    depth: Option<usize>,
    found: &[UndeclaredRepo],
) -> String {
    let entries: Vec<serde_json::Value> = found
        .iter()
        .map(|r| {
            let path = r.path.to_string_lossy().replace('\\', "/");
            serde_json::json!({
                "path": path,
                "inferred_url": r.inferred_url,
            })
        })
        .collect();
    let doc = serde_json::json!({
        "scan_undeclared": {
            "workspace": workspace.display().to_string(),
            "depth": depth,
            "count": found.len(),
            "repos": entries,
        },
    });
    serde_json::to_string(&doc).unwrap_or_else(|_| "{}".to_string())
}
