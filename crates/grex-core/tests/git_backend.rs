//! Integration tests for the [`grex_core::git`] backend.
//!
//! # No-network policy
//!
//! Every test builds a **local bare repo** under a `tempfile::TempDir` and
//! clones/fetches from its `file://` URL. CI must not hit the public
//! internet.
//!
//! # Windows path quirk
//!
//! `file://` URLs require forward slashes. On Windows, `Path::display` emits
//! backslashes, so we normalise via `gix_backend::file_url_from_path` before
//! handing URLs to the backend.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use grex_core::git::gix_backend::file_url_from_path;
use grex_core::{ClonedRepo, GitBackend, GitError, GixBackend};
use tempfile::TempDir;

/// CI runners (ubuntu, macos) have no global `user.name`/`user.email`, which
/// makes gix reject reflog/ref transactions during `checkout` and `fetch`.
/// Set identity env vars exactly once per test binary; gix and git both honour
/// `GIT_AUTHOR_*` / `GIT_COMMITTER_*` when no config is available.
fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        // Called once at test entry, before any test spawns threads that
        // might observe a torn read of the environment.
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
    });
}

// -------------------------------------------------------------------------
// Bare-repo fixture helpers
// -------------------------------------------------------------------------

/// Build a bare repo with a single commit on `main` that adds `README.md`.
/// Returns `(bare_path, head_sha)`.
fn create_bare_repo(tmp: &Path) -> (PathBuf, String) {
    init_git_identity();
    let work = tmp.join("seed-work");
    fs::create_dir_all(&work).unwrap();

    // Use the `git` CLI here for fixture construction — it's available on
    // every supported dev/CI platform and keeps the fixture independent of
    // the backend under test.
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    fs::write(work.join("README.md"), b"hello grex\n").unwrap();
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "-q", "-m", "initial"]);

    let head_sha = stdout(&work, &["rev-parse", "HEAD"]).trim().to_owned();

    let bare = tmp.join("seed.git");
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);

    (bare, head_sha)
}

/// Create an additional commit on `main` in `bare`, return the new SHA.
fn add_commit_to_bare(tmp: &Path, bare: &Path, file: &str, msg: &str) -> String {
    // Clone bare → working copy → commit → push back.
    let work = tmp.join(format!("editor-{msg}"));
    run_git(tmp, &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    fs::write(work.join(file), format!("{msg}\n")).unwrap();
    run_git(&work, &["add", file]);
    run_git(&work, &["commit", "-q", "-m", msg]);
    run_git(&work, &["push", "-q", "origin", "main"]);
    stdout(&work, &["rev-parse", "HEAD"]).trim().to_owned()
}

/// Tag `sha` as `tag` in the bare repo.
fn tag_in_bare(tmp: &Path, bare: &Path, tag: &str, sha: &str) {
    let work = tmp.join(format!("tagger-{tag}"));
    run_git(tmp, &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
    run_git(&work, &["tag", tag, sha]);
    run_git(&work, &["push", "-q", "origin", tag]);
}

/// Push a new branch pointing at `sha` to the bare repo.
fn branch_in_bare(tmp: &Path, bare: &Path, branch: &str, sha: &str) {
    let work = tmp.join(format!("brancher-{branch}"));
    run_git(tmp, &["clone", "-q", bare.to_str().unwrap(), work.to_str().unwrap()]);
    run_git(&work, &["branch", branch, sha]);
    run_git(&work, &["push", "-q", "origin", branch]);
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

fn stdout(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("git on PATH");
    assert!(out.status.success());
    String::from_utf8(out.stdout).expect("utf8")
}

// -------------------------------------------------------------------------
// Tests
// -------------------------------------------------------------------------

#[test]
fn clone_empty_dest_ok() {
    let tmp = TempDir::new().unwrap();
    let (bare, _) = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("clone-ok");

    let b = GixBackend::new();
    let ClonedRepo { path, head_sha } = b.clone(&url, &dest, None).expect("clone");
    assert_eq!(path, dest);
    assert_eq!(head_sha.len(), 40);
    assert!(dest.join("README.md").is_file());
}

#[test]
fn clone_nonempty_dest_errors() {
    let tmp = TempDir::new().unwrap();
    let (bare, _) = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("already-here");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("stray.txt"), b"x").unwrap();

    let err = GixBackend::new().clone(&url, &dest, None).unwrap_err();
    match err {
        GitError::DestinationNotEmpty(p) => assert_eq!(p, dest),
        other => panic!("expected DestinationNotEmpty, got {other:?}"),
    }
}

#[test]
fn clone_with_ref_checks_out() {
    let tmp = TempDir::new().unwrap();
    let (bare, first_sha) = create_bare_repo(tmp.path());
    let second_sha = add_commit_to_bare(tmp.path(), &bare, "more.txt", "second");
    tag_in_bare(tmp.path(), &bare, "v1", &first_sha);

    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("clone-tag");
    let cloned = GixBackend::new().clone(&url, &dest, Some("v1")).expect("clone at tag v1");

    assert_eq!(cloned.head_sha, first_sha, "v1 should pin to first commit");
    assert_ne!(cloned.head_sha, second_sha);
}

#[test]
fn fetch_existing_repo_ok() {
    let tmp = TempDir::new().unwrap();
    let (bare, _) = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("fetch-repo");
    let backend = GixBackend::new();

    backend.clone(&url, &dest, None).expect("clone");
    let _new_sha = add_commit_to_bare(tmp.path(), &bare, "late.txt", "later");

    backend.fetch(&dest).expect("fetch");
    // Working tree must remain untouched — README.md still present, late.txt
    // must NOT have been checked out (fetch never touches the worktree).
    assert!(dest.join("README.md").is_file());
    assert!(!dest.join("late.txt").exists());
}

#[test]
fn checkout_resolves_branch_name() {
    let tmp = TempDir::new().unwrap();
    let (bare, first_sha) = create_bare_repo(tmp.path());
    let second_sha = add_commit_to_bare(tmp.path(), &bare, "feature.txt", "feature");
    branch_in_bare(tmp.path(), &bare, "feat/x", &second_sha);

    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("co-branch");
    let backend = GixBackend::new();
    let cloned = backend.clone(&url, &dest, None).expect("clone");
    // Clone puts us on whatever the remote HEAD was (main). Branch feat/x
    // exists as a remote ref already since bare was cloned after the push.
    assert_eq!(cloned.head_sha, second_sha);

    // Move HEAD back to first commit via SHA, then forward to feat/x via
    // ref name. First step avoids depending on starting position.
    backend.checkout(&dest, &first_sha).expect("checkout first");
    assert_eq!(backend.head_sha(&dest).unwrap(), first_sha);

    backend.checkout(&dest, "origin/feat/x").expect("checkout feat/x");
    assert_eq!(backend.head_sha(&dest).unwrap(), second_sha);
}

#[test]
fn checkout_ref_not_found_errors() {
    let tmp = TempDir::new().unwrap();
    let (bare, _) = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("co-missing");
    let backend = GixBackend::new();
    backend.clone(&url, &dest, None).expect("clone");

    let err = backend.checkout(&dest, "does-not-exist").unwrap_err();
    match err {
        GitError::RefNotFound(name) => assert_eq!(name, "does-not-exist"),
        other => panic!("expected RefNotFound, got {other:?}"),
    }
}

#[test]
fn head_sha_on_nonrepo_errors() {
    let tmp = TempDir::new().unwrap();
    let empty = tmp.path().join("not-a-repo");
    fs::create_dir_all(&empty).unwrap();

    let err = GixBackend::new().head_sha(&empty).unwrap_err();
    match err {
        GitError::NotARepository(p) => assert_eq!(p, empty),
        other => panic!("expected NotARepository, got {other:?}"),
    }
}

#[test]
fn head_sha_length_40() {
    let tmp = TempDir::new().unwrap();
    let (bare, _) = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);
    let dest = tmp.path().join("sha-len");
    let backend = GixBackend::new();
    backend.clone(&url, &dest, None).expect("clone");

    let sha = backend.head_sha(&dest).expect("head");
    assert_eq!(sha.len(), 40);
    assert!(sha.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
}

// -------------------------------------------------------------------------
// Trait-object smoke test: proves the trait surface is mockable so future
// slices can substitute alternative backends (IPC, shell, in-memory) without
// touching callers.
// -------------------------------------------------------------------------

#[derive(Default)]
struct MockBackend {
    name: &'static str,
}

impl GitBackend for MockBackend {
    fn name(&self) -> &'static str {
        if self.name.is_empty() {
            "mock"
        } else {
            self.name
        }
    }
    fn clone(&self, _url: &str, dest: &Path, _ref: Option<&str>) -> Result<ClonedRepo, GitError> {
        Ok(ClonedRepo { path: dest.to_path_buf(), head_sha: "0".repeat(40) })
    }
    fn fetch(&self, _dest: &Path) -> Result<(), GitError> {
        Ok(())
    }
    fn checkout(&self, _dest: &Path, _r: &str) -> Result<(), GitError> {
        Ok(())
    }
    fn head_sha(&self, _dest: &Path) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

#[test]
fn mock_backend_satisfies_trait() {
    let backend: Box<dyn GitBackend> = Box::<MockBackend>::default();
    assert_eq!(backend.name(), "mock");

    let tmp = TempDir::new().unwrap();
    let dest = tmp.path().join("mock");
    let c = backend.clone("http://example/foo", &dest, None).unwrap();
    assert_eq!(c.head_sha.len(), 40);
    backend.fetch(&dest).unwrap();
    backend.checkout(&dest, "whatever").unwrap();
    assert_eq!(backend.head_sha(&dest).unwrap().len(), 40);
}
