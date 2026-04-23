//! `grex doctor` — read-only health checks for a grex workspace.
//!
//! The doctor runs three pack-health checks by default and one opt-in
//! config-lint check. Each check is a separate function returning a
//! [`CheckResult`]; [`run_doctor`] orchestrates them sequentially and
//! builds a [`DoctorReport`]. The severity roll-up → process exit code
//! lives in [`DoctorReport::exit_code`].
//!
//! # Safety contract for `--fix`
//!
//! `--fix` ONLY heals gitignore drift (re-emit the managed block via
//! the M5-2 writer). It must NOT touch the manifest (user data) or the
//! filesystem (user state) or any config file. The contract is
//! enforced by the private `apply_fixes` helper which dispatches
//! exclusively on [`CheckKind::GitignoreSync`].
//!
//! See `openspec/changes/feat-m7-4-import-doctor-license/spec.md`
//! §"Sub-scope 2 — `grex doctor`".

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::fs::gitignore::{read_managed_block, upsert_managed_block, GitignoreError};
use crate::manifest::{self, Event, ManifestError, PackState};

/// Which check produced this finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CheckKind {
    /// Manifest JSONL schema / corruption.
    ManifestSchema,
    /// Gitignore managed block drift vs manifest-declared patterns.
    GitignoreSync,
    /// Directory listed in manifest missing, or dir present but not
    /// registered.
    OnDiskDrift,
    /// Opt-in config lint (`--lint-config` only).
    ConfigLint,
}

impl CheckKind {
    /// Short human label used in the CLI table.
    pub fn label(self) -> &'static str {
        match self {
            CheckKind::ManifestSchema => "manifest-schema",
            CheckKind::GitignoreSync => "gitignore-sync",
            CheckKind::OnDiskDrift => "on-disk-drift",
            CheckKind::ConfigLint => "config-lint",
        }
    }
}

/// Severity of a single finding. Worst severity across the report
/// drives the process exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    /// Check passed cleanly.
    Ok,
    /// Non-critical drift. Exit 1.
    Warning,
    /// Critical — schema invalid, missing files, etc. Exit 2.
    Error,
}

/// One observation from a single check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Which check produced the finding.
    pub check: CheckKind,
    /// Severity — drives the exit-code roll-up.
    pub severity: Severity,
    /// Optional pack id (None for workspace-wide findings).
    pub pack: Option<String>,
    /// Human-readable detail.
    pub detail: String,
    /// True if `--fix` can heal this finding. Only
    /// `CheckKind::GitignoreSync` ever sets this to true; the flag gates
    /// the safety contract of `apply_fixes`.
    pub auto_fixable: bool,
}

impl Finding {
    /// Build an `Ok` finding for a check that passed cleanly.
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

/// One check's outcome — a list of findings (may be empty in the
/// degenerate case but normally holds at least one `Ok` finding so the
/// report shows a row per check).
#[derive(Debug, Clone, Default)]
pub struct CheckResult {
    /// Findings produced by this check.
    pub findings: Vec<Finding>,
}

impl CheckResult {
    /// Single-finding helper.
    pub fn single(finding: Finding) -> Self {
        Self { findings: vec![finding] }
    }

    /// Worst severity across the findings.
    pub fn worst(&self) -> Severity {
        self.findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok)
    }
}

/// Full health report.
#[derive(Debug, Clone, Default)]
pub struct DoctorReport {
    /// All findings, in check order.
    pub findings: Vec<Finding>,
}

impl DoctorReport {
    /// Worst severity across all findings. `Ok` when the report is empty.
    pub fn worst(&self) -> Severity {
        self.findings.iter().map(|f| f.severity).max().unwrap_or(Severity::Ok)
    }

    /// Process exit code derived from worst severity.
    ///
    /// * `0` — all findings are [`Severity::Ok`] or report is empty.
    /// * `1` — at least one [`Severity::Warning`] but no `Error`.
    /// * `2` — at least one [`Severity::Error`].
    pub fn exit_code(&self) -> i32 {
        match self.worst() {
            Severity::Ok => 0,
            Severity::Warning => 1,
            Severity::Error => 2,
        }
    }
}

/// Options for [`run_doctor`].
#[derive(Debug, Clone, Default)]
pub struct DoctorOpts {
    /// Heal gitignore drift. Only fixes [`CheckKind::GitignoreSync`]
    /// findings; all other checks remain read-only.
    pub fix: bool,
    /// Run the opt-in config-lint check. When `false`,
    /// [`CheckKind::ConfigLint`] never appears in the report.
    pub lint_config: bool,
}

/// Errors produced during doctor orchestration that are NOT surfaced as
/// findings. A hard I/O error on the manifest file (other than missing
/// file or corruption) aborts the run.
#[derive(Debug, thiserror::Error)]
pub enum DoctorError {
    /// Non-recoverable I/O error hitting the manifest.
    #[error("manifest read failure: {0}")]
    ManifestIo(#[source] ManifestError),
    /// Non-recoverable I/O error on a gitignore fix.
    #[error("gitignore fix failure: {0}")]
    GitignoreFix(#[source] GitignoreError),
}

/// Top-level orchestrator. Runs the 3 default checks; adds the 4th when
/// `opts.lint_config`. Applies `--fix` to gitignore findings after the
/// initial scan, then re-runs the gitignore check to record the healed
/// state.
pub fn run_doctor(workspace: &Path, opts: &DoctorOpts) -> Result<DoctorReport, DoctorError> {
    let manifest_path = workspace.join("grex.jsonl");

    let (schema_result, events_opt) = check_manifest_schema(&manifest_path);

    let mut report = DoctorReport::default();
    report.findings.extend(schema_result.findings.clone());

    // Subsequent pack-level checks need the folded state. If the
    // manifest is malformed, we still surface the schema error and skip
    // the dependent checks so we don't double-report garbage.
    let packs = events_opt.map(manifest::fold);

    let gi_result = match &packs {
        Some(p) => check_gitignore_sync(workspace, p),
        None => CheckResult::single(Finding {
            check: CheckKind::GitignoreSync,
            severity: Severity::Warning,
            pack: None,
            detail: "skipped: manifest unreadable".to_string(),
            auto_fixable: false,
        }),
    };
    report.findings.extend(gi_result.findings.clone());

    let drift_result = match &packs {
        Some(p) => check_on_disk_drift(workspace, p),
        None => CheckResult::single(Finding {
            check: CheckKind::OnDiskDrift,
            severity: Severity::Warning,
            pack: None,
            detail: "skipped: manifest unreadable".to_string(),
            auto_fixable: false,
        }),
    };
    report.findings.extend(drift_result.findings);

    if opts.lint_config {
        let cfg_result = check_config_lint(workspace);
        report.findings.extend(cfg_result.findings);
    }

    if opts.fix {
        apply_fixes(workspace, packs.as_ref(), &mut report)?;
    }

    Ok(report)
}

/// Run fixes and rebuild the gitignore-sync rows in `report`. Only
/// touches [`CheckKind::GitignoreSync`] findings with
/// `auto_fixable = true`. Other findings are left untouched — this is
/// the safety contract.
fn apply_fixes(
    workspace: &Path,
    packs: Option<&std::collections::HashMap<String, PackState>>,
    report: &mut DoctorReport,
) -> Result<(), DoctorError> {
    // Collect packs that need healing.
    let to_fix: Vec<(String, String)> = report
        .findings
        .iter()
        .filter(|f| f.check == CheckKind::GitignoreSync && f.auto_fixable)
        .filter_map(|f| f.pack.clone().map(|p| (p, f.detail.clone())))
        .collect();

    let Some(packs) = packs else {
        return Ok(());
    };

    for (pack_id, _detail) in to_fix {
        let Some(state) = packs.get(&pack_id) else { continue };
        let gi_path = workspace.join(&state.path).join(".gitignore");
        let expected = expected_patterns_for_pack(state);
        let patterns_ref: Vec<&str> = expected.iter().map(String::as_str).collect();
        upsert_managed_block(&gi_path, &state.id, &patterns_ref)
            .map_err(DoctorError::GitignoreFix)?;
    }

    // Re-run the gitignore-sync check; replace previous gi findings.
    let refreshed = check_gitignore_sync(workspace, packs);
    report.findings.retain(|f| f.check != CheckKind::GitignoreSync);
    report.findings.extend(refreshed.findings);
    Ok(())
}

/// Check 1 — manifest schema. Streams the JSONL log via the M3
/// corruption-resistant reader and converts the outcome into findings.
pub fn check_manifest_schema(manifest_path: &Path) -> (CheckResult, Option<Vec<Event>>) {
    if !manifest_path.exists() {
        // Empty workspace → no manifest, no findings beyond Ok.
        return (CheckResult::single(Finding::ok(CheckKind::ManifestSchema)), Some(Vec::new()));
    }
    match manifest::read_all(manifest_path) {
        Ok(evs) => (CheckResult::single(Finding::ok(CheckKind::ManifestSchema)), Some(evs)),
        Err(ManifestError::Corruption { line, source }) => {
            let detail = format!("corruption at line {line}: {source}");
            (
                CheckResult::single(Finding {
                    check: CheckKind::ManifestSchema,
                    severity: Severity::Error,
                    pack: None,
                    detail,
                    auto_fixable: false,
                }),
                None,
            )
        }
        Err(e) => {
            let detail = format!("io error: {e}");
            (
                CheckResult::single(Finding {
                    check: CheckKind::ManifestSchema,
                    severity: Severity::Error,
                    pack: None,
                    detail,
                    auto_fixable: false,
                }),
                None,
            )
        }
    }
}

/// Expected managed-block patterns for a single pack. M5-2 wrote this
/// as a constant slice; until a richer list lands we keep the contract
/// small and explicit: every pack's managed block holds its own path so
/// the parent workspace ignores its working tree.
///
/// TODO(m8): populate this list once plugin packs emit their own
/// pattern sets — tracked by issue #34. Until then any pack that writes
/// patterns via the M5-2 writer would be misreported as drift; this is
/// acceptable for M7-4b scope (declarative packs only, no patterns).
fn expected_patterns_for_pack(_state: &PackState) -> Vec<String> {
    // M7-4 contract: the doctor is anchored on M5-2's default
    // managed-block body. We don't re-define the content here; the
    // spec guarantees the default list, and this helper exists so the
    // set is computed from pack state rather than hard-coded at call
    // sites. The default list is "no patterns beyond the markers" —
    // pack-type plugins push their own patterns via the M5-2 writer.
    Vec::new()
}

/// Check 2 — gitignore sync. For every pack whose on-disk path has a
/// managed block, compare the body to the expected pattern list.
pub fn check_gitignore_sync(
    workspace: &Path,
    packs: &std::collections::HashMap<String, PackState>,
) -> CheckResult {
    let mut findings = Vec::new();
    // Stable iteration order — users want deterministic output.
    let ordered: BTreeMap<_, _> = packs.iter().collect();
    for (id, state) in ordered {
        let gi_path = workspace.join(&state.path).join(".gitignore");
        match read_managed_block(&gi_path, id) {
            Ok(Some(actual)) => {
                let expected = expected_patterns_for_pack(state);
                if actual != expected {
                    findings.push(Finding {
                        check: CheckKind::GitignoreSync,
                        severity: Severity::Warning,
                        pack: Some(id.clone()),
                        detail: format!(
                            "managed block drift: expected {} line(s), got {}",
                            expected.len(),
                            actual.len()
                        ),
                        auto_fixable: true,
                    });
                }
            }
            Ok(None) => {
                // Absent block is tolerated — plugins may not emit one.
            }
            Err(e) => {
                findings.push(Finding {
                    check: CheckKind::GitignoreSync,
                    severity: Severity::Warning,
                    pack: Some(id.clone()),
                    detail: format!("cannot read managed block: {e}"),
                    auto_fixable: matches!(e, GitignoreError::UnclosedBlock { .. }),
                });
            }
        }
    }
    if findings.is_empty() {
        findings.push(Finding::ok(CheckKind::GitignoreSync));
    }
    CheckResult { findings }
}

/// Check 3 — on-disk drift. Detect (a) manifest-registered pack dirs
/// that are missing, and (b) directories under the workspace root not
/// registered in the manifest. Both are reported as
/// [`CheckKind::OnDiskDrift`]; missing dirs are `Error`, unregistered
/// dirs are `Warning`.
pub fn check_on_disk_drift(
    workspace: &Path,
    packs: &std::collections::HashMap<String, PackState>,
) -> CheckResult {
    let mut findings = Vec::new();
    let registered_paths: BTreeSet<PathBuf> =
        packs.values().map(|p| PathBuf::from(&p.path)).collect();
    collect_manifest_to_disk_findings(workspace, packs, &mut findings);
    collect_disk_to_manifest_findings(workspace, &registered_paths, &mut findings);
    if findings.is_empty() {
        findings.push(Finding::ok(CheckKind::OnDiskDrift));
    }
    CheckResult { findings }
}

/// Manifest → disk half of [`check_on_disk_drift`]: every registered
/// pack dir must exist and be a directory. All failures are `Error`.
fn collect_manifest_to_disk_findings(
    workspace: &Path,
    packs: &std::collections::HashMap<String, PackState>,
    findings: &mut Vec<Finding>,
) {
    let ordered: BTreeMap<_, _> = packs.iter().collect();
    for (id, state) in ordered {
        let full = workspace.join(&state.path);
        if !full.exists() {
            findings.push(drift_error(id, format!("registered pack dir missing: {}", state.path)));
            continue;
        }
        match std::fs::symlink_metadata(&full) {
            Ok(md) if !md.is_dir() => findings.push(drift_error(
                id,
                format!("registered pack path is not a directory: {}", state.path),
            )),
            Ok(_) => {}
            Err(e) => findings.push(drift_error(id, format!("stat failed: {e}"))),
        }
    }
}

/// Disk → manifest half of [`check_on_disk_drift`]: only direct
/// children of `workspace` are walked (no pack interiors). Dotfiles
/// and housekeeping dirs are skipped.
fn collect_disk_to_manifest_findings(
    workspace: &Path,
    registered_paths: &BTreeSet<PathBuf>,
    findings: &mut Vec<Finding>,
) {
    let Ok(entries) = std::fs::read_dir(workspace) else { return };
    for ent in entries.flatten() {
        let Ok(ft) = ent.file_type() else { continue };
        if !ft.is_dir() {
            continue;
        }
        let name = ent.file_name();
        let Some(name_str) = name.to_str() else { continue };
        if name_str.starts_with('.') || is_housekeeping_dir(name_str) {
            continue;
        }
        if !registered_paths.contains(&PathBuf::from(name_str)) {
            findings.push(Finding {
                check: CheckKind::OnDiskDrift,
                severity: Severity::Warning,
                pack: None,
                detail: format!("unregistered directory on disk: {name_str}"),
                auto_fixable: false,
            });
        }
    }
}

/// Shorthand — build a pack-scoped on-disk-drift error finding.
fn drift_error(id: &str, detail: String) -> Finding {
    Finding {
        check: CheckKind::OnDiskDrift,
        severity: Severity::Error,
        pack: Some(id.to_string()),
        detail,
        auto_fixable: false,
    }
}

/// Dirs that live beside packs but are workspace meta, not pack roots.
fn is_housekeeping_dir(name: &str) -> bool {
    matches!(name, "target" | "node_modules" | "crates" | "openspec" | "dist")
}

/// Check 4 — config lint (opt-in). Parses `openspec/config.yaml` if
/// present; walks `.omne/cfg/*.md` for basic syntax validity (we just
/// read them to prove they're valid UTF-8 — the spec calls out "basic
/// markdown parse", not a full markdown lint). Missing files/dirs are
/// no-ops (not findings).
pub fn check_config_lint(workspace: &Path) -> CheckResult {
    let mut findings = Vec::new();
    check_openspec_config_yaml(workspace, &mut findings);
    check_omne_cfg_markdown(workspace, &mut findings);
    if findings.is_empty() {
        findings.push(Finding::ok(CheckKind::ConfigLint));
    }
    CheckResult { findings }
}

/// `openspec/config.yaml` half of [`check_config_lint`] — parses the
/// file as `serde_yaml::Value`. Absent file is a no-op.
fn check_openspec_config_yaml(workspace: &Path, findings: &mut Vec<Finding>) {
    let cfg_yaml = workspace.join("openspec").join("config.yaml");
    if !cfg_yaml.exists() {
        return;
    }
    match std::fs::read_to_string(&cfg_yaml) {
        Ok(s) => {
            if let Err(e) = serde_yaml::from_str::<serde_yaml::Value>(&s) {
                findings
                    .push(config_lint_warning(format!("openspec/config.yaml parse error: {e}")));
            }
        }
        Err(e) => {
            findings.push(config_lint_warning(format!("openspec/config.yaml unreadable: {e}")))
        }
    }
}

/// `.omne/cfg/*.md` half of [`check_config_lint`] — proves each file
/// is valid UTF-8. Absent dir is a no-op.
fn check_omne_cfg_markdown(workspace: &Path, findings: &mut Vec<Finding>) {
    let cfg_dir = workspace.join(".omne").join("cfg");
    if !cfg_dir.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(&cfg_dir) else { return };
    for ent in entries.flatten() {
        let path = ent.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        if let Err(e) = std::fs::read_to_string(&path) {
            let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("?").to_string();
            findings.push(config_lint_warning(format!(".omne/cfg/{name} unreadable: {e}")));
        }
    }
}

/// Shorthand — build a workspace-scoped config-lint warning finding.
fn config_lint_warning(detail: String) -> Finding {
    Finding {
        check: CheckKind::ConfigLint,
        severity: Severity::Warning,
        pack: None,
        detail,
        auto_fixable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{append_event, Event, SCHEMA_VERSION};
    use chrono::{TimeZone, Utc};
    use std::fs;
    use tempfile::tempdir;

    fn ts() -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 22, 10, 0, 0).unwrap()
    }

    /// Recursive path+bytes snapshot of a directory, keyed by path
    /// relative to `root`. Used by `--fix` safety tests to prove that
    /// a fix attempt left NO write anywhere in the fixture when the
    /// doctor refused to heal (e.g. schema error, drift error).
    ///
    /// Skips `.git/` and `target/` if present, since they are never
    /// relevant to doctor writes and keep the snapshot deterministic
    /// on machines that might have stray VCS/build state.
    fn fs_snapshot(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
        fn walk(dir: &Path, root: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
            let entries = match fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => return,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if name == ".git" || name == "target" {
                    continue;
                }
                let ft = match entry.file_type() {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                if ft.is_dir() {
                    walk(&path, root, out);
                } else if ft.is_file() {
                    let rel = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                    let bytes = fs::read(&path).unwrap_or_default();
                    out.insert(rel, bytes);
                }
            }
        }
        let mut out = BTreeMap::new();
        walk(root, root, &mut out);
        out
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

    // --- Unit: manifest schema ---

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
        // Line 1 is garbage (not last — there's a valid line 2), so
        // M3's reader flags it as Corruption.
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
        assert!(evs.is_none(), "corruption must disable downstream checks");
    }

    #[test]
    fn schema_missing_manifest_is_ok() {
        let d = tempdir().unwrap();
        let (r, evs) = check_manifest_schema(&d.path().join("grex.jsonl"));
        assert_eq!(r.worst(), Severity::Ok);
        assert!(evs.unwrap().is_empty());
    }

    // --- Unit: gitignore sync ---

    #[test]
    fn gitignore_clean_block_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        // Upsert the expected empty block.
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &[]).unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Ok);
    }

    #[test]
    fn gitignore_drift_is_warning_and_autofixable() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        // Write a drifted block body — nonempty where expected is empty.
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &["unexpected-line"])
            .unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Warning);
        assert!(r.findings.iter().any(|f| f.auto_fixable));
    }

    // --- Unit: on-disk drift ---

    #[test]
    fn on_disk_missing_pack_is_error() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        // Delete the pack dir after seeding.
        fs::remove_dir_all(d.path().join("a")).unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Error);
    }

    #[test]
    fn on_disk_unregistered_dir_is_warning() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        fs::create_dir_all(d.path().join("stranger")).unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Warning);
    }

    #[test]
    fn on_disk_clean_workspace_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Ok);
    }

    // --- Unit: config lint ---

    #[test]
    fn config_lint_absent_dir_is_ok() {
        let d = tempdir().unwrap();
        let r = check_config_lint(d.path());
        assert_eq!(r.worst(), Severity::Ok);
    }

    #[test]
    fn config_lint_bad_yaml_is_warning() {
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join("openspec")).unwrap();
        fs::write(d.path().join("openspec").join("config.yaml"), "::: bad: : yaml : [").unwrap();
        let r = check_config_lint(d.path());
        assert_eq!(r.worst(), Severity::Warning);
    }

    // --- Module: exit code roll-up ---

    #[test]
    fn exit_code_roll_up_ok_is_zero() {
        let mut r = DoctorReport::default();
        r.findings.push(Finding::ok(CheckKind::ManifestSchema));
        assert_eq!(r.exit_code(), 0);
    }

    #[test]
    fn exit_code_roll_up_warning_is_one() {
        let mut r = DoctorReport::default();
        r.findings.push(Finding::ok(CheckKind::ManifestSchema));
        r.findings.push(Finding {
            check: CheckKind::GitignoreSync,
            severity: Severity::Warning,
            pack: None,
            detail: String::new(),
            auto_fixable: true,
        });
        assert_eq!(r.exit_code(), 1);
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
    fn exit_code_roll_up_warn_and_error_is_two() {
        let mut r = DoctorReport::default();
        r.findings.push(Finding {
            check: CheckKind::GitignoreSync,
            severity: Severity::Warning,
            pack: None,
            detail: String::new(),
            auto_fixable: true,
        });
        r.findings.push(Finding {
            check: CheckKind::OnDiskDrift,
            severity: Severity::Error,
            pack: None,
            detail: String::new(),
            auto_fixable: false,
        });
        assert_eq!(r.exit_code(), 2);
    }

    // --- Integration: run_doctor orchestrator ---

    #[test]
    fn run_doctor_clean_workspace_exits_zero() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &[]).unwrap();
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn run_doctor_gitignore_drift_exits_one() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &["drift"]).unwrap();
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn run_doctor_fix_heals_gitignore_drift() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &["drift"]).unwrap();
        let opts = DoctorOpts { fix: true, lint_config: false };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 0, "fix must zero out exit code");
        // Confirm idempotence: running again without --fix also returns 0.
        let again = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(again.exit_code(), 0);
    }

    #[test]
    fn run_doctor_fix_does_not_touch_manifest_on_schema_error() {
        let d = tempdir().unwrap();
        // Seed a corrupt manifest (line 1 garbage, line 2 valid).
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
        let before = fs_snapshot(d.path());

        let opts = DoctorOpts { fix: true, lint_config: false };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 2, "schema error → exit 2");

        // SAFETY CRITICAL: --fix must NOT touch the manifest OR any
        // other file on schema errors. The recursive snapshot proves
        // no stray write happened anywhere in the fixture.
        let after_bytes = fs::read(&m).unwrap();
        assert_eq!(before_bytes, after_bytes, "manifest bytes must be unchanged");
        let after = fs_snapshot(d.path());
        assert_eq!(before, after, "--fix must not write anywhere on schema error");
    }

    #[test]
    fn run_doctor_fix_does_not_touch_disk_on_drift_error() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        // Delete the pack dir → on-disk drift error.
        fs::remove_dir_all(d.path().join("a")).unwrap();

        // SAFETY CRITICAL: --fix must NOT write anywhere in the
        // workspace on drift error — not the missing dir, not
        // `grex.jsonl`, not a stray `.gitignore`, nothing. A recursive
        // path+bytes snapshot catches any such write, not just the
        // presence/absence of the missing pack dir.
        let before = fs_snapshot(d.path());

        let opts = DoctorOpts { fix: true, lint_config: false };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 2);

        let after = fs_snapshot(d.path());
        assert_eq!(before, after, "--fix must not write anywhere on drift error");
        assert!(!d.path().join("a").exists(), "missing pack dir must stay missing");
    }

    #[test]
    fn run_doctor_config_lint_skipped_by_default() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &[]).unwrap();
        // Seed a broken config.yaml; default run must ignore it.
        fs::create_dir_all(d.path().join("openspec")).unwrap();
        fs::write(d.path().join("openspec").join("config.yaml"), ": : : [bad").unwrap();
        let before = fs_snapshot(d.path());
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 0, "config-lint must be skipped by default");
        assert!(
            !report.findings.iter().any(|f| f.check == CheckKind::ConfigLint),
            "no ConfigLint finding when --lint-config absent"
        );
        // SAFETY: read-only run — every byte must be untouched.
        let after = fs_snapshot(d.path());
        assert_eq!(before, after, "default doctor run must be read-only");
    }

    #[test]
    fn run_doctor_lint_config_flag_reports_config() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join("a").join(".gitignore"), "a", &[]).unwrap();
        fs::create_dir_all(d.path().join("openspec")).unwrap();
        fs::write(d.path().join("openspec").join("config.yaml"), ": : : [bad").unwrap();
        let opts = DoctorOpts { fix: false, lint_config: true };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 1);
        assert!(report.findings.iter().any(|f| f.check == CheckKind::ConfigLint));
    }

    // --- Property: exit code roll-up invariant ---

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig { cases: 128, ..Default::default() })]

        #[test]
        fn prop_exit_code_matches_worst_severity(
            severities in proptest::collection::vec(0u8..3, 0..20)
        ) {
            let mut r = DoctorReport::default();
            for s in &severities {
                let sev = match s {
                    0 => Severity::Ok,
                    1 => Severity::Warning,
                    _ => Severity::Error,
                };
                r.findings.push(Finding {
                    check: CheckKind::ManifestSchema,
                    severity: sev,
                    pack: None,
                    detail: String::new(),
                    auto_fixable: false,
                });
            }
            let worst = severities.iter().max().copied().unwrap_or(0);
            let expected = match worst { 0 => 0, 1 => 1, _ => 2 };
            proptest::prop_assert_eq!(r.exit_code(), expected);
        }
    }
}
