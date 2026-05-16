//! Offline fixture seeding for the real-smoke harness.
//!
//! The online tier (`worktree::WorktreeGuard`) clones pre-provisioned
//! `egoisth777/*` fixtures from GitHub. That tier costs network and
//! requires an outbound token in CI. The offline tier seeded here
//! produces equivalent bare-repo URLs (`file://...`) so smoke tests
//! exercise the full clone/fetch/checkout path without ever touching
//! the network.
//!
//! # Layout
//!
//! Each [`seed_bare_repo`] call produces a `<basename>.git` directory
//! containing the bare repo and a sibling work directory that gets
//! cleaned up on return. Use [`seed_pack_template`] for the common
//! "leaf pack with one declarative action" shape; use [`seed_bare_repo`]
//! directly for custom payloads.
//!
//! # No network, no credentials
//!
//! Every git invocation runs with `GIT_CONFIG_GLOBAL` /
//! `GIT_CONFIG_SYSTEM` redirected to an empty file so user-level git
//! settings (`init.defaultBranch`, `commit.gpgsign`, `core.autocrlf`)
//! cannot leak in and make tests host-dependent.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// One file to write into the seed work directory before the initial
/// commit. Path is relative to the work directory.
pub struct SeedFile {
    pub relative: PathBuf,
    pub contents: String,
}

/// Seed a bare repo at `<base_dir>/<basename>.git` carrying the given
/// files in its first commit on `main`. Returns the bare-repo path
/// (suitable for `file://` URL conversion via
/// [`crate::worktree::file_url_from_path`] if you need URL form).
pub fn seed_bare_repo(base_dir: &Path, basename: &str, files: &[SeedFile]) -> Result<PathBuf> {
    init_git_identity();
    std::fs::create_dir_all(base_dir)
        .with_context(|| format!("create base_dir {}", base_dir.display()))?;
    let work = base_dir.join(format!("__seed-{basename}-work"));
    if work.exists() {
        std::fs::remove_dir_all(&work).context("cleanup prior seed work dir")?;
    }
    std::fs::create_dir_all(&work).with_context(|| format!("create work {}", work.display()))?;

    for f in files {
        let target = work.join(&f.relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create seed file parent {}", parent.display()))?;
        }
        std::fs::write(&target, &f.contents)
            .with_context(|| format!("write seed file {}", target.display()))?;
    }

    git(&work, &["init", "-q", "-b", "main"])?;
    git(&work, &["config", "user.email", "smoke@grex.local"])?;
    git(&work, &["config", "user.name", "smoke"])?;
    git(&work, &["config", "commit.gpgsign", "false"])?;
    git(&work, &["add", "-A"])?;
    git(&work, &["commit", "-q", "-m", "seed"])?;

    let bare = base_dir.join(format!("{basename}.git"));
    if bare.exists() {
        std::fs::remove_dir_all(&bare).context("cleanup prior bare clone")?;
    }
    git(base_dir, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()])?;

    // Leave the work dir cleaned up so the base_dir holds only
    // `<basename>.git` per seed call — keeps fixture inspection cheap.
    let _ = std::fs::remove_dir_all(&work);

    Ok(bare)
}

/// Convenience wrapper around [`seed_bare_repo`] that ships a minimal
/// declarative leaf pack: `pack.yaml` declaring one `mkdir` action.
/// `sink_dir` is where the action will create its target dir at sync
/// time — pass an absolute path under the test's tempdir so the action
/// is observable without escaping the sandbox.
pub fn seed_pack_template(base_dir: &Path, basename: &str, sink_dir: &Path) -> Result<PathBuf> {
    let pack_yaml = format!(
        "schema_version: \"1\"\n\
         name: {basename}\n\
         type: declarative\n\
         actions:\n  \
           - mkdir:\n      \
               path: {sink}\n",
        basename = basename,
        sink = sink_dir.join(format!("made-{basename}")).to_string_lossy().replace('\\', "/"),
    );
    seed_bare_repo(
        base_dir,
        basename,
        &[SeedFile { relative: PathBuf::from(".grex/pack.yaml"), contents: pack_yaml }],
    )
}

/// Convert a bare-repo path (or any local path) to a `file://` URL that
/// gix (and command-line git) will accept as a clone source. Mirrors
/// the helper in `import_then_sync.rs` but lives here so the offline
/// tier has no test-tree dependency.
pub fn file_url(path: &Path) -> String {
    // Canonicalize so relative paths in seed dirs round-trip, then
    // normalize separators for the URL form.
    let canon = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut s = canon.to_string_lossy().replace('\\', "/");
    // Windows canonicalize() returns `\\?\C:\...` — strip the prefix
    // so the resulting URL is a valid file:// host-less URI.
    if let Some(stripped) = s.strip_prefix("//?/") {
        s = stripped.to_string();
    }
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// One row in a synthetic cfg-shape `REPOS.json`. `platform` is
/// optional and surfaces the cfg layout convention (cmn/win/lnx/mac).
pub struct ReposRow {
    pub url: String,
    pub path: String,
    pub platform: Option<String>,
}

/// Render a `REPOS.json` document from the rows. Stable ordering so
/// snapshot assertions are deterministic.
pub fn render_repos_json(rows: &[ReposRow]) -> String {
    let arr: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            let mut m = BTreeMap::new();
            m.insert("url".to_string(), serde_json::Value::String(r.url.clone()));
            m.insert("path".to_string(), serde_json::Value::String(r.path.clone()));
            if let Some(p) = &r.platform {
                m.insert("platform".to_string(), serde_json::Value::String(p.clone()));
            }
            serde_json::Value::Object(m.into_iter().collect())
        })
        .collect();
    let body = serde_json::to_string_pretty(&serde_json::Value::Array(arr))
        .expect("serde_json cannot fail on owned Vec");
    format!("{body}\n")
}

fn git(cwd: &Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("spawn git {args:?} in {cwd:?}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "git {args:?} in {} failed (status {}): {}",
            cwd.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr),
        );
    }
    Ok(())
}

fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "smoke");
        std::env::set_var("GIT_AUTHOR_EMAIL", "smoke@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "smoke");
        std::env::set_var("GIT_COMMITTER_EMAIL", "smoke@grex.local");
        let null_cfg = std::env::temp_dir().join("grex-smoke-empty-gitconfig");
        let _ = std::fs::write(&null_cfg, b"");
        std::env::set_var("GIT_CONFIG_GLOBAL", &null_cfg);
        std::env::set_var("GIT_CONFIG_SYSTEM", &null_cfg);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn seed_bare_repo_creates_clone_on_main() {
        let dir = tempdir().unwrap();
        let bare = seed_bare_repo(
            dir.path(),
            "alpha",
            &[SeedFile { relative: PathBuf::from("README"), contents: "alpha\n".into() }],
        )
        .unwrap();
        assert!(bare.is_dir(), "bare clone must exist");
        assert!(bare.join("HEAD").is_file());
        let head = std::fs::read_to_string(bare.join("HEAD")).unwrap();
        assert!(head.contains("refs/heads/main"), "HEAD must point to main");
    }

    #[test]
    fn seed_pack_template_writes_declarative_manifest() {
        let dir = tempdir().unwrap();
        let sink = dir.path().join("sink");
        let bare = seed_pack_template(dir.path(), "beta", &sink).unwrap();
        // Inspect the seeded commit by cloning and reading pack.yaml.
        let work = dir.path().join("verify");
        Command::new("git")
            .args(["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()])
            .output()
            .expect("git on PATH");
        let body = std::fs::read_to_string(work.join(".grex/pack.yaml")).unwrap();
        assert!(body.contains("name: beta"));
        assert!(body.contains("mkdir"));
        assert!(body.contains("made-beta"));
    }

    #[test]
    fn render_repos_json_emits_platform_when_set() {
        let body = render_repos_json(&[
            ReposRow { url: "u1".into(), path: "p1".into(), platform: Some("cmn".into()) },
            ReposRow { url: "u2".into(), path: "p2".into(), platform: None },
        ]);
        assert!(body.contains("\"platform\": \"cmn\""));
        // second row omits the key entirely (BTreeMap ordering puts path before url before platform).
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(v[1].get("platform").is_none());
    }

    #[test]
    fn file_url_round_trips_through_gix_form() {
        let dir = tempdir().unwrap();
        let url = file_url(dir.path());
        assert!(url.starts_with("file://"));
        // The URL must reference an existing on-disk path.
        let stripped = url.strip_prefix("file://").unwrap();
        let stripped = stripped.trim_start_matches('/');
        // Windows: path may include drive letter (C:/...) — present in the URL.
        assert!(
            stripped.contains(':')
                || stripped.starts_with('/')
                || std::path::Path::new(stripped).exists()
        );
    }
}
