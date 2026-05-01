//! v1.2.1 item 4 — `grex doctor --scan-undeclared` filesystem walker.
//!
//! The default doctor walk is manifest-driven: it visits every meta declared
//! by `pack.yaml` `children:` chains. This module complements that walk with
//! a full-filesystem scan that surfaces `.git/` directories which exist on
//! disk but are NOT registered anywhere in the manifest tree. Useful for
//! policy audits where an operator wants to confirm that nothing has been
//! dropped into the workspace tree without going through `grex add`.
//!
//! The scan is read-only — no clones, no fetches, no manifest writes, no
//! lockfile writes. Output is a list of [`UndeclaredRepo`] entries that
//! the CLI dispatcher renders to stdout.
//!
//! ## Filtering rules
//!
//! 1. Skip the `.grex/` housekeeping directory and everything beneath it
//!    (grex's own state).
//! 2. Skip any `.git/` whose containing directory is a registered pack
//!    declared by the manifest tree (those are managed by grex).
//! 3. Skip any `.git/` whose path lies INSIDE a registered pack's subtree
//!    (those are sub-packs the registered pack owns; they belong to the
//!    inner walker, not to this audit).
//! 4. Skip nested `.git/` discoveries — once we report an undeclared `.git/`
//!    at path `P`, do not descend into `P` looking for more (a git repo's
//!    interior is not a grex concern).
//!
//! ## URL inference
//!
//! For each surviving `.git/` we shell out to `git -C <repo> config --get
//! remote.origin.url`. Any failure (missing git binary, no remote configured,
//! detached repo) maps to `inferred_url = None` which the CLI renders as
//! `[unknown]`. The audit must keep going on per-repo failures — a single
//! detached repo cannot abort the whole scan.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::pack;

/// One on-disk git repo that is not registered in the manifest tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndeclaredRepo {
    /// Path to the directory containing the `.git/` (NOT the `.git/`
    /// itself), normalised relative to the workspace root and using
    /// forward slashes.
    pub path: PathBuf,
    /// Best-effort `remote.origin.url`. `None` when the repo has no
    /// origin remote, or when invoking `git` failed for any reason.
    pub inferred_url: Option<String>,
}

/// Errors surfaced by [`scan_undeclared`]. Only hard failures that abort
/// the entire scan; per-repo failures (e.g. git missing for one repo)
/// degrade to `inferred_url = None` rather than erroring out.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    /// Workspace root is not a directory or cannot be read.
    #[error("workspace root unreadable: {path}: {source}")]
    WorkspaceUnreadable {
        /// Path that failed to read.
        path: PathBuf,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
}

/// Recursively scan `workspace` for `.git/` directories that are not
/// registered in the meta-tree rooted at `workspace`.
///
/// `depth` bounds how deep the walk descends:
/// * `None` → unbounded.
/// * `Some(0)` → workspace root only (no children).
/// * `Some(n)` → up to `n` directory levels below the workspace root.
///
/// The returned vec is sorted by relative path for deterministic output.
pub fn scan_undeclared(
    workspace: &Path,
    depth: Option<usize>,
) -> Result<Vec<UndeclaredRepo>, ScanError> {
    let registered = collect_registered_packs(workspace);
    let mut found: Vec<UndeclaredRepo> = Vec::new();
    walk(workspace, workspace, 0, depth, &registered, &mut found)?;
    found.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(found)
}

/// Walk `dir` looking for `.git/` directories. Recursion is bounded by
/// `depth_cap` (relative to the workspace root).
fn walk(
    workspace: &Path,
    dir: &Path,
    depth: usize,
    depth_cap: Option<usize>,
    registered: &BTreeSet<PathBuf>,
    out: &mut Vec<UndeclaredRepo>,
) -> Result<(), ScanError> {
    let entries = std::fs::read_dir(dir).map_err(|source| ScanError::WorkspaceUnreadable {
        path: dir.to_path_buf(),
        source,
    })?;

    // First pass: detect a `.git/` (dir) or `.git` (file — gitlink) at
    // this level. If present, the containing directory is a git repo.
    // Decide whether to report it, then DO NOT descend further into
    // sibling directories' git internals — but continue scanning sibling
    // non-`.git` entries the same way as for any other directory.
    let mut is_repo = false;
    let mut subdirs: Vec<PathBuf> = Vec::new();

    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        let path = entry.path();

        if name_str == ".git" {
            // `.git/` directory OR a `.git` file (worktree gitlink).
            // Both mark the containing directory as a git repo.
            is_repo = true;
            continue;
        }

        // Skip grex's own housekeeping dir at any level.
        if name_str == ".grex" {
            continue;
        }

        // Symlinked directories are skipped — following them risks
        // cycles and doubles up on repos already counted via their
        // canonical path.
        if !ft.is_dir() {
            continue;
        }

        subdirs.push(path);
    }

    // Decide whether `dir` itself is an undeclared repo. The workspace
    // root, even if it is a git repo, is never reported — the operator
    // already knows about it.
    if is_repo && dir != workspace {
        let rel = dir.strip_prefix(workspace).unwrap_or(dir).to_path_buf();
        let is_registered = registered.contains(&rel);
        let inside_registered = is_inside_registered(&rel, registered);
        if !is_registered && !inside_registered {
            out.push(UndeclaredRepo {
                path: rel,
                inferred_url: probe_origin_url(dir),
            });
        }
        // In all `is_repo` cases (registered, inside-registered, or
        // newly reported) we stop descending. A repo's interior is not
        // a grex audit concern, and a registered pack is owned by the
        // manifest walker.
        return Ok(());
    }

    // Bound recursion.
    if let Some(cap) = depth_cap {
        if depth >= cap {
            return Ok(());
        }
    }

    for sub in subdirs {
        walk(workspace, &sub, depth + 1, depth_cap, registered, out)?;
    }
    Ok(())
}

/// True when `rel` lies INSIDE one of the registered pack subtrees (a
/// strict descendant — equality is handled by the caller).
fn is_inside_registered(rel: &Path, registered: &BTreeSet<PathBuf>) -> bool {
    for reg in registered {
        if rel != reg && rel.starts_with(reg) {
            return true;
        }
    }
    false
}

/// Walk the manifest tree rooted at `workspace` and collect every
/// declared child's relative path. Returns paths normalised with the
/// workspace as the implicit prefix, e.g. `["alpha", "alpha/beta"]`.
fn collect_registered_packs(workspace: &Path) -> BTreeSet<PathBuf> {
    let mut out: BTreeSet<PathBuf> = BTreeSet::new();
    walk_manifest(workspace, workspace, &mut out);
    out
}

fn walk_manifest(workspace: &Path, meta_dir: &Path, out: &mut BTreeSet<PathBuf>) {
    let manifest_path = meta_dir.join(".grex").join("pack.yaml");
    let raw = match std::fs::read_to_string(&manifest_path) {
        Ok(s) => s,
        Err(_) => return,
    };
    let manifest = match pack::parse(&raw) {
        Ok(m) => m,
        Err(_) => return,
    };
    for child in &manifest.children {
        let segment = child.path.clone().unwrap_or_else(|| child.effective_path());
        let child_dir = meta_dir.join(&segment);
        if let Ok(rel) = child_dir.strip_prefix(workspace) {
            out.insert(rel.to_path_buf());
        }
        // Recurse only when the child carries its own manifest. Plain-
        // git children (no `.grex/pack.yaml`) are leaves — same rule the
        // distributed lockfile fold uses (see lockfile/distributed.rs).
        if child_dir.join(".grex").join("pack.yaml").is_file() {
            walk_manifest(workspace, &child_dir, out);
        }
    }
}

/// Best-effort `git config --get remote.origin.url` probe.
/// Swallows stderr; returns `None` on any failure (missing git, no
/// remote, non-zero exit).
fn probe_origin_url(repo: &Path) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["config", "--get", "remote.origin.url"])
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8(output.stdout).ok()?.trim().to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Make `dir` look like a git repo by writing a `.git/HEAD` file.
    /// `git config --get remote.origin.url` will fail (no config), so
    /// the inferred URL will be `None` unless the test seeds one.
    fn fake_repo(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
        fs::write(dir.join(".git/HEAD"), b"ref: refs/heads/main\n").unwrap();
    }

    /// Seed a pack.yaml at `<meta_dir>/.grex/pack.yaml` declaring the
    /// given children (each `(segment, url)`).
    fn write_meta_yaml(meta_dir: &Path, name: &str, children: &[(&str, &str)]) {
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

    #[test]
    fn empty_workspace_returns_nothing() {
        let d = tempdir().unwrap();
        let found = scan_undeclared(d.path(), None).unwrap();
        assert!(found.is_empty(), "no .git/ → no findings; got: {found:?}");
    }

    #[test]
    fn registered_pack_with_git_is_filtered_out() {
        // Workspace declares one child `alpha`. Drop a `.git/` inside
        // `alpha/` — it must NOT be reported because it is registered.
        let d = tempdir().unwrap();
        write_meta_yaml(d.path(), "root", &[("alpha", "https://example/alpha.git")]);
        fake_repo(&d.path().join("alpha"));

        let found = scan_undeclared(d.path(), None).unwrap();
        assert!(found.is_empty(), "registered pack must not be reported; got: {found:?}");
    }

    #[test]
    fn untracked_repo_is_reported() {
        // Root has one declared child `alpha`. Drop an unrelated
        // `vendor/legacy` repo — it must be reported.
        let d = tempdir().unwrap();
        write_meta_yaml(d.path(), "root", &[("alpha", "https://example/alpha.git")]);
        fake_repo(&d.path().join("alpha"));
        fake_repo(&d.path().join("vendor").join("legacy"));

        let found = scan_undeclared(d.path(), None).unwrap();
        assert_eq!(found.len(), 1, "exactly one undeclared repo; got: {found:?}");
        assert_eq!(found[0].path, PathBuf::from("vendor").join("legacy"));
        // No remote.origin.url configured by the fixture → None.
        assert!(found[0].inferred_url.is_none());
    }

    #[test]
    fn nested_tree_with_depth_one_only_walks_top_level() {
        // Root declares `alpha`. There is also an undeclared
        // `vendor/legacy` (depth 2). With `--depth 1` we descend into
        // `vendor/` (depth 1) but not into `vendor/legacy/` (depth 2),
        // so the repo is missed. `--depth 2` (or unbounded) finds it.
        let d = tempdir().unwrap();
        write_meta_yaml(d.path(), "root", &[("alpha", "https://example/alpha.git")]);
        fake_repo(&d.path().join("alpha"));
        fake_repo(&d.path().join("vendor").join("legacy"));

        let depth1 = scan_undeclared(d.path(), Some(1)).unwrap();
        assert!(
            depth1.is_empty(),
            "depth=1 must not find vendor/legacy/.git; got: {depth1:?}"
        );

        let unbounded = scan_undeclared(d.path(), None).unwrap();
        assert_eq!(unbounded.len(), 1);
        assert_eq!(unbounded[0].path, PathBuf::from("vendor").join("legacy"));
    }

    #[test]
    fn registered_pack_subtree_is_skipped() {
        // Even if a stray `.git/` lives INSIDE a registered pack's
        // subtree, the audit ignores it — that interior is owned by the
        // pack, not the audit.
        let d = tempdir().unwrap();
        write_meta_yaml(d.path(), "root", &[("alpha", "https://example/alpha.git")]);
        fake_repo(&d.path().join("alpha"));
        // A second `.git/` lives at `alpha/inner/.git/`. The walker
        // returns at `alpha/` once it confirms `alpha` is a registered
        // pack, so `alpha/inner/.git/` is never observed.
        fake_repo(&d.path().join("alpha").join("inner"));

        let found = scan_undeclared(d.path(), None).unwrap();
        assert!(found.is_empty(), "interior of registered pack must be skipped; got: {found:?}");
    }

    #[test]
    fn workspace_root_git_is_not_reported() {
        // The workspace itself being a git repo is normal (the meta
        // pack lives in a git repo). It must not appear in the report.
        let d = tempdir().unwrap();
        fake_repo(d.path());

        let found = scan_undeclared(d.path(), None).unwrap();
        assert!(found.is_empty(), "workspace root must not be reported; got: {found:?}");
    }

    #[test]
    fn dotgrex_dir_is_skipped() {
        // Anything under `.grex/` is grex's own state — never an
        // undeclared repo. Drop a `.git/` under `.grex/` and make sure
        // the scan ignores it.
        let d = tempdir().unwrap();
        fs::create_dir_all(d.path().join(".grex")).unwrap();
        fake_repo(&d.path().join(".grex").join("foreign"));

        let found = scan_undeclared(d.path(), None).unwrap();
        assert!(found.is_empty(), ".grex/ tree must be skipped; got: {found:?}");
    }

    #[test]
    fn gitlink_file_variant_is_detected() {
        // A `.git` FILE (not a directory) marks a git worktree. Make
        // sure the scanner treats it the same as a `.git/` directory.
        let d = tempdir().unwrap();
        let repo = d.path().join("worktree").join("alpha");
        fs::create_dir_all(&repo).unwrap();
        fs::write(repo.join(".git"), b"gitdir: /some/elsewhere/.git/worktrees/alpha\n").unwrap();

        let found = scan_undeclared(d.path(), None).unwrap();
        assert_eq!(found.len(), 1, ".git file (gitlink) must be detected; got: {found:?}");
        assert_eq!(found[0].path, PathBuf::from("worktree").join("alpha"));
    }

    #[test]
    fn results_are_sorted_for_determinism() {
        // Stable output is a small but real ergonomic win — the CLI
        // dispatcher prints the vec verbatim, so out-of-order results
        // would manifest as flaky doctor output across runs.
        let d = tempdir().unwrap();
        fake_repo(&d.path().join("zebra"));
        fake_repo(&d.path().join("alpha"));
        fake_repo(&d.path().join("mango"));

        let found = scan_undeclared(d.path(), None).unwrap();
        let paths: Vec<&Path> = found.iter().map(|r| r.path.as_path()).collect();
        assert_eq!(
            paths,
            vec![Path::new("alpha"), Path::new("mango"), Path::new("zebra")],
        );
    }
}
