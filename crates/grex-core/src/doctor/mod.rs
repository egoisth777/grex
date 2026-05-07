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

pub mod scan_undeclared;
pub use scan_undeclared::{scan_undeclared, ScanError, UndeclaredRepo};

const GITIGNORE_EXT_KEY: &str = "x-gitignore";

/// Which check produced this finding.
///
/// Marked `#[non_exhaustive]` so future check kinds (additional
/// quarantine ops, plugin-contributed checks, etc.) can be added in a
/// PATCH release without breaking out-of-crate `match` consumers. Within
/// `grex-core` every match arm is exhaustive.
#[non_exhaustive]
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
    /// v1.2.5 — quarantine GC status. Reports `OK` on a clean trash
    /// bucket (no entries, or all within the retention window) and an
    /// `Info`-severity Warning carrying the stale-entry count when the
    /// bucket holds entries older than `--retain-days N`.
    QuarantineGc,
    /// v1.2.5 — quarantine restore status. Surfaced exclusively by the
    /// `--restore-quarantine TS[:BASENAME]` op (operator-requested
    /// snapshot rehydration). Distinct from [`CheckKind::QuarantineGc`]
    /// so JSON consumers can branch on `restore` vs `gc` outcomes
    /// without parsing the human-readable detail string.
    QuarantineRestore,
    /// v1.3.1 (B12) — advisory: a pack directory under the meta-repo
    /// is present in the parent git index (`git ls-files` returns a
    /// match). The advisory is **informational only**: it carries
    /// [`Severity::Ok`] so the worst-severity exit-code roll-up is not
    /// affected. Operators can dismiss the finding by adding the pack
    /// path to the parent meta-repo's `.gitignore` and (optionally)
    /// running `git rm --cached <pack>` once. Doctor never auto-mutates
    /// the parent meta-repo's `.gitignore` — that contract is owned by
    /// the operator.
    ParentGitTracksPackContent,
    /// v1.3.3 (B5) — advisory: a registered pack's path is NOT covered
    /// by any rule in the parent git repo's `.gitignore`. Surfaced as
    /// **warn-only** (`Severity::Ok`) so the exit-code roll-up is not
    /// affected. Aggregated across all drift candidates into a single
    /// summary finding at the end of the check rather than per-pack
    /// rows. Operator action: add the pack path to the parent repo's
    /// `.gitignore` (or narrow an over-broad rule). grex never mutates
    /// the parent `.gitignore`.
    GitignoreDrift,
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
            CheckKind::QuarantineGc => "quarantine-gc",
            CheckKind::QuarantineRestore => "quarantine-restore",
            CheckKind::ParentGitTracksPackContent => "parent-git-tracks-pack-content",
            CheckKind::GitignoreDrift => "gitignore-drift",
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
///
/// Marked `#[non_exhaustive]` so future opt fields (additional
/// quarantine knobs, audit toggles, etc.) can be added in a PATCH
/// release without breaking downstream destructuring (`let DoctorOpts
/// { fix, .. } = opts;`). External callers cannot use struct-literal
/// construction at all per E0639 — even the `..base` spread shorthand
/// is rejected. Construct via `DoctorOpts::default()` and mutate fields
/// on a `mut` binding instead. In-crate code can still struct-literal
/// freely.
#[non_exhaustive]
#[derive(Debug, Clone, Default)]
pub struct DoctorOpts {
    /// Heal gitignore drift. Only fixes [`CheckKind::GitignoreSync`]
    /// findings; all other checks remain read-only.
    pub fix: bool,
    /// Run the opt-in config-lint check. When `false`,
    /// [`CheckKind::ConfigLint`] never appears in the report.
    pub lint_config: bool,
    /// v1.2.0 Stage 1.j — depth bound on recursive ManifestTree walk.
    ///
    /// * `None` (default) → walk every nested meta exhaustively.
    /// * `Some(0)` → root meta only (no recursion).
    /// * `Some(n)` → recurse up to `n` levels of nesting (root is
    ///   depth 0; depth-`n` metas are visited but their children are
    ///   not).
    ///
    /// The walk is read-only — no clones, fetches, or filesystem
    /// mutations happen at any frame regardless of `shallow`.
    pub shallow: Option<usize>,
    /// v1.2.5 — when `Some(N)`, run the quarantine GC sweep against
    /// every visited meta's `<meta>/.grex/trash/` bucket using `N`-day
    /// retention. Reports per-meta findings (`Info`/`Warning` for any
    /// entries actually pruned) and surfaces an `OK` finding on a
    /// clean sweep. `None` (default) skips the sweep entirely; the
    /// default doctor walk stays read-only.
    pub prune_quarantine: Option<u32>,
    /// v1.2.5 — when `Some((ts, basename))`, restore the snapshot at
    /// `<workspace>/.grex/trash/<ts>/<basename>/` back to the
    /// workspace at `<workspace>/<basename>`. `basename = None`
    /// requires the `<ts>/` slot to hold exactly one child entry.
    /// Refuses to clobber an existing dest unless [`Self::force`] is
    /// also `true`. Run-once at the root meta; not threaded into the
    /// recursive walk.
    pub restore_quarantine: Option<(String, Option<String>)>,
    /// v1.2.5 — paired with [`Self::restore_quarantine`]: when `true`,
    /// remove the existing dest before the rename. Default `false`
    /// surfaces [`crate::tree::QuarantineError::DestExists`] instead.
    /// Not used by other checks.
    pub force: bool,
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
///
/// v1.2.0 Stage 1.j: walks the ManifestTree depth-first by default,
/// running every per-meta check at each frame. `opts.shallow` bounds
/// the recursion (`None` = unbounded, `Some(0)` = root-only,
/// `Some(n)` = up to `n` nested levels). Recursion is read-only —
/// no clones, fetches, or filesystem mutations happen at any frame.
#[allow(clippy::too_many_lines)] // 63 lines: linear orchestration over migration → walk → quarantine GC → restore branches; splitting harms readability.
pub fn run_doctor(workspace: &Path, opts: &DoctorOpts) -> Result<DoctorReport, DoctorError> {
    // Auto-migrate v1.x `<ws>/grex.jsonl` → v2 `<ws>/.grex/events.jsonl`
    // before the schema check so doctor sees a consistent canonical
    // location whether the workspace was synced under v1.x or v2.0+.
    // Migration is the ONLY write doctor ever performs at the root
    // frame (and only when a legacy v1.x layout is present); the
    // recursive `walk_meta` step never migrates sub-meta event logs.
    manifest::ensure_event_log_migrated(workspace).map_err(DoctorError::ManifestIo)?;

    let mut report = DoctorReport::default();
    walk_meta(workspace, opts, /* depth */ 0, &mut report);

    if opts.lint_config {
        let cfg_result = check_config_lint(workspace);
        report.findings.extend(cfg_result.findings);
    }

    // v1.2.5 — quarantine-GC check / sweep at the root meta. The
    // recursive `walk_meta` does not branch into per-meta GC because
    // sweeps SHOULD only fire when explicitly requested by the
    // operator (default doctor stays read-only). Run-once at root
    // matches the `--prune-quarantine [--retain-days N]` design.
    if let Some(retain_days) = opts.prune_quarantine {
        let qc = check_quarantine_gc(workspace, retain_days, /* prune */ true);
        report.findings.extend(qc.findings);
    }

    // v1.2.5 — operator-requested restore. Single-shot at the root
    // meta; failures surface as an Error finding so the report still
    // returns an exit code rather than aborting the orchestration.
    // Findings are tagged `CheckKind::QuarantineRestore` (distinct from
    // `QuarantineGc`) so JSON consumers can branch on the structured
    // op rather than parsing the detail string.
    if let Some((ts, basename)) = &opts.restore_quarantine {
        use crate::tree::quarantine::restore_quarantine;
        let audit_log = crate::manifest::event_log_path(workspace);
        // Defensive: if the operator pasted an ISO-8601 timestamp with
        // colons (`2026-05-02T10:30:00Z`) into the `TS:BASENAME`
        // syntax, we'd see extra colons baked into `ts` (the CLI
        // splitn(2, ':') gives `ts="2026-05-02T10"` and
        // `basename="30:00Z"` — almost certainly NOT what the operator
        // meant). Surface a clearer hint than a downstream
        // "snapshot not found" lookup. The CLI itself ought to grow
        // `--restore-quarantine TS [--basename B]` (Reviewer 1 P2-5);
        // until then this hint catches the common foot-gun in-band.
        let finding = if ts.contains(':') || basename.as_deref().is_some_and(|b| b.contains(':')) {
            Finding {
                check: CheckKind::QuarantineRestore,
                severity: Severity::Error,
                pack: None,
                detail: format!(
                    "restore failed: malformed `TS[:BASENAME]` argument (ts={ts:?}, basename={basename:?}) — the syntax splits on the FIRST colon, so an ISO-8601 timestamp like `2026-05-02T10:30:00Z` is ambiguous. Use the trash slot directory name verbatim (e.g. `2026-05-02T10-30-00Z`) followed by at most one colon + basename."
                ),
                auto_fixable: false,
                synthetic: false,
            }
        } else {
            let res = restore_quarantine(
                workspace,
                ts,
                basename.as_deref(),
                opts.force,
                Some(&audit_log),
            );
            match res {
                Ok(report_inner) => Finding {
                    check: CheckKind::QuarantineRestore,
                    severity: Severity::Ok,
                    pack: None,
                    detail: format!("restored snapshot to {}", report_inner.dest.display()),
                    auto_fixable: false,
                    synthetic: false,
                },
                Err(e) => Finding {
                    check: CheckKind::QuarantineRestore,
                    severity: Severity::Error,
                    pack: None,
                    detail: format!("restore failed: {e}"),
                    auto_fixable: false,
                    synthetic: false,
                },
            }
        };
        report.findings.push(finding);
    }

    if opts.fix {
        // `--fix` heals only the root meta's gitignore. Sub-meta
        // gitignore drift is reported but never auto-healed — the
        // recursive walk is read-only by contract.
        let manifest_path = workspace.join(".grex").join("events.jsonl");
        let packs = match manifest::read_all(&manifest_path) {
            Ok(evs) => Some(manifest::fold(evs)),
            Err(_) => None,
        };
        apply_fixes(workspace, packs.as_ref(), &mut report)?;
    }

    Ok(report)
}

/// Run the per-meta checks at `meta_dir`, then recurse into every child
/// whose dest carries its own `<dest>/.grex/pack.yaml` while
/// `depth + 1 <= shallow_cap`. Mirrors the topology of
/// [`crate::lockfile::read_lockfile_tree`] (1.h) so doctor and the
/// distributed-lockfile fold agree on what counts as a sub-meta.
///
/// Read-only: no FS mutations. The schema check uses the per-meta
/// `<meta>/.grex/events.jsonl`; the gitignore-sync, on-disk-drift, and
/// synthetic-pack checks use the per-meta `<meta>/.grex/grex.lock.jsonl`.
fn walk_meta(meta_dir: &Path, opts: &DoctorOpts, depth: usize, report: &mut DoctorReport) {
    run_meta_checks(meta_dir, report);

    if let Some(cap) = opts.shallow {
        if depth >= cap {
            return;
        }
    }

    // Discover nested metas via the manifest, exactly like
    // `read_lockfile_tree`'s fold.
    let manifest_path = meta_dir.join(".grex").join("pack.yaml");
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let manifest = match crate::pack::parse(&raw) {
        Ok(m) => m,
        Err(_) => return,
    };
    for child in &manifest.children {
        let segment = child.path.clone().unwrap_or_else(|| child.effective_path());
        let child_meta = meta_dir.join(&segment);
        if child_meta.join(".grex").join("pack.yaml").is_file() {
            walk_meta(&child_meta, opts, depth + 1, report);
        }
    }
}

/// Run the per-meta checks (schema + gitignore-sync + on-disk-drift +
/// synthetic-pack) for a single meta directory and append their findings
/// to `report`. Pure read-only: never mutates the filesystem.
fn run_meta_checks(meta_dir: &Path, report: &mut DoctorReport) {
    let manifest_path = meta_dir.join(".grex").join("events.jsonl");
    let (schema_result, events_opt) = check_manifest_schema(&manifest_path);
    report.findings.extend(schema_result.findings.clone());

    // Subsequent pack-level checks need the folded state. If the
    // manifest is malformed, we still surface the schema error and skip
    // the dependent checks so we don't double-report garbage.
    let packs = events_opt.map(manifest::fold);

    // v1.1.1 — load the lockfile so per-pack checks can branch on
    // `LockEntry::synthetic`. A missing lockfile is tolerated silently;
    // a corrupt / unreadable lockfile produces an empty map AND a
    // warning finding.
    let (lock, lock_finding) = read_synthetic_lock(meta_dir);
    if let Some(f) = lock_finding {
        report.findings.push(f);
    }

    let gi_result = match &packs {
        Some(p) => check_gitignore_sync(meta_dir, p),
        None => CheckResult::single(Finding {
            check: CheckKind::GitignoreSync,
            severity: Severity::Warning,
            pack: None,
            detail: "skipped: manifest unreadable".to_string(),
            auto_fixable: false,
            synthetic: false,
        }),
    };
    report.findings.extend(gi_result.findings);

    let drift_result = match &packs {
        Some(p) => check_on_disk_drift(meta_dir, p, &lock),
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

    let synth = check_synthetic_packs(&lock);
    report.findings.extend(synth.findings);

    // v1.3.1 (B12) — advisory: report when a pack path under this
    // meta is tracked by the parent meta-repo's git index. Pure
    // read-only probe; emits an `Info`-equivalent finding
    // (`Severity::Ok`) per pack so the exit-code roll-up is not
    // affected. The advisory is mute when no parent git repo is
    // visible above `meta_dir`.
    let parent_findings = check_parent_git_tracks_pack_content(meta_dir, packs.as_ref());
    report.findings.extend(parent_findings.findings);

    // v1.3.3 (B5) — advisory: detect packs whose path is not covered
    // by the parent git repo's `.gitignore`. Warn-only, aggregated
    // into one summary finding. Skipped silently when the manifest is
    // unreadable (already reported by the schema check) or when no
    // parent git repo is visible above `meta_dir`.
    let drift_findings = check_gitignore_drift(meta_dir, packs.as_ref());
    report.findings.extend(drift_findings.findings);
}

/// v1.2.5 — quarantine-GC check. Surveys `<meta>/.grex/trash/` and
/// reports either a clean `Ok` finding (no aged entries OR no trash
/// bucket at all) or a `Warning` finding carrying the count of stale
/// entries that would be swept under the supplied retention window.
/// When `prune == true`, the check ALSO runs the actual sweep and
/// records the pruned entries in the finding detail; otherwise the
/// check is purely informational (matches the default doctor read-only
/// contract).
#[allow(clippy::too_many_lines)]
pub fn check_quarantine_gc(meta_dir: &Path, retain_days: u32, prune: bool) -> CheckResult {
    use crate::tree::quarantine::{parse_iso8601_quarantine, prune_quarantine, RetentionConfig};
    use std::time::{Duration, SystemTime};

    let trash_root = meta_dir.join(".grex").join("trash");
    if !trash_root.is_dir() {
        return CheckResult::single(Finding::ok(CheckKind::QuarantineGc));
    }
    let cutoff = SystemTime::now()
        .checked_sub(Duration::from_secs(u64::from(retain_days) * 86_400))
        .unwrap_or(SystemTime::UNIX_EPOCH);

    if prune {
        let retention = RetentionConfig { retain_days };
        let audit_log = crate::manifest::event_log_path(meta_dir);
        let report = match prune_quarantine(meta_dir, retention, Some(&audit_log)) {
            Ok(r) => r,
            Err(e) => {
                return CheckResult::single(Finding {
                    check: CheckKind::QuarantineGc,
                    severity: Severity::Warning,
                    pack: None,
                    detail: format!("GC sweep failed: {e}"),
                    auto_fixable: false,
                    synthetic: false,
                });
            }
        };
        if report.pruned.is_empty() && report.failed.is_empty() {
            return CheckResult::single(Finding::ok(CheckKind::QuarantineGc));
        }
        let mut detail =
            format!("pruned {} entr{}", report.pruned.len(), pluralize(report.pruned.len()));
        if !report.failed.is_empty() {
            detail.push_str(&format!("; {} failed", report.failed.len()));
        }
        return CheckResult::single(Finding {
            check: CheckKind::QuarantineGc,
            severity: if report.failed.is_empty() { Severity::Warning } else { Severity::Error },
            pack: None,
            detail,
            auto_fixable: false,
            synthetic: false,
        });
    }

    // Read-only inspection: count stale entries without deleting.
    let entries = match std::fs::read_dir(&trash_root) {
        Ok(e) => e,
        Err(e) => {
            return CheckResult::single(Finding {
                check: CheckKind::QuarantineGc,
                severity: Severity::Warning,
                pack: None,
                detail: format!("cannot read trash bucket: {e}"),
                auto_fixable: false,
                synthetic: false,
            });
        }
    };
    let mut stale = 0usize;
    for ent in entries.flatten() {
        let name = ent.file_name();
        let Some(name_str) = name.to_str() else { continue };
        let Some(ts) = parse_iso8601_quarantine(name_str) else { continue };
        if ts < cutoff {
            stale += 1;
        }
    }
    if stale == 0 {
        CheckResult::single(Finding::ok(CheckKind::QuarantineGc))
    } else {
        CheckResult::single(Finding {
            check: CheckKind::QuarantineGc,
            severity: Severity::Warning,
            pack: None,
            detail: format!(
                "{stale} stale entr{} older than {retain_days}d (run `grex doctor --prune-quarantine --retain-days {retain_days}` to sweep)",
                pluralize(stale),
            ),
            auto_fixable: false,
            synthetic: false,
        })
    }
}

fn pluralize(n: usize) -> &'static str {
    if n == 1 {
        "y"
    } else {
        "ies"
    }
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
/// Directories matching any lockfile entry are also skipped — the
/// lockfile is the authoritative registry for packs that don't surface
/// as `Event::Add` rows (v1.1.1 plain-git children, v1.2.0+ walker-
/// synthesised leaves). v1.3.2 W1 retired the `LockEntry.synthetic`
/// writer flag, so the skip predicate is now the entry's mere presence
/// rather than the obsolete `synthetic == true` discriminator.
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
        if lock.contains_key(name_str) {
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
/// present; walks `inst/cfg/*.md` for basic syntax validity (we just
/// read them to prove they're valid UTF-8 — the spec calls out "basic
/// markdown parse", not a full markdown lint). Missing files/dirs are
/// no-ops (not findings).
pub fn check_config_lint(workspace: &Path) -> CheckResult {
    let mut findings = Vec::new();
    check_openspec_config_yaml(workspace, &mut findings);
    check_inst_cfg_markdown(workspace, &mut findings);
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

/// `inst/cfg/*.md` half of [`check_config_lint`] — proves each file
/// is valid UTF-8. Absent dir is a no-op.
fn check_inst_cfg_markdown(workspace: &Path, findings: &mut Vec<Finding>) {
    let cfg_dir = workspace.join("inst").join("cfg");
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
            findings.push(config_lint_warning(format!("inst/cfg/{name} unreadable: {e}")));
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

/// v1.3.1 (B12) — advisory check: report packs whose on-disk content
/// is tracked by the parent meta-repo's git index. Pure read-only.
///
/// Behaviour:
/// * If `<meta_dir>` is not inside any git repo (no `.git/` walking up
///   the ancestors), no findings are emitted — the advisory is mute
///   when there is no parent to advise about.
/// * For each registered pack at `<meta_dir>/<state.path>`, run
///   `git -C <parent_repo> ls-files --error-unmatch <pack_rel_path>`.
///   A zero exit indicates the path is tracked → emit one
///   `ParentGitTracksPackContent` finding with `Severity::Ok` (advisory
///   only — does NOT change exit code).
/// * Per-pack git failures (binary missing, etc.) silently degrade to
///   "no finding for this pack" so the doctor walk completes.
///
/// The check runs against `packs` produced by [`crate::manifest::fold::fold`]; if
/// `packs` is `None` (manifest unreadable) the check is skipped — the
/// schema-error finding already informs the operator.
pub fn check_parent_git_tracks_pack_content(
    meta_dir: &Path,
    packs: Option<&HashMap<String, PackState>>,
) -> CheckResult {
    let Some(packs) = packs else {
        return CheckResult::default();
    };
    let Some(parent_repo) = find_parent_git_repo(meta_dir) else {
        return CheckResult::default();
    };
    let mut findings = Vec::new();
    let ordered: BTreeMap<_, _> = packs.iter().collect();
    for (id, state) in ordered {
        // Compute the pack path relative to the parent git repo root.
        let pack_abs = meta_dir.join(&state.path);
        let Ok(pack_rel) = pack_abs.strip_prefix(&parent_repo) else {
            continue;
        };
        let rel_str = pack_rel.to_string_lossy();
        if rel_str.is_empty() {
            continue;
        }
        if parent_git_path_tracked(&parent_repo, rel_str.as_ref()) {
            findings.push(Finding {
                check: CheckKind::ParentGitTracksPackContent,
                severity: Severity::Ok,
                pack: Some(id.clone()),
                detail: format!(
                    "advisory: pack `{id}` at `{rel_str}` is tracked by the parent meta-repo's git index. Add it to the meta-repo's `.gitignore` (and `git rm --cached` once) to clear this finding. grex never writes to the parent `.gitignore` automatically."
                ),
                auto_fixable: false,
                synthetic: false,
            });
        }
    }
    CheckResult { findings }
}

/// Walk parent directories of `start` looking for the nearest ancestor
/// that contains a `.git/` entry (directory or worktree gitlink file).
/// Returns the ancestor path, NOT the `.git/` itself. None if no parent
/// git repo is found.
///
/// Note: this deliberately walks STRICTLY upward starting from
/// `start.parent()` — a pack-managed meta-repo with its own `.git/` at
/// `meta_dir/.git/` is NOT the "parent" in the sense the advisory
/// cares about (the advisory is "the meta-repo above me tracks my
/// content", not "my own repo tracks my own content").
fn find_parent_git_repo(start: &Path) -> Option<PathBuf> {
    let mut cur = start.parent()?;
    loop {
        if cur.join(".git").exists() {
            return Some(cur.to_path_buf());
        }
        cur = cur.parent()?;
    }
}

/// Best-effort `git -C <repo> ls-files --error-unmatch <rel_path>`
/// probe. Returns `true` when the path is tracked, `false` otherwise
/// (untracked, ignored, missing git binary, etc.). Stderr is silenced
/// so the doctor output stays clean.
fn parent_git_path_tracked(repo: &Path, rel_path: &str) -> bool {
    use std::process::{Command, Stdio};
    let normalised = rel_path.replace('\\', "/");
    let status = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["ls-files", "--error-unmatch", "--"])
        .arg(&normalised)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    matches!(status, Ok(s) if s.success())
}

/// v1.3.3 (B5) — `.gitignore`-aware drift advisory.
///
/// For every pack registered under `meta_dir`, classify the pack
/// against the nearest parent git repo's `.gitignore`. A pack whose
/// path is NOT matched by any ignore rule is a "drift candidate" — the
/// operator most likely forgot to add the pack to `.gitignore`, which
/// risks the parent meta-repo accidentally tracking pack content.
///
/// Severity is `Severity::Ok` (warn-only): the finding is informational
/// and never affects the exit-code roll-up. All drift candidates are
/// aggregated into a SINGLE summary finding at the end of the check
/// rather than emitting per-pack rows, matching the design (Q3): "warn
/// only, summary prompt at end".
///
/// Skipped silently (no finding emitted) when:
/// * `packs` is `None` (manifest unreadable — already reported by the
///   schema check);
/// * no parent git repo is visible above `meta_dir` (no `.gitignore`
///   to advise about);
/// * `packs` is empty (nothing to drift).
///
/// Doctor never writes to the parent `.gitignore` — the advisory is
/// always a passive nudge.
pub fn check_gitignore_drift(
    meta_dir: &Path,
    packs: Option<&HashMap<String, PackState>>,
) -> CheckResult {
    let Some(packs) = packs else {
        return CheckResult::default();
    };
    if packs.is_empty() {
        return CheckResult::default();
    }
    let Some(parent_repo) = find_parent_git_repo(meta_dir) else {
        return CheckResult::default();
    };
    let gi_path = parent_repo.join(".gitignore");
    let rules = read_gitignore_rules(&gi_path);

    let mut drift: Vec<(String, String)> = Vec::new();
    let ordered: BTreeMap<_, _> = packs.iter().collect();
    for (id, state) in ordered {
        let pack_abs = meta_dir.join(&state.path);
        let Ok(pack_rel) = pack_abs.strip_prefix(&parent_repo) else {
            continue;
        };
        let rel_str = pack_rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            continue;
        }
        if !gitignore_covers(&rules, &rel_str) {
            drift.push((id.clone(), rel_str));
        }
    }

    if drift.is_empty() {
        return CheckResult::default();
    }

    let mut detail = format!(
        "{} pack(s) drift parent .gitignore tracking expectations:\n",
        drift.len()
    );
    for (id, rel) in &drift {
        detail.push_str(&format!(
            "  {rel}  — not-tracked (consider adding `{id}` to .gitignore)\n"
        ));
    }
    detail.push_str(&format!(
        "Action: review .gitignore at {} and add rules accordingly. grex never mutates the parent `.gitignore` automatically.",
        parent_repo.display()
    ));

    CheckResult::single(Finding {
        check: CheckKind::GitignoreDrift,
        severity: Severity::Ok,
        pack: None,
        detail,
        auto_fixable: false,
        synthetic: false,
    })
}

/// Read `.gitignore` rules from `path`. Returns an empty vec when the
/// file is absent or unreadable. Lines are trimmed; comments
/// (leading `#`) and empty lines are skipped. Negation (`!rule`) and
/// glob expansion are NOT modelled — this is a coarse string-prefix
/// matcher tuned for the common case of pack basenames in
/// `.gitignore`. False negatives (drift candidate emitted when the
/// rule actually covers the pack via a glob) are tolerated by design:
/// the advisory is warn-only.
fn read_gitignore_rules(path: &Path) -> Vec<String> {
    let raw = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| line.trim_end_matches('/').to_string())
        .collect()
}

/// Coarse-grained `.gitignore` coverage check. Returns `true` when any
/// rule matches the pack-relative path:
/// * exact equality (`<rule> == <rel>`);
/// * leading-slash anchored equality (`/<rule> == <rel>`);
/// * basename equality (`<rule> == <last-segment of rel>`) — matches
///   the unanchored `.gitignore` semantics where a bare name applies
///   anywhere in the tree;
/// * directory-prefix equality (`<rel>` starts with `<rule>/`) — a
///   broader rule swallowing the pack subtree.
///
/// Glob characters (`*`, `?`, `[`) make the rule conservatively
/// match-any: we treat the pack as covered to avoid noisy false
/// positives. Negation (`!`) rules are ignored — too rare in pack
/// `.gitignore` practice to be worth the complexity here.
fn gitignore_covers(rules: &[String], rel: &str) -> bool {
    let rel_norm = rel.trim_start_matches('/');
    let rel_basename = rel_norm.rsplit('/').next().unwrap_or(rel_norm);
    for rule in rules {
        if rule.starts_with('!') {
            continue;
        }
        // Conservative: any glob → assume coverage to keep noise low.
        if rule.contains('*') || rule.contains('?') || rule.contains('[') {
            return true;
        }
        let rule_stripped = rule.trim_start_matches('/');
        if rule_stripped.is_empty() {
            continue;
        }
        if rule_stripped == rel_norm || rule_stripped == rel_basename {
            return true;
        }
        let prefix = format!("{rule_stripped}/");
        if rel_norm.starts_with(&prefix) {
            return true;
        }
    }
    false
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
        let m = workspace.join(".grex/events.jsonl");
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
        let (r, evs) = check_manifest_schema(&d.path().join(".grex/events.jsonl"));
        assert_eq!(r.worst(), Severity::Ok);
        assert_eq!(evs.unwrap().len(), 1);
    }

    #[test]
    fn schema_corruption_is_error() {
        let d = tempdir().unwrap();
        // Line 1 is garbage (not last — there's a valid line 2), so
        // M3's reader flags it as Corruption.
        let m = d.path().join(".grex/events.jsonl");
        fs::create_dir_all(m.parent().unwrap()).unwrap();
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
        let (r, evs) = check_manifest_schema(&d.path().join(".grex/events.jsonl"));
        assert_eq!(r.worst(), Severity::Ok);
        assert!(evs.unwrap().is_empty());
    }

    // --- Unit: gitignore sync ---

    #[test]
    fn expected_patterns_for_pack_populates_builtin_defaults() {
        for pack_type in ["meta", "declarative", "scripted"] {
            let d = tempdir().unwrap();
            seed_pack_with_type(d.path(), pack_type, pack_type);
            let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
            let packs = manifest::fold(events);
            let state = packs.get(pack_type).unwrap();
            assert_eq!(
                expected_patterns_for_pack(d.path(), state),
                vec![".grex/".to_string()],
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
                    "schema_version: \"1\"\nname: {id}\ntype: {pack_type}\nx-gitignore:\n  - \".grex/\"\n  - {authored}\n",
                ),
            );
            let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
            let packs = manifest::fold(events);
            let state = packs.get(&id).unwrap();
            assert_eq!(
                expected_patterns_for_pack(d.path(), state),
                vec![".grex/".to_string(), authored],
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
        let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
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
        let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
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
        upsert_managed_block(&d.path().join(".gitignore"), "a", &[".grex/", "target/", "*.log"])
            .unwrap();
        let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
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
        let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs, &HashMap::new());
        assert_eq!(r.worst(), Severity::Error);
    }

    #[test]
    fn on_disk_unregistered_dir_is_warning() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        fs::create_dir_all(d.path().join("stranger")).unwrap();
        let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
        let packs = manifest::fold(events);
        let r = check_on_disk_drift(d.path(), &packs, &HashMap::new());
        assert_eq!(r.worst(), Severity::Warning);
    }

    #[test]
    fn on_disk_clean_workspace_is_ok() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        let events = manifest::read_all(&d.path().join(".grex/events.jsonl")).unwrap();
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
        let opts = DoctorOpts { fix: true, lint_config: false, ..DoctorOpts::default() };
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
        let m = d.path().join(".grex/events.jsonl");
        fs::create_dir_all(m.parent().unwrap()).unwrap();
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

        let opts = DoctorOpts { fix: true, lint_config: false, ..DoctorOpts::default() };
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
        // `.grex/events.jsonl`, not a stray `.gitignore`, nothing. A recursive
        // path+bytes snapshot catches any such write, not just the
        // presence/absence of the missing pack dir.
        let before = fs_snapshot(d.path());

        let opts = DoctorOpts { fix: true, lint_config: false, ..DoctorOpts::default() };
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
        let opts = DoctorOpts { fix: false, lint_config: true, ..DoctorOpts::default() };
        let report = run_doctor(d.path(), &opts).unwrap();
        assert_eq!(report.exit_code(), 1);
        assert!(report.findings.iter().any(|f| f.check == CheckKind::ConfigLint));
    }

    // --- v1.1.1: synthetic plain-git children ---

    /// A workspace whose lockfile carries a `synthetic: true` entry for
    /// pack `a` reports `OK (synthetic)` for it, exits 0, and never
    /// emits a missing-manifest finding even though no `.grex/pack.yaml`
    /// exists on disk for that pack.
    ///
    /// v1.3.2 W1 retired the writer side of `LockEntry.synthetic` (the
    /// `skip_serializing_if` predicate now always omits the field), so
    /// this test seeds a legacy v1.1.x-shaped JSONL line directly via
    /// `fs::write` rather than going through `write_lockfile`. The reader
    /// honours `synthetic: true` via `#[serde(default)]`, exercising the
    /// preserved legacy-carryover branch in `check_synthetic_packs`.
    #[test]
    fn run_doctor_synthetic_pack_reports_ok_synthetic_and_exits_zero() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();

        // Hand-write a v1.1.x-shaped lockfile line with `synthetic: true`
        // for pack `a`. `write_lockfile` strips the field on emit, so the
        // raw write is required to pin the legacy carryover branch.
        let lock_dir = d.path().join(".grex");
        fs::create_dir_all(&lock_dir).unwrap();
        let lock_path = lock_dir.join("grex.lock.jsonl");
        fs::write(
            &lock_path,
            br#"{"id":"a","path":"a","sha":"deadbeef","branch":"main","installed_at":"2026-04-22T10:00:00Z","actions_hash":"","schema_version":"1","synthetic":true}
"#,
        )
        .unwrap();

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

    // --- v1.2.5: --restore-quarantine TS:BASENAME input validation ---

    /// Operator pasted a raw ISO-8601 timestamp containing colons into
    /// the `--restore-quarantine TS:BASENAME` slot. The CLI splits on
    /// the FIRST colon, so `ts` reaches doctor still carrying colons.
    /// Doctor must short-circuit with a `QuarantineRestore` Error
    /// finding whose detail names the foot-gun rather than dispatching
    /// to `restore_quarantine` (which would surface a confusing
    /// `SnapshotNotFound`).
    #[test]
    fn run_doctor_restore_quarantine_rejects_colon_in_timestamp() {
        let d = tempdir().unwrap();
        seed_pack(d.path(), "a");
        upsert_managed_block(
            &d.path().join(".gitignore"),
            "a",
            default_managed_gitignore_patterns(),
        )
        .unwrap();

        let opts = DoctorOpts {
            restore_quarantine: Some(("2026-05-02T10:30:00Z".into(), Some("pack-a".into()))),
            ..DoctorOpts::default()
        };
        let report = run_doctor(d.path(), &opts).unwrap();

        let restore_findings: Vec<_> =
            report.findings.iter().filter(|f| f.check == CheckKind::QuarantineRestore).collect();
        assert_eq!(restore_findings.len(), 1, "exactly one restore finding expected");
        let f = restore_findings[0];
        assert_eq!(f.severity, Severity::Error);
        assert!(
            f.detail.contains("malformed") && f.detail.contains("FIRST colon"),
            "detail must explain the colon foot-gun: {}",
            f.detail,
        );
        assert_eq!(report.exit_code(), 2, "Error severity rolls up to exit 2");
    }

    // --- v1.2.0 Stage 1.j: recursive ManifestTree walk + --shallow ---

    /// Build a meta directory at `meta_dir` with a `pack.yaml` declaring
    /// `children`. Each child is `(segment, url)`. The pack type is
    /// `meta` so the manifest is shaped like a workspace orchestrator.
    fn write_meta_manifest(meta_dir: &Path, name: &str, children: &[(&str, &str)]) {
        let grex_dir = meta_dir.join(".grex");
        fs::create_dir_all(&grex_dir).unwrap();
        let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\n");
        if !children.is_empty() {
            yaml.push_str("children:\n");
            for (segment, url) in children {
                yaml.push_str(&format!("  - url: {url}\n    path: {segment}\n"));
            }
        }
        fs::write(grex_dir.join("pack.yaml"), yaml).unwrap();
    }

    /// Build a leaf meta whose own `events.jsonl` registers one pack
    /// `pack_id` at sub-path `pack_id` so the per-meta on-disk-drift
    /// check sees a clean pack. Does NOT touch the parent.
    fn seed_meta_with_pack(meta_dir: &Path, meta_name: &str, pack_id: &str) {
        write_meta_manifest(meta_dir, meta_name, &[]);
        let m = meta_dir.join(".grex").join("events.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: pack_id.into(),
                url: format!("https://example/{pack_id}"),
                path: pack_id.into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        fs::create_dir_all(meta_dir.join(pack_id)).unwrap();
    }

    /// AC: by default, doctor walks every nested meta. A 3-level tree
    /// (root → alpha → gamma) yields one ManifestSchema finding per
    /// meta (3 total).
    #[test]
    fn test_doctor_recurses_default() {
        let d = tempdir().unwrap();
        let root = d.path();

        // Root meta declares child `alpha`.
        write_meta_manifest(root, "root", &[("alpha", "https://example.invalid/alpha.git")]);
        // Root's events.jsonl registers `alpha` so on-disk-drift is clean.
        let m = root.join(".grex").join("events.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: "alpha".into(),
                url: "https://example.invalid/alpha.git".into(),
                path: "alpha".into(),
                pack_type: "meta".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();

        // Alpha meta declares child `gamma`.
        let alpha = root.join("alpha");
        write_meta_manifest(&alpha, "alpha", &[("gamma", "https://example.invalid/gamma.git")]);
        let am = alpha.join(".grex").join("events.jsonl");
        append_event(
            &am,
            &Event::Add {
                ts: ts(),
                id: "gamma".into(),
                url: "https://example.invalid/gamma.git".into(),
                path: "gamma".into(),
                pack_type: "declarative".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        fs::create_dir_all(alpha.join("gamma")).unwrap();

        // Gamma is a leaf meta with one registered pack `delta`.
        let gamma = alpha.join("gamma");
        seed_meta_with_pack(&gamma, "gamma", "delta");

        let report = run_doctor(root, &DoctorOpts::default()).unwrap();

        let schema_oks: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.check == CheckKind::ManifestSchema && f.severity == Severity::Ok)
            .collect();
        assert_eq!(
            schema_oks.len(),
            3,
            "three metas visited (root + alpha + gamma); got: {:?}",
            report.findings,
        );
    }

    /// AC: `--shallow 0` halts at the root meta — only one
    /// ManifestSchema finding even when the root has nested metas.
    #[test]
    fn test_doctor_shallow_zero_root_only() {
        let d = tempdir().unwrap();
        let root = d.path();

        write_meta_manifest(root, "root", &[("alpha", "https://example.invalid/alpha.git")]);
        let m = root.join(".grex").join("events.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: "alpha".into(),
                url: "https://example.invalid/alpha.git".into(),
                path: "alpha".into(),
                pack_type: "meta".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        let alpha = root.join("alpha");
        seed_meta_with_pack(&alpha, "alpha", "leaf");

        let opts = DoctorOpts { shallow: Some(0), ..DoctorOpts::default() };
        let report = run_doctor(root, &opts).unwrap();
        let schema_oks: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.check == CheckKind::ManifestSchema && f.severity == Severity::Ok)
            .collect();
        assert_eq!(schema_oks.len(), 1, "shallow=0 must halt at root; got: {:?}", report.findings,);
    }

    /// AC: `--shallow 1` visits root + depth-1 metas but not deeper.
    /// A 3-level tree (root → alpha → gamma) yields 2 ManifestSchema
    /// findings (root + alpha) under shallow=1.
    #[test]
    fn test_doctor_shallow_n_stops_at_n() {
        let d = tempdir().unwrap();
        let root = d.path();

        write_meta_manifest(root, "root", &[("alpha", "https://example.invalid/alpha.git")]);
        let m = root.join(".grex").join("events.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: "alpha".into(),
                url: "https://example.invalid/alpha.git".into(),
                path: "alpha".into(),
                pack_type: "meta".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();

        let alpha = root.join("alpha");
        write_meta_manifest(&alpha, "alpha", &[("gamma", "https://example.invalid/gamma.git")]);
        let am = alpha.join(".grex").join("events.jsonl");
        append_event(
            &am,
            &Event::Add {
                ts: ts(),
                id: "gamma".into(),
                url: "https://example.invalid/gamma.git".into(),
                path: "gamma".into(),
                pack_type: "meta".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();
        fs::create_dir_all(alpha.join("gamma")).unwrap();

        let gamma = alpha.join("gamma");
        seed_meta_with_pack(&gamma, "gamma", "delta");

        let opts = DoctorOpts { shallow: Some(1), ..DoctorOpts::default() };
        let report = run_doctor(root, &opts).unwrap();
        let schema_oks: Vec<_> = report
            .findings
            .iter()
            .filter(|f| f.check == CheckKind::ManifestSchema && f.severity == Severity::Ok)
            .collect();
        assert_eq!(
            schema_oks.len(),
            2,
            "shallow=1 must visit root + depth-1; got: {:?}",
            report.findings,
        );
    }

    /// AC: doctor performs zero filesystem mutations on a multi-level
    /// tree even when sub-meta gitignores have drift. Read-only by
    /// contract — only the root frame's `--fix` is allowed to write.
    /// Here `--fix` is OFF, so every byte of the fixture is preserved.
    #[test]
    fn test_doctor_no_fs_mutations() {
        let d = tempdir().unwrap();
        let root = d.path();

        // Root meta with declared child `alpha`.
        write_meta_manifest(root, "root", &[("alpha", "https://example.invalid/alpha.git")]);
        let m = root.join(".grex").join("events.jsonl");
        append_event(
            &m,
            &Event::Add {
                ts: ts(),
                id: "alpha".into(),
                url: "https://example.invalid/alpha.git".into(),
                path: "alpha".into(),
                pack_type: "meta".into(),
                schema_version: SCHEMA_VERSION.into(),
            },
        )
        .unwrap();

        // Alpha meta carries DRIFT in its own .gitignore — its managed
        // block body deviates from the expected list. The recursive
        // walk must observe it (Warning finding) but mutate nothing.
        let alpha = root.join("alpha");
        seed_meta_with_pack(&alpha, "alpha", "leaf");
        upsert_managed_block(&alpha.join(".gitignore"), "leaf", &["drifted-pattern"]).unwrap();

        let before = fs_snapshot(root);
        let report = run_doctor(root, &DoctorOpts::default()).unwrap();
        let after = fs_snapshot(root);

        assert_eq!(before, after, "recursive doctor walk must perform zero writes");
        // Sanity: the sub-meta drift was actually observed (so the
        // mutation-check isn't passing trivially because the walker
        // didn't recurse).
        assert!(
            report
                .findings
                .iter()
                .any(|f| f.check == CheckKind::GitignoreSync && f.severity == Severity::Warning),
            "expected sub-meta gitignore-drift warning; got: {:?}",
            report.findings,
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

    // --- v1.3.3 (B5) — `check_gitignore_drift` ---

    /// Build a minimal `PackState` for drift tests. The check only
    /// reads `state.path`, so other fields are irrelevant placeholder
    /// values.
    fn drift_pack(id: &str, path: &str) -> (String, PackState) {
        (
            id.to_string(),
            PackState {
                id: id.to_string(),
                url: format!("https://example/{id}"),
                path: path.to_string(),
                pack_type: "declarative".to_string(),
                ref_spec: None,
                last_sync_sha: None,
                added_at: ts(),
                updated_at: ts(),
            },
        )
    }

    /// Build a parent git repo with a workspace meta directory inside.
    /// Returns `(parent_repo, workspace)`. Caller writes
    /// `<parent>/.gitignore` themselves.
    fn drift_fixture() -> (tempfile::TempDir, PathBuf) {
        let parent = tempdir().unwrap();
        fs::create_dir_all(parent.path().join(".git")).unwrap();
        fs::write(parent.path().join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
        let ws = parent.path().join("ws");
        fs::create_dir_all(&ws).unwrap();
        (parent, ws)
    }

    #[test]
    fn gitignore_drift_emits_finding_for_uncovered_pack() {
        // One pack tracked by .gitignore, one NOT tracked → expect a
        // single summary finding listing only the uncovered pack.
        let (parent, ws) = drift_fixture();
        fs::write(parent.path().join(".gitignore"), "ws/alpha\n").unwrap();
        let mut packs: HashMap<String, PackState> = HashMap::new();
        let (a_id, a) = drift_pack("alpha", "alpha");
        let (b_id, b) = drift_pack("beta", "beta");
        packs.insert(a_id, a);
        packs.insert(b_id, b);

        let r = check_gitignore_drift(&ws, Some(&packs));
        assert_eq!(r.findings.len(), 1, "summary finding only; got: {:?}", r.findings);
        let f = &r.findings[0];
        assert_eq!(f.check, CheckKind::GitignoreDrift);
        assert_eq!(f.severity, Severity::Ok, "warn-only — must not affect exit code");
        assert!(f.detail.contains("beta"), "detail must name uncovered pack: {}", f.detail);
        assert!(!f.detail.contains("ws/alpha  — not-tracked"), "covered pack must not appear: {}", f.detail);
    }

    #[test]
    fn gitignore_drift_empty_gitignore_means_all_packs_drift() {
        // Empty .gitignore covers nothing → every pack is drift.
        let (parent, ws) = drift_fixture();
        fs::write(parent.path().join(".gitignore"), "").unwrap();
        let mut packs: HashMap<String, PackState> = HashMap::new();
        for id in ["alpha", "beta"] {
            let (k, v) = drift_pack(id, id);
            packs.insert(k, v);
        }

        let r = check_gitignore_drift(&ws, Some(&packs));
        assert_eq!(r.findings.len(), 1);
        let detail = &r.findings[0].detail;
        assert!(detail.contains("alpha"), "alpha must be flagged: {detail}");
        assert!(detail.contains("beta"), "beta must be flagged: {detail}");
        assert!(detail.starts_with("2 pack(s) drift"), "summary count: {detail}");
    }

    #[test]
    fn gitignore_drift_all_covered_emits_no_finding() {
        // Every pack covered → no summary finding.
        let (parent, ws) = drift_fixture();
        fs::write(parent.path().join(".gitignore"), "ws/alpha\nws/beta\n").unwrap();
        let mut packs: HashMap<String, PackState> = HashMap::new();
        for id in ["alpha", "beta"] {
            let (k, v) = drift_pack(id, id);
            packs.insert(k, v);
        }

        let r = check_gitignore_drift(&ws, Some(&packs));
        assert!(r.findings.is_empty(), "no drift → no finding; got: {:?}", r.findings);
    }

    #[test]
    fn gitignore_drift_no_packs_no_finding() {
        // No packs registered → nothing to drift.
        let (_parent, ws) = drift_fixture();
        let packs: HashMap<String, PackState> = HashMap::new();
        let r = check_gitignore_drift(&ws, Some(&packs));
        assert!(r.findings.is_empty());

        // None packs (manifest unreadable) → silent skip.
        let r2 = check_gitignore_drift(&ws, None);
        assert!(r2.findings.is_empty());
    }

    #[test]
    fn gitignore_drift_no_parent_repo_silent_skip() {
        // No parent git repo above meta_dir → nothing to advise.
        let d = tempdir().unwrap();
        let mut packs: HashMap<String, PackState> = HashMap::new();
        let (k, v) = drift_pack("alpha", "alpha");
        packs.insert(k, v);
        let r = check_gitignore_drift(d.path(), Some(&packs));
        assert!(r.findings.is_empty(), "no parent repo → silent; got: {:?}", r.findings);
    }

    #[test]
    fn gitignore_drift_basename_rule_covers_pack() {
        // Bare name `alpha` in .gitignore (unanchored) covers
        // `ws/alpha` per `.gitignore` basename semantics.
        let (parent, ws) = drift_fixture();
        fs::write(parent.path().join(".gitignore"), "alpha\n").unwrap();
        let mut packs: HashMap<String, PackState> = HashMap::new();
        let (k, v) = drift_pack("alpha", "alpha");
        packs.insert(k, v);
        let r = check_gitignore_drift(&ws, Some(&packs));
        assert!(r.findings.is_empty(), "basename rule covers pack; got: {:?}", r.findings);
    }
}
