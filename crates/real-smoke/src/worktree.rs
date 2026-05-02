//! RAII git-worktree manager.
//!
//! [`WorktreeGuard`] owns one throwaway `git worktree` per test invocation,
//! backed by a per-URL **base clone cache** so we only pay the network cost
//! once per fixture across an entire `cargo test` run.
//!
//! Layout (in `base_dir`):
//!
//! ```text
//! base_dir/
//!   <slug>/                  ← bare-ish base clone (full history, never
//!                              touched after creation)
//!   worktrees/<uuid>/        ← throwaway worktree (test workspace)
//! ```
//!
//! On [`WorktreeGuard::drop`], the worktree directory is removed via
//! `git worktree remove --force` so the base clone's metadata stays clean.
//! A best-effort `git worktree prune` runs as a final sweep; failures during
//! drop are logged to stderr and swallowed (cannot panic from `Drop`).

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// RAII handle to a git worktree carved out of a cached base clone.
///
/// The worktree path is exposed via [`WorktreeGuard::path`]; pass it to
/// [`crate::grex_cli::run`] as the `cwd`.
pub struct WorktreeGuard {
    base_clone: PathBuf,
    worktree_path: PathBuf,
}

impl WorktreeGuard {
    /// Clones `repo_url` into `<base_dir>/<slug>` (skipped if already present)
    /// and adds a throwaway worktree at `<base_dir>/worktrees/<uuid>`.
    ///
    /// `branch` is the ref to check out in the worktree (typically `"main"`).
    pub fn new(repo_url: &str, base_dir: &Path, branch: &str) -> Result<Self> {
        std::fs::create_dir_all(base_dir)
            .with_context(|| format!("creating base_dir {}", base_dir.display()))?;

        let slug = url_to_slug(repo_url);
        let base_clone = base_dir.join(&slug);
        if !base_clone.exists() {
            let status = Command::new("git")
                .args(["clone", "--no-tags", repo_url])
                .arg(&base_clone)
                .status()
                .with_context(|| format!("spawning `git clone {repo_url}`"))?;
            if !status.success() {
                anyhow::bail!("git clone {repo_url} failed (status: {status})");
            }
        }

        let worktree_root = base_dir.join("worktrees");
        std::fs::create_dir_all(&worktree_root)
            .with_context(|| format!("creating {}", worktree_root.display()))?;
        let worktree_path = worktree_root.join(unique_id());

        let status = Command::new("git")
            .arg("-C")
            .arg(&base_clone)
            .args(["worktree", "add", "--detach"])
            .arg(&worktree_path)
            .arg(branch)
            .status()
            .with_context(|| format!("spawning `git worktree add` for {repo_url}"))?;
        if !status.success() {
            anyhow::bail!(
                "git worktree add failed for {} -> {} (status: {status})",
                base_clone.display(),
                worktree_path.display()
            );
        }

        Ok(Self { base_clone, worktree_path })
    }

    /// Path to the live worktree directory (= the test workspace).
    pub fn path(&self) -> &Path {
        &self.worktree_path
    }

    /// Path to the cached base clone backing this worktree.
    pub fn base_clone(&self) -> &Path {
        &self.base_clone
    }
}

impl Drop for WorktreeGuard {
    fn drop(&mut self) {
        // Best-effort cleanup; never panic from Drop.
        let remove = Command::new("git")
            .arg("-C")
            .arg(&self.base_clone)
            .args(["worktree", "remove", "--force"])
            .arg(&self.worktree_path)
            .status();
        if let Err(err) = remove {
            eprintln!(
                "real-smoke: warning: `git worktree remove` failed for {}: {err}",
                self.worktree_path.display()
            );
        }

        let _ = Command::new("git")
            .arg("-C")
            .arg(&self.base_clone)
            .args(["worktree", "prune"])
            .status();

        // If the worktree directory still exists (e.g. git refused to remove
        // it because of a manifest left behind), make a best-effort filesystem
        // sweep so subsequent test runs don't trip over the leftover.
        if self.worktree_path.exists() {
            let _ = std::fs::remove_dir_all(&self.worktree_path);
        }
    }
}

/// Maps `git@github.com:foo/bar.git` (or `https://...`) to a filesystem-safe
/// slug like `github_com__foo__bar`. Stable across runs so the base clone
/// cache hits.
fn url_to_slug(url: &str) -> String {
    url.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => c,
            _ => '_',
        })
        .collect::<String>()
        .trim_matches('_')
        .to_string()
}

/// Returns a process-unique id for naming the worktree directory. Combines
/// the current timestamp with the process id so concurrent test threads
/// inside the same `cargo test` invocation don't collide.
fn unique_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let pid = std::process::id();
    format!("wt-{pid}-{nanos}")
}
