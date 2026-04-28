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

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::fs::gitignore::{read_managed_block, upsert_managed_block, GitignoreError};
use crate::lockfile::{read_lockfile, LockEntry, LockfileError};
use crate::manifest::{self, Event, ManifestError, PackState};
use crate::plugin::pack_type::default_managed_gitignore_patterns;

const GITIGNORE_EXT_KEY: &str = "x-gitignore";

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
    /// Per-pack synthetic-status row — emitted only for v1.1.1
    /// plain-git children whose lockfile entry has `synthetic: true`.
    /// Always reports `OK (synthetic)`; downstream JSON consumers see
    /// the `synthetic: true` flag on the finding.
    SyntheticPack,
}

impl CheckKind {
    /// Short human label used in the CLI table.
    pub fn label(self) -> &'static str {
        match self {
            CheckKind::ManifestSchema => "manifest-schema",
            CheckKind::GitignoreSync => "gitignore-sync",
            CheckKind::OnDiskDrift => "on-disk-drift",
            CheckKind::ConfigLint => "config-lint",
            CheckKind::SyntheticPack => "synthetic-pack",
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
///
/// Marked `#[non_exhaustive]` so future audit fields (per-finding
/// timestamp, plugin id, remediation hint) can land without breaking
/// out-of-crate consumers that destructure or struct-literal-construct
/// findings. Within `grex-core` the existing struct-literal sites
/// continue to work unchanged.
#[non_exhaustive]
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
    /// `true` when this finding describes a v1.1.1 synthetic plain-git
    /// pack (no `.grex/pack.yaml` on disk; manifest synthesised
    /// in-memory by the walker). Surfaced in `--json` output so
    /// downstream consumers can branch on the structured signal rather
    /// than parsing the human-readable detail string.
    pub synthetic: bool,
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
            synthetic: false,
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

    // v1.1.1 — load the lockfile so per-pack checks can branch on
    // `LockEntry::synthetic`. A missing lockfile is tolerated silently
    // (workspaces that have never synced are a normal state); a
    // corrupt / unreadable lockfile produces an empty map AND a
    // warning finding so operators see the root cause instead of the
    // downstream "unregistered directory on disk" warnings the on-disk
    // drift check would otherwise emit (those warnings rely on the
    // synthetic flag that was just swallowed).
    let (lock, lock_finding) = read_synthetic_lock(workspace);
    if let Some(f) = lock_finding {
        report.findings.push(f);
    }

    let gi_result = match &packs {
        Some(p) => check_gitignore_sync(workspace, p),
        None => CheckResult::single(Finding {
            check: CheckKind::GitignoreSync,
            severity: Severity::Warning,
            pack: None,
            detail: "skipped: manifest unreadable".to_string(),
            auto_fixable: false,
            synthetic: false,
        }),
    };
    report.findings.extend(gi_result.findings.clone());

    let drift_result = match &packs {
        Some(p) => check_on_disk_drift(workspace, p, &lock),
        None => CheckResult::single(Finding {
            check: CheckKind::OnDiskDrift,
            severity: Severity::Warning,
            pack: None,
            detail: "skipped: manifest unreadable".to_string(),
            auto_fixable: false,
            synthetic: false,
        }),
    };
    report.findings.extend(drift_result.findings);

    // v1.1.1 — synthetic plain-git children only ever land in the
    // lockfile (no `Event::Add` is logged for them). Iterate the
    // lockfile, not the manifest-derived `packs` map, so the canonical
    // sync-only flow surfaces an `OK (synthetic)` row per child.
    let synth = check_synthetic_packs(&lock);
    report.findings.extend(synth.findings);

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
        let gi_path = workspace.join(".gitignore");
        let expected = expected_patterns_for_pack(workspace, state);
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
                    synthetic: false,
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
                    synthetic: false,
                }),
                None,
            )
        }
    }
}

/// Expected managed-block patterns for a single pack.
///
/// The built-in pack-type plugins (`meta`, `declarative`, `scripted`) all
/// call `pack_type::apply_gitignore`, which writes the grex default
/// patterns first, then appends authored `x-gitignore` entries from the
/// pack manifest without duplicating defaults. Unknown pack types are
/// plugin-owned; doctor has no v1 contract for their emitted patterns.
fn expected_patterns_for_pack(workspace: &Path, state: &PackState) -> Vec<String> {
    if !is_builtin_pack_type(&state.pack_type) {
        return Vec::new();
    }

    let mut expected: Vec<String> =
        default_managed_gitignore_patterns().iter().map(|p| (*p).to_string()).collect();

    for pattern in authored_gitignore_patterns(workspace, state) {
        if !expected.iter().any(|p| p == &pattern) {
            expected.push(pattern);
        }
    }

    expected
}

fn is_builtin_pack_type(pack_type: &str) -> bool {
    matches!(pack_type, "meta" | "declarative" | "scripted")
}

fn authored_gitignore_patterns(workspace: &Path, state: &PackState) -> Vec<String> {
    let pack_yaml = workspace.join(&state.path).join(".grex").join("pack.yaml");
    let Ok(contents) = std::fs::read_to_string(pack_yaml) else {
        return Vec::new();
    };
    let Ok(pack) = crate::pack::parse(&contents) else {
        return Vec::new();
    };
    let Some(raw) = pack.extensions.get(GITIGNORE_EXT_KEY) else {
        return Vec::new();
    };
    let Some(seq) = raw.as_sequence() else {
        return Vec::new();
    };
    seq.iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
}

/// Check 2 — gitignore sync. For every pack with a managed block in
/// the workspace `.gitignore`, compare the body to the expected pattern
/// list.
pub fn check_gitignore_sync(
    workspace: &Path,
    packs: &std::collections::HashMap<String, PackState>,
) -> CheckResult {
    let mut findings = Vec::new();
    // Stable iteration order - users want deterministic output.
    let ordered: BTreeMap<_, _> = packs.iter().collect();
    let gi_path = workspace.join(".gitignore");
    for (id, state) in ordered {
        match read_managed_block(&gi_path, id) {
            Ok(Some(actual)) => {
                let expected = expected_patterns_for_pack(workspace, state);
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
                        synthetic: false,
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
                    synthetic: false,
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
///
/// `lock` is consulted to suppress unregistered-drift warnings for
/// directories whose lockfile entry has `synthetic: true` — those are
/// v1.1.1 plain-git children that never get an `Event::Add`, so the
/// lockfile is the authoritative registry for them.
pub fn check_on_disk_drift(
    workspace: &Path,
    packs: &std::collections::HashMap<String, PackState>,
    lock: &HashMap<String, LockEntry>,
) -> CheckResult {
    let mut findings = Vec::new();
    let registered_paths: BTreeSet<PathBuf> =
        packs.values().map(|p| PathBuf::from(&p.path)).collect();
    collect_manifest_to_disk_findings(workspace, packs, &mut findings);
    collect_disk_to_manifest_findings(workspace, &registered_paths, lock, &mut findings);
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
///
/// Directories matching a synthetic lockfile entry (`lock[name].synthetic
/// == true`) are also skipped — v1.1.1 plain-git children never appear
/// in `Event::Add`, so the lockfile is authoritative for them.
fn collect_disk_to_manifest_findings(
    workspace: &Path,
    registered_paths: &BTreeSet<PathBuf>,
    lock: &HashMap<String, LockEntry>,
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
        if registered_paths.contains(&PathBuf::from(name_str)) {
            continue;
        }
        if lock.get(name_str).is_some_and(|e| e.synthetic) {
            continue;
        }
        findings.push(Finding {
            check: CheckKind::OnDiskDrift,
            severity: Severity::Warning,
            pack: None,
            detail: format!("unregistered directory on disk: {name_str}"),
            auto_fixable: false,
            synthetic: false,
        });
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
        synthetic: false,
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

/// Read the workspace's lockfile and return entries keyed by pack id,
/// alongside an optional finding when the lockfile exists but cannot
/// be parsed.
///
/// Behaviour:
/// * Missing lockfile → `(empty map, None)`. A workspace that has
///   never synced is a normal state, not a finding.
/// * Corruption (`LockfileError::Corruption`) or I/O failure
///   (`LockfileError::Io`) → `(empty map, Some(Warning))`. The
///   downstream synthetic / on-disk-drift checks still run against an
///   empty map, but the operator now sees the root cause instead of
///   being misled by spurious "unregistered directory on disk"
///   warnings (the on-disk-drift skip relies on the synthetic flag in
///   the lockfile entries that just got swallowed).
fn read_synthetic_lock(workspace: &Path) -> (HashMap<String, LockEntry>, Option<Finding>) {
    let lock_path = workspace.join(".grex").join("grex.lock.jsonl");
    match read_lockfile(&lock_path) {
        Ok(map) => (map, None),
        Err(err @ LockfileError::Corruption { .. }) | Err(err @ LockfileError::Io(_)) => {
            let finding = Finding {
                check: CheckKind::ManifestSchema,
                severity: Severity::Warning,
                pack: None,
                detail: format!("lockfile corruption: {err}"),
                auto_fixable: false,
                synthetic: false,
            };
            (HashMap::new(), Some(finding))
        }
        // `read_lockfile` already maps NotFound → Ok(empty), and
        // `Serialize` is write-side only, so neither path is reachable
        // here. Tolerate any future variant by treating it the same as
        // a corruption warning rather than panicking.
        Err(err) => {
            let finding = Finding {
                check: CheckKind::ManifestSchema,
                severity: Severity::Warning,
                pack: None,
                detail: format!("lockfile corruption: {err}"),
                auto_fixable: false,
                synthetic: false,
            };
            (HashMap::new(), Some(finding))
        }
    }
}

/// v1.1.1 — emit one `OK (synthetic)` finding per pack whose lockfile
/// entry has `synthetic: true`.
///
/// The lockfile is the canonical synthetic registry: plain-git children
/// are walked + cloned during `grex sync` and only ever recorded in
/// `grex.lock.jsonl` (no `Event::Add` fires for them, so they never
/// appear in the manifest-fold `packs` map). Iterating the lockfile
/// here means the canonical sync-only flow surfaces the row, and
/// downstream JSON consumers see the structured `synthetic: true`
/// signal regardless of whether the pack also has an `Event::Add`.
pub fn check_synthetic_packs(lock: &HashMap<String, LockEntry>) -> CheckResult {
    let mut findings = Vec::new();
    let ordered: BTreeMap<_, _> = lock.iter().collect();
    for (id, entry) in ordered {
        if !entry.synthetic {
            continue;
        }
        findings.push(Finding {
            check: CheckKind::SyntheticPack,
            severity: Severity::Ok,
            pack: Some(id.clone()),
            detail: "OK (synthetic)".to_string(),
            auto_fixable: false,
            synthetic: true,
        });
    }
    CheckResult { findings }
}

/// Shorthand — build a workspace-scoped config-lint warning finding.
fn config_lint_warning(detail: String) -> Finding {
    Finding {
        check: CheckKind::ConfigLint,
        severity: Severity::Warning,
        pack: None,
        detail,
        auto_fixable: false,
        synthetic: false,
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
        seed_pack_with_type(workspace, id, "declarative");
    }

    fn seed_pack_with_type(workspace: &Path, id: &str, pack_type: &str) {
        let m = workspace.join("grex.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: id.into(),
                url: format!("https://example/{id}"),
                path: id.into(),
                pack_type: pack_type.into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        fs::create_dir_all(workspace.join(id)).unwrap();
    }

    fn write_pack_yaml(workspace: &Path, id: &str, yaml: &str) {
        let dir = workspace.join(id).join(".grex");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("pack.yaml"), yaml).unwrap();
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
    fn expected_patterns_for_pack_populates_builtin_defaults() {
        for pack_type in ["meta", "declarative", "scripted"] {
            let d = tempdir().unwrap();
            seed_pack_with_type(d.path(), pack_type, pack_type);
            let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
            let packs = manifest::fold(events);
            let state = packs.get(pack_type).unwrap();
            assert_eq!(
                expected_patterns_for_pack(d.path(), state),
                vec![".grex-lock".to_string()],
                "pack type: {pack_type}"
            );
        }
    }

    #[test]
    fn expected_patterns_for_pack_merges_authored_extensions_for_builtins() {
        for pack_type in ["meta", "declarative", "scripted"] {
            let d = tempdir().unwrap();
            let id = format!("{pack_type}-pack");
            let authored = format!("{pack_type}-cache/");
            seed_pack_with_type(d.path(), &id, pack_type);
            write_pack_yaml(
                d.path(),
                &id,
                &format!(
                    "schema_version: \"1\"\nname: {id}\ntype: {pack_type}\nx-gitignore:\n  - \".grex-lock\"\n  - {authored}\n",
                ),
            );
            let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
            let packs = manifest::fold(events);
            let state = packs.get(&id).unwrap();
            assert_eq!(
                expected_patterns_for_pack(d.path(), state),
                vec![".grex-lock".to_string(), authored],
                "pack type: {pack_type}"
            );
        }
    }

    #[test]
    fn gitignore_clean_block_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        // Upsert the expected workspace-level block.
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Ok);
    }

    #[test]
    fn gitignore_drift_is_warning_and_autofixable() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        // Write a drifted workspace-level block body.
        upsert_managed_block(&d.path().join(".gitignore"), "a", &["unexpected-line"]).unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Warning);
        assert!(r.findings.iter().any(|f| f.auto_fixable));
    }

    #[test]
    fn gitignore_authored_patterns_are_not_reported_as_drift() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        write_pack_yaml(
            d.path(),
            "a",
            "schema_version: \"1\"\nname: a\ntype: declarative\nx-gitignore:\n  - target/\n  - \"*.log\"\n",
        );
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            &[".grex-lock", "target/", "*.log"],
        )
        .unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_gitignore_sync(d.path(), &packs);
        assert_eq!(r.worst(), Severity::Ok);
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
        let r = check_on_disk_drift(d.path(), &packs, &HashMap::new());
        assert_eq!(r.worst(), Severity::Error);
    }

    #[test]
    fn on_disk_unregistered_dir_is_warning() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        fs::create_dir_all(d.path().join("stranger")).unwrap();
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs, &HashMap::new());
        assert_eq!(r.worst(), Severity::Warning);
    }

    #[test]
    fn on_disk_clean_workspace_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        let events = manifest::read_all(&d.path().join("grex.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs, &HashMap::new());
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
            synthetic: false,
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
            synthetic: false,
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
            synthetic: false,
        });
        r.findings.push(Finding {
            check: CheckKind::OnDiskDrift,
            severity: Severity::Error,
            pack: None,
            detail: String::new(),
            auto_fixable: false,
            synthetic: false,
        });
        assert_eq!(r.exit_code(), 2);
    }

    // --- Integration: run_doctor orchestrator ---

    #[test]
    fn run_doctor_clean_workspace_exits_zero() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 0);
    }

    #[test]
    fn run_doctor_gitignore_drift_exits_one() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join(".gitignore"), "a", &["drift"]).unwrap();
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 1);
    }

    #[test]
    fn run_doctor_fix_heals_gitignore_drift() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(&d.path().join(".gitignore"), "a", &["drift"]).unwrap();
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
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();
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
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();
        fs::create_dir_all(d.path().join("openspec")).unwrap();
        fs::write(d.path().join("openspec").join("config.yaml"), ": : : [bad").unwrap();
        let opts = DoctorOpts { fix: false, lint_config: true };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 1);
        assert!(report.findings.iter().any(|f| f.check == CheckKind::ConfigLint));
    }

    // --- v1.1.1: synthetic plain-git children ---

    /// A workspace whose lockfile carries a `synthetic: true` entry for
    /// pack `a` reports `OK (synthetic)` for it, exits 0, and never
    /// emits a missing-manifest finding even though no `.grex/pack.yaml`
    /// exists on disk for that pack.
    #[test]
    fn run_doctor_synthetic_pack_reports_ok_synthetic_and_exits_zero() {
        use crate::lockfile::{write_lockfile, LockEntry};
        use std::collections::HashMap;

        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();

        // Hand-write a lockfile with `synthetic: true` for pack `a`.
        let lock_dir = d.path().join(".grex");
        fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("grex.lock.jsonl");
        let mut lock = HashMap::new();
        lock.insert(
            "a".to_string(),
            LockEntry {
                id: "a".into(),
                sha: "deadbeef".into(),
                branch: "main".into(),
                installed_at: ts(),
                actions_hash: String::new(),
                schema_version: "1".into(),
                synthetic: true,
            },
        );
        write_lockfile(&lock_path, &lock).unwrap();

        // Note: we deliberately do NOT write `<pack>/.grex/pack.yaml`,
        // matching the v1.1.1 plain-git-child case.
        let report = run_doctor(d.path(), &DoctorOpts::default()).unwrap();
        assert_eq!(report.exit_code(), 0, "synthetic-only workspace must exit 0");
        let synth: Vec<_> =
            report.findings.iter().filter(|f| f.check == CheckKind::SyntheticPack).collect();
        assert_eq!(synth.len(), 1, "exactly one synthetic-pack finding");
        assert_eq!(synth[0].pack.as_deref(), Some("a"));
        assert_eq!(synth[0].detail, "OK (synthetic)");
        assert!(synth[0].synthetic, "Finding.synthetic must be true");
        assert_eq!(synth[0].severity, Severity::Ok);

        // Sanity: nothing in the report claims pack `a` is missing or
        // schema-invalid.
        for f in &report.findings {
            assert!(f.severity != Severity::Error, "no error-severity finding allowed; got: {f:?}",);
        }
    }

    /// A workspace whose `.grex/grex.lock.jsonl` is malformed (one line
    /// of invalid JSON) must produce a `Severity::Warning` finding
    /// mentioning lockfile corruption. The doctor must still complete
    /// — lockfile errors are findings, not orchestration aborts —
    /// because operators rely on `grex doctor` to surface root causes,
    /// not crash on them.
    #[test]
    fn run_doctor_corrupt_lockfile_emits_warning_finding() {
        let d = tempdir().unwrap();
        // Seed a clean schema-and-gitignore baseline so the corruption
        // finding stands out against an otherwise-OK report.
        seed_pack(d.path(), "a");
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();

        // Hand-write a malformed lockfile: one line of invalid JSON.
        let lock_dir = d.path().join(".grex");
        fs::create_dir_all(&lock_dir).unwrap();
        fs::write(lock_dir.join("grex.lock.jsonl"), b"not-json-at-all\n").unwrap();

        let report = run_doctor(d.path(), &DoctorOpts::default())
            .expect("doctor must complete despite lockfile corruption");

        // The lockfile corruption is reported as a ManifestSchema
        // warning whose detail mentions "lockfile corruption".
        let lock_warns: Vec<_> = report
            .findings
            .iter()
            .filter(|f| {
                f.check == CheckKind::ManifestSchema
                    && f.severity == Severity::Warning
                    && f.detail.contains("lockfile corruption")
            })
            .collect();
        assert_eq!(
            lock_warns.len(),
            1,
            "exactly one lockfile-corruption warning expected; got: {:?}",
            report.findings,
        );

        // Doctor still completed: report has the usual gitignore /
        // drift / synthetic rows even though the lockfile was unusable.
        assert!(
            report.findings.iter().any(|f| f.check == CheckKind::GitignoreSync),
            "gitignore-sync check must still run",
        );
        assert!(
            report.findings.iter().any(|f| f.check == CheckKind::OnDiskDrift),
            "on-disk-drift check must still run",
        );
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
                    synthetic: false,
                });
            }
            let worst = severities.iter().max().copied().unwrap_or(0);
            let expected = match worst { 0 => 0, 1 => 1, _ => 2 };
            proptest::prop_assert_eq!(r.exit_code(), expected);
        }
    }
}
