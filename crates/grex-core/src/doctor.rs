//! `grex doctor` — read-only health checks for a grex workspace.
//!
//! STAGE 1 (red): types + tests only — check bodies are `todo!()`.
//! Stage 2 lands the real implementation.

use std::path::Path;

use crate::fs::gitignore::GitignoreError;
use crate::manifest::{Event, ManifestError, PackState};

/// Which check produced this finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckKind {
    ManifestSchema,
    GitignoreSync,
    OnDiskDrift,
    ConfigLint,
}

impl CheckKind {
    pub fn label(self) -> &'static str {
        match self {
            CheckKind::ManifestSchema => "manifest-schema",
            CheckKind::GitignoreSync => "gitignore-sync",
            CheckKind::OnDiskDrift => "on-disk-drift",
            CheckKind::ConfigLint => "config-lint",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Ok,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: CheckKind,
    pub severity: Severity,
    pub pack: Option<String>,
    pub detail: String,
    pub auto_fixable: bool,
}

impl Finding {
    pub fn ok(check: CheckKind) -> Self {
        Self {
            check,
            severity: Severity::Ok,
            pack: None,
            detail: String::new(),
            auto_fixable: false,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct CheckResult {
    pub findings: Vec<Finding>,
}

impl CheckResult {
    pub fn single(finding: Finding) -> Self {
        Self { findings: vec![finding] }
    }

    pub fn worst(&self) -> Severity {
        self.findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DoctorReport {
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    pub fn worst(&self) -> Severity {
        self.findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok)
    }

    pub fn exit_code(&self) -> i32 {
        match self.worst() {
            Severity::Ok => 0,
            Severity::Warning => 1,
            Severity::Error => 2,
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DoctorOpts {
    pub fix: bool,
    pub lint_config: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    #[error("manifest read failure: {0}")]
    ManifestIo(#[source] ManifestError),
    #[error("gitignore fix failure: {0}")]
    GitignoreFix(#[source] GitignoreError),
}

pub fn run_doctor(_workspace: &Path, _opts: &DoctorOpts) -> Result<DoctorReport, DoctorError> {
    todo!("stage 2: implement run_doctor orchestrator")
}

pub fn check_manifest_schema(_manifest_path: &Path) -> (CheckResult, Option<Vec<Event>>) {
    todo!("stage 2: schema check")
}

pub fn check_gitignore_sync(
    _workspace: &Path,
    _packs: &std::collections::HashMap<String, PackState>,
) -> CheckResult {
    todo!("stage 2: gitignore sync check")
}

pub fn check_on_disk_drift(
    _workspace: &Path,
    _packs: &std::collections::HashMap<String, PackState>,
) -> CheckResult {
    todo!("stage 2: on-disk drift check")
}

pub fn check_config_lint(_workspace: &Path) -> CheckResult {
    todo!("stage 2: config lint check")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fs::gitignore::upsert_managed_block;
    use crate::manifest::{append_event, Event, SCHEMA_VERSION};
    use chrono::{TimeZone, Utc};
    use std::fs;
    use tempfile::tempdir;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 22, 10, 0, 0).unwrap()
    }

    fn seed_pack(workspace: &Path, id: &str) {
        let m = workspace.join("grex.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: id.into(),
                url: format!("https://example/{id}"),
                path: id.into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        fs::create_dir_all(workspace.join(id)).unwrap();
    }

    #[test]
    fn schema_clean_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        let (r, evs) = check_manifest_schema(&d.path().join("grex.jsonl"));
        assert_eq!(r.worst(), Severity::Ok);
        assert_eq!(evs.unwrap().len(), 1);
    }

    #[test]
    fn schema_corruption_is_error() {
        let d = tempdir().unwrap();
        let m = d.path().join("grex.jsonl");
        fs::write(&m, b"not-json\n").unwrap();
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: "x".into(),
                url: "u".into(),
                path: "x".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        let (r, evs) = check_manifest_schema(&m);
        assert_eq!(r.worst(), Severity::Error);
        assert!(evs.is_none());
    }

    #[test]
    fn gitignore_clean_block_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &[]).unwrap();
        let events = crate::manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = crate::manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Ok);
    }

    #[test]
    fn gitignore_drift_is_warning() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(
            &d.path().join("a").join(".gitignore"),
            "a",
            &["unexpected-line"],
        )
        .unwrap();
        let events = crate::manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = crate::manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Warning);
    }

    #[test]
    fn on_disk_missing_pack_is_error() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        fs::remove_dir_all(d.path().join("a")).unwrap();
        let events = crate::manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = crate::manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Error);
    }

    #[test]
    fn config_lint_bad_yaml_is_warning() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("openspec")).unwrap();
        fs::write(d.path().join("openspec").join("config.yaml"), "::: bad: : yaml : [").unwrap();
        let r = check_config_lint(d.path());
        assert_eq!(r.worst(), Severity::Warning);
    }

    #[test]
    fn exit_code_roll_up_ok_is_zero() {
        let mut r = DoctorReport::default();
        r.findings.push(Finding::ok(CheckKind::ManifestSchema));
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn exit_code_roll_up_error_is_two() {
        let mut r = DoctorReport::default();
        r.findings.push(Finding {
            check: CheckKind::OnDiskDrift,
            severity: Severity::Error,
            pack: None,
            detail: String::new(),
            auto_fixable: false,
        });
        assert_eq!(r.exit_code(), 2);
    }

    #[test]
    fn run_doctor_clean_workspace_exits_zero() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &[]).unwrap();
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn run_doctor_fix_does_not_touch_manifest_on_schema_error() {
        let d = tempdir().unwrap();
        let m = d.path().join("grex.jsonl");
        fs::write(&m, b"garbage-line\n").unwrap();
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: "x".into(),
                url: "u".into(),
                path: "x".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        let before_bytes = fs::read(&m).unwrap();
        let opts = DoctorOpts { fix: true, lint_config: false };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 2);
        let after_bytes = fs::read(&m).unwrap();
        assert_eq!(before_bytes, after_bytes);
    }
}
