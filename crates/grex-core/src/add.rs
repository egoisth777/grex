//! Shared pack-registration helper used by `grex add` and import.

use crate::manifest::{self, Event, PackId, SCHEMA_VERSION};
use crate::pack::yaml_writer::{
    append_child_to_pack_yaml, AppendOutcome, ChildEntry, PackYamlWriteError,
};
use chrono::Utc;
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Add request after CLI / MCP edge parsing has resolved defaults.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub url: String,
    pub path: String,
    pub pack_type: String,
    /// v1.3.3 B10 — parsed `--ref <git-ref>` token. `None` when the
    /// caller did not pass `--ref` (defaults to remote `main` HEAD per
    /// the 8-cell folder-FA cell 1).
    pub git_ref: Option<crate::refspec::Ref>,
}

impl AddRequest {
    pub fn new(
        url: impl Into<String>,
        path: impl Into<String>,
        pack_type: impl Into<String>,
    ) -> Self {
        Self { url: url.into(), path: path.into(), pack_type: pack_type.into(), git_ref: None }
    }

    /// v1.3.3 B10 — attach a parsed [`crate::refspec::Ref`] to the
    /// request. Builder-style for ergonomic CLI / MCP construction.
    pub fn with_ref(mut self, r: crate::refspec::Ref) -> Self {
        self.git_ref = Some(r);
        self
    }
}

/// Runtime options for add dispatch.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AddOpts {
    pub dry_run: bool,
}

impl AddOpts {
    pub fn new(dry_run: bool) -> Self {
        Self { dry_run }
    }
}

/// Result of an add dispatch.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddReport {
    pub id: PackId,
    pub url: String,
    pub path: String,
    pub pack_type: String,
    pub dry_run: bool,
    pub appended: bool,
    /// v1.3.3 B10 — resolved `<refdir>` folder name (post collision-
    /// extend) when the request carried a `--ref` token. `None` when
    /// no `--ref` flag was passed.
    pub refdir: Option<String>,
    /// v1.3.3 B10 — FA action tag classifying this add against the
    /// 8-cell transition table. `None` when no `--ref` flag was
    /// passed (current call sites still use the legacy single-checkout
    /// path).
    pub ref_action: Option<crate::refspec::RefAction>,
    /// v1.4.1 — whether the pack.yaml `children:` sequence gained a
    /// new row for this add. `false` for dry-run, reject-class adds,
    /// and adds that found the path already in the sequence (idempotent
    /// re-registration). v1.4.0 left this entirely unwritten, which
    /// caused `grex sync` / `ls` / `status` to miss freshly-added packs.
    pub pack_yaml_updated: bool,
}

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum AddError {
    #[error("manifest write failed: {0}")]
    Manifest(#[from] manifest::ManifestError),
    #[error("pack.yaml write failed: {0}")]
    PackYaml(#[from] PackYamlWriteError),
}

/// Probe the parent manifest at `manifest_path` to seed the dynamic
/// FA axes for a candidate `(url, refdir)` pair. Reads existing
/// `Event::Add` records and returns:
///
/// * `url_tracked` — any prior `Add` carries the same `url` as the
///   candidate request.
/// * `dup_hit` — any prior `Add` carries BOTH the same `url` AND the
///   same `path` (= encoded `<refdir>`) as the candidate. v1.3.3 stores
///   the refdir in the `path` field of the manifest `Add` event because
///   the schema does not yet carry a separate `(branch, commit)` tuple
///   per OQ4-deferred work; matching on `path` is sufficient for
///   FA-driven dup detection because [`crate::refspec::encode_refdir`]
///   is injective up to collision-extend (OQ5: `encodeRefdir_distinct`).
///
/// Returns `AddContext::default()` (both axes false) when the manifest
/// file does not yet exist on disk — fresh-repo case for cell 1.
fn probe_manifest_for_ref(
    manifest_path: &Path,
    candidate_url: &str,
    candidate_refdir: &str,
) -> crate::refspec::AddContext {
    let events = match manifest::read_all(manifest_path) {
        Ok(ev) => ev,
        // Treat missing/unreadable manifest as "fresh repo" — both
        // axes false. Errors here surface elsewhere (caller's append
        // path will fail with the same error if the manifest is
        // genuinely broken).
        Err(_) => return crate::refspec::AddContext::default(),
    };
    let mut url_tracked = false;
    let mut dup_hit = false;
    for ev in &events {
        if let Event::Add { url, path, .. } = ev {
            if url == candidate_url {
                url_tracked = true;
                if path == candidate_refdir {
                    dup_hit = true;
                    break;
                }
            }
        }
    }
    crate::refspec::AddContext { url_tracked, dup_hit }
}

/// Append the manifest event for a pack registration unless this is a dry-run.
pub fn add_pack(
    manifest_path: &Path,
    request: AddRequest,
    opts: AddOpts,
) -> Result<AddReport, AddError> {
    let id = PackId::from(request.path.clone());
    // v1.3.3 B10 — encode `<refdir>` from the parsed ref then probe
    // the parent manifest for the (`url_tracked`, `dup_hit`) axes
    // before driving the 8-cell FA. Reject-class actions (cells 2,
    // 4-dup, 6-dup, 8-dup) MUST be no-ops per Lean `dup_safe`: no
    // manifest event appended, no folder created. Warn-reject also
    // surfaces via [`AddReport::ref_action`] so the caller can render
    // a stderr warning + exit 1.
    let (refdir, ref_action) = match &request.git_ref {
        Some(r) => {
            let dir = crate::refspec::encode_refdir(r);
            let ctx = probe_manifest_for_ref(manifest_path, &request.url, &dir);
            let action = crate::refspec::classify_ref_input(r, &ctx);
            (Some(dir), Some(action))
        }
        None => (None, None),
    };

    // Gate manifest mutation on the FA action. Reject-class is a
    // no-op (idempotent re-add); Add-class appends the event.
    let is_reject = matches!(ref_action, Some(a) if a.is_reject());
    let appended = !opts.dry_run && !is_reject;
    let mut pack_yaml_updated = false;
    if appended {
        let ev = Event::Add {
            ts: Utc::now(),
            id: id.clone(),
            url: request.url.clone(),
            path: request.path.clone(),
            pack_type: request.pack_type.clone(),
            schema_version: SCHEMA_VERSION.to_string(),
        };
        manifest::append_event(manifest_path, &ev)?;

        // v1.4.1 — bridge the event log to the declarative manifest.
        // `grex sync` / `ls` / `status` walk `pack.yaml.children`, so
        // without this step a freshly-added pack stays invisible (the
        // exact v1.4.0 bug the smoke test surfaced).
        let pack_yaml_path = pack_yaml_path_for(manifest_path);
        let git_ref_label = request.git_ref.as_ref().map(format_git_ref);
        let outcome = append_child_to_pack_yaml(
            &pack_yaml_path,
            &ChildEntry {
                url: request.url.clone(),
                path: request.path.clone(),
                git_ref: git_ref_label,
            },
        )?;
        pack_yaml_updated = matches!(outcome, AppendOutcome::Appended);
    }

    Ok(AddReport {
        id,
        url: request.url,
        path: request.path,
        pack_type: request.pack_type,
        dry_run: opts.dry_run,
        appended,
        refdir,
        ref_action,
        pack_yaml_updated,
    })
}

/// Derive the `pack.yaml` path from a manifest (events.jsonl) path.
///
/// `<workspace>/.grex/events.jsonl` → `<workspace>/.grex/pack.yaml`.
/// Falls back to a sibling `pack.yaml` when the manifest path has no
/// parent.
pub(crate) fn pack_yaml_path_for(manifest_path: &Path) -> PathBuf {
    manifest_path
        .parent()
        .map(|p| p.join("pack.yaml"))
        .unwrap_or_else(|| PathBuf::from("pack.yaml"))
}

/// Render a [`crate::refspec::Ref`] back into the spelling we want to
/// land in `pack.yaml`'s `ref:` field. Mirrors the parser's accepted
/// shapes: `<branch>`, `<commit>`, or `<branch>@<commit>`. Returns an
/// empty string only for the (currently impossible) all-none case;
/// caller skips emitting `ref:` when input is `None`.
fn format_git_ref(r: &crate::refspec::Ref) -> String {
    match (&r.branch, &r.commit) {
        (Some(b), Some(c)) => format!("{b}@{c}"),
        (Some(b), None) => b.clone(),
        (None, Some(c)) => c.clone(),
        (None, None) => String::new(),
    }
}

/// Infer the default workspace-relative path from a repository URL.
pub fn infer_path_from_url(url: &str) -> String {
    let trimmed = url.trim_end_matches(['/', '\\']);
    let tail = trimmed.rsplit(['/', '\\', ':']).next().unwrap_or(trimmed);
    tail.strip_suffix(".git").unwrap_or(tail).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_pack_appends_add_event() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let report = add_pack(
            &manifest,
            AddRequest::new("https://example.com/repo.git", "repo", "scripted"),
            AddOpts { dry_run: false },
        )
        .unwrap();

        assert!(report.appended);
        assert_eq!(report.id, "repo");
        assert!(report.refdir.is_none());
        assert!(report.ref_action.is_none());
        let events = manifest::read_all(&manifest).unwrap();
        assert_eq!(events.len(), 1);
        match &events[0] {
            Event::Add { id, url, path, pack_type, schema_version, .. } => {
                assert_eq!(id, "repo");
                assert_eq!(url, "https://example.com/repo.git");
                assert_eq!(path, "repo");
                assert_eq!(pack_type, "scripted");
                assert_eq!(schema_version, SCHEMA_VERSION);
            }
            _ => panic!("expected add event"),
        }
    }

    #[test]
    fn add_pack_dry_run_does_not_write_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let report = add_pack(
            &manifest,
            AddRequest::new("", "local", "declarative"),
            AddOpts { dry_run: true },
        )
        .unwrap();

        assert!(!report.appended);
        assert!(!manifest.exists());
    }

    // ----- v1.3.3 B10 — `--ref` integration --------------------------

    #[test]
    fn b10_add_pack_with_ref_emits_refdir_and_action() {
        // Fresh repo (url_tracked = false, dup_hit = false) → cell 1/3/5/7
        // → Add. `<refdir>` is the encoded folder name.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let r = crate::refspec::Ref {
            branch: Some("main".into()),
            commit: Some("a3f9c1d2b8e7".into()),
        };
        let report = add_pack(
            &manifest,
            AddRequest::new("https://example.com/repo.git", "repo", "scripted").with_ref(r),
            AddOpts { dry_run: false },
        )
        .unwrap();

        assert_eq!(report.refdir.as_deref(), Some("main@a3f9c1d"));
        assert_eq!(report.ref_action, Some(crate::refspec::RefAction::Add));
    }

    #[test]
    fn b10_add_same_ref_twice_is_idempotent() {
        // First call seeds the manifest with (url, path=refdir). Second
        // call with the same `--ref` MUST classify as a Reject-class
        // action (cell 8-dup: B=1 C=1 U=1 dup) and produce no new
        // manifest event.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let url = "https://example.com/repo.git";
        let r = crate::refspec::Ref {
            branch: Some("main".into()),
            commit: Some("a3f9c1d2b8e7".into()),
        };
        let refdir = crate::refspec::encode_refdir(&r);

        // First call — fresh repo, Add fires.
        let r1 = add_pack(
            &manifest,
            AddRequest::new(url, refdir.clone(), "scripted").with_ref(r.clone()),
            AddOpts { dry_run: false },
        )
        .unwrap();
        assert!(r1.appended);
        assert_eq!(r1.ref_action, Some(crate::refspec::RefAction::Add));

        // Second call — manifest now carries (url, path=refdir).
        // dup_hit fires; cell 8-dup → WarnReject → no-op.
        let r2 = add_pack(
            &manifest,
            AddRequest::new(url, refdir.clone(), "scripted").with_ref(r),
            AddOpts { dry_run: false },
        )
        .unwrap();
        assert!(!r2.appended, "reject-class must not append manifest event");
        assert_eq!(r2.ref_action, Some(crate::refspec::RefAction::WarnReject));
        assert_eq!(r2.refdir.as_deref(), Some(refdir.as_str()));

        // Manifest must contain exactly one Add event after the dup
        // attempt.
        let events = manifest::read_all(&manifest).unwrap();
        let add_count = events.iter().filter(|e| matches!(e, Event::Add { .. })).count();
        assert_eq!(add_count, 1, "only the first add should have been appended");
    }

    #[test]
    fn b10_add_different_refs_independent() {
        // Two different refs against the same URL: first is fresh
        // (cell 7 → Add), second is url_tracked but distinct path
        // (cell 8 no-dup → WarnAdd). Both append.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let url = "https://example.com/repo.git";
        let r1 = crate::refspec::Ref {
            branch: Some("main".into()),
            commit: Some("a3f9c1d2b8e7".into()),
        };
        let r2 = crate::refspec::Ref {
            branch: Some("develop".into()),
            commit: Some("b4e8d2a1c9f6".into()),
        };
        let dir1 = crate::refspec::encode_refdir(&r1);
        let dir2 = crate::refspec::encode_refdir(&r2);

        let rep1 = add_pack(
            &manifest,
            AddRequest::new(url, dir1.clone(), "scripted").with_ref(r1),
            AddOpts { dry_run: false },
        )
        .unwrap();
        assert!(rep1.appended);
        assert_eq!(rep1.ref_action, Some(crate::refspec::RefAction::Add));

        let rep2 = add_pack(
            &manifest,
            AddRequest::new(url, dir2.clone(), "scripted").with_ref(r2),
            AddOpts { dry_run: false },
        )
        .unwrap();
        assert!(rep2.appended, "distinct refdir must not be reject-class");
        assert_eq!(rep2.ref_action, Some(crate::refspec::RefAction::WarnAdd));
        assert_ne!(dir1, dir2);

        let events = manifest::read_all(&manifest).unwrap();
        let add_count = events.iter().filter(|e| matches!(e, Event::Add { .. })).count();
        assert_eq!(add_count, 2);
    }

    #[test]
    fn b10_warn_reject_emits_warn_action_no_append() {
        // Bare-commit dup (cell 6-dup) must classify as WarnReject and
        // skip the manifest append.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let url = "https://example.com/repo.git";
        let r = crate::refspec::Ref { branch: None, commit: Some("a3f9c1d2b8e7".into()) };
        let refdir = crate::refspec::encode_refdir(&r);

        let _ = add_pack(
            &manifest,
            AddRequest::new(url, refdir.clone(), "scripted").with_ref(r.clone()),
            AddOpts { dry_run: false },
        )
        .unwrap();
        let rep = add_pack(
            &manifest,
            AddRequest::new(url, refdir.clone(), "scripted").with_ref(r),
            AddOpts { dry_run: false },
        )
        .unwrap();

        assert_eq!(rep.ref_action, Some(crate::refspec::RefAction::WarnReject));
        assert!(!rep.appended);
        assert!(rep.ref_action.unwrap().warns());
    }

    #[test]
    fn b10_silent_reject_default_branch_dup_no_append() {
        // Cell 2: B=0 C=0 U=1 → SilentReject. Re-adding the same URL
        // with no ref token after a prior bare-default-branch add must
        // be a silent no-op.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let url = "https://example.com/repo.git";
        let r = crate::refspec::Ref { branch: None, commit: None };
        let refdir = crate::refspec::encode_refdir(&r); // "main"

        let rep1 = add_pack(
            &manifest,
            AddRequest::new(url, refdir.clone(), "scripted").with_ref(r.clone()),
            AddOpts { dry_run: false },
        )
        .unwrap();
        assert!(rep1.appended);

        let rep2 = add_pack(
            &manifest,
            AddRequest::new(url, refdir.clone(), "scripted").with_ref(r),
            AddOpts { dry_run: false },
        )
        .unwrap();
        assert_eq!(rep2.ref_action, Some(crate::refspec::RefAction::SilentReject));
        assert!(!rep2.appended);
        assert!(!rep2.ref_action.unwrap().warns());
    }

    #[test]
    fn b10_add_pack_without_ref_leaves_refdir_none() {
        // Backward-compat: legacy `add` without `--ref` keeps `refdir`
        // and `ref_action` as None.
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join(".grex/events.jsonl");
        let report = add_pack(
            &manifest,
            AddRequest::new("https://example.com/repo.git", "repo", "scripted"),
            AddOpts { dry_run: false },
        )
        .unwrap();

        assert!(report.refdir.is_none());
        assert!(report.ref_action.is_none());
    }

    #[test]
    fn infer_path_from_https_git_url() {
        assert_eq!(infer_path_from_url("https://example.com/org/repo.git"), "repo");
    }

    #[test]
    fn infer_path_from_scp_like_url() {
        assert_eq!(infer_path_from_url("git@example.com:org/repo.git"), "repo");
    }

    #[test]
    fn infer_path_from_trailing_slash() {
        assert_eq!(infer_path_from_url("https://example.com/org/repo/"), "repo");
    }

    #[test]
    fn infer_path_from_empty_url_is_empty() {
        assert_eq!(infer_path_from_url(""), "");
    }
}
