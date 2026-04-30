//! Shared pack-registration helper used by `grex add` and import.

use crate::manifest::{self, Event, PackId, SCHEMA_VERSION};
use chrono::Utc;
use std::path::Path;
use thiserror::Error;

/// Add request after CLI / MCP edge parsing has resolved defaults.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub url: String,
    pub path: String,
    pub pack_type: String,
}

impl AddRequest {
    pub fn new(
        url: impl Into<String>,
        path: impl Into<String>,
        pack_type: impl Into<String>,
    ) -> Self {
        Self { url: url.into(), path: path.into(), pack_type: pack_type.into() }
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
}

#[non_exhaustive]
#[derive(Debug, Error)]
pub enum AddError {
    #[error("manifest write failed: {0}")]
    Manifest(#[from] manifest::ManifestError),
}

/// Append the manifest event for a pack registration unless this is a dry-run.
pub fn add_pack(
    manifest_path: &Path,
    request: AddRequest,
    opts: AddOpts,
) -> Result<AddReport, AddError> {
    let id = PackId::from(request.path.clone());
    if !opts.dry_run {
        let ev = Event::Add {
            ts: Utc::now(),
            id: id.clone(),
            url: request.url.clone(),
            path: request.path.clone(),
            pack_type: request.pack_type.clone(),
            schema_version: SCHEMA_VERSION.to_string(),
        };
        manifest::append_event(manifest_path, &ev)?;
    }

    Ok(AddReport {
        id,
        url: request.url,
        path: request.path,
        pack_type: request.pack_type,
        dry_run: opts.dry_run,
        appended: !opts.dry_run,
    })
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
            AddRequest {
                url: "https://example.com/repo.git".into(),
                path: "repo".into(),
                pack_type: "scripted".into(),
            },
            AddOpts { dry_run: false },
        )
        .unwrap();

        assert!(report.appended);
        assert_eq!(report.id, "repo");
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
            AddRequest { url: "".into(), path: "local".into(), pack_type: "declarative".into() },
            AddOpts { dry_run: true },
        )
        .unwrap();

        assert!(!report.appended);
        assert!(!manifest.exists());
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
