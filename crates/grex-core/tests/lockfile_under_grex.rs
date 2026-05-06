//! v1.3.2 W2 / B11 — lockfile-location migration regression suite.
//!
//! Pins the three lock-artifact paths after the v1.3.2 hard-cut path move:
//!
//! 1. **Per-pack pack-lock** lives at `<pack_workdir>/.grex/.grex-lock` —
//!    NOT at `<pack_workdir>/.grex-lock` (legacy v1.2.x location).
//! 2. **Workspace sync sidecar** lives at `<workspace>/.grex/.grex.sync.lock`
//!    — NOT at `<workspace>/.grex.sync.lock` (legacy v1.2.x location).
//! 3. **Per-repo backend lock** lives at
//!    `<parent_meta>/.grex/locks/<child-path>.backend.lock` (parent-owned)
//!    — NOT at `<dest>.grex-backend.lock` (legacy sibling) and NOT at
//!    `<dest>/.grex/.grex-backend.lock` (briefly-proposed inside-dest).
//!
//! Negative assertions (asserts the OLD paths are NEVER created during or
//! after a sync) cover the "hard-cut" guarantee in design.md §"Hard-cut
//! readers".
//!
//! Slash-path child fixture (`tools/foo`) verifies nested directory
//! creation under `<parent>/.grex/locks/` — `fs::create_dir_all` is
//! invoked on first lock acquire.

use std::path::{Path, PathBuf};

use grex_core::pack_lock::{PackLock, PACK_LOCK_FILE_NAME, PACK_LOCK_REL_PATH};
use grex_core::{BackendLockCtx, GitBackend, GixBackend};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Helper: spawn a local bare git repo so the GixBackend has something to
// clone/fetch. Mirrors the helper in `git_backend.rs` but trimmed to
// what these tests need.
// ---------------------------------------------------------------------------

fn init_git_identity() {
    use std::sync::OnceLock;
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
    });
}

fn run_git(cwd: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .current_dir(cwd)
        .args(args)
        .status()
        .unwrap_or_else(|e| panic!("spawn git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

fn create_bare_repo(tmp: &Path) -> PathBuf {
    init_git_identity();
    let work = tmp.join("seed-work");
    std::fs::create_dir_all(&work).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    std::fs::write(work.join("README.md"), b"hello grex\n").unwrap();
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "-q", "-m", "initial"]);

    let bare = tmp.join("seed.git");
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

fn file_url_from_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

// ---------------------------------------------------------------------------
// 1. Per-pack pack-lock
// ---------------------------------------------------------------------------

/// `PackLock::open` materialises the sidecar at `<pack>/.grex/.grex-lock`,
/// creating the `.grex/` parent directory on demand.
#[test]
fn per_pack_lock_lands_under_grex_dir() {
    let tmp = TempDir::new().unwrap();
    let pack = tmp.path().join("pack-root");
    std::fs::create_dir_all(&pack).unwrap();

    let plock = PackLock::open(&pack).expect("open pack lock");

    let expected = pack.join(".grex").join(".grex-lock");
    assert_eq!(plock.path(), expected.as_path());
    assert!(expected.exists(), "pack lock file must be created on open");
    assert_eq!(plock.path().to_path_buf(), pack.join(PACK_LOCK_REL_PATH));
}

/// Negative: the legacy bare path `<pack>/.grex-lock` MUST NOT be created
/// by `PackLock::open` — v1.3.2 B11 hard-cut path move.
#[test]
fn per_pack_lock_legacy_bare_path_never_created() {
    let tmp = TempDir::new().unwrap();
    let pack = tmp.path().join("pack-root");
    std::fs::create_dir_all(&pack).unwrap();

    let _plock = PackLock::open(&pack).expect("open pack lock");

    let legacy_bare = pack.join(PACK_LOCK_FILE_NAME);
    assert!(
        !legacy_bare.is_file(),
        "v1.3.2 B11 regressed: legacy `<pack>/.grex-lock` was created at {}",
        legacy_bare.display()
    );
}

// ---------------------------------------------------------------------------
// 2. Per-repo backend lock — happy path + slash-path nesting + negatives
// ---------------------------------------------------------------------------

/// A bare-name child path lands the backend lock at
/// `<parent_meta>/.grex/locks/<name>.backend.lock`.
#[test]
fn backend_lock_bare_name_under_parent_grex_locks() {
    let tmp = TempDir::new().unwrap();
    let bare = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);

    let parent_meta = tmp.path().join("parent");
    std::fs::create_dir_all(&parent_meta).unwrap();
    let dest = parent_meta.join("foo");
    let lock_ctx = BackendLockCtx::new(&parent_meta, "foo");

    let backend = GixBackend::new();
    backend.clone(&url, &dest, Some("main"), lock_ctx).expect("clone");

    let expected = parent_meta.join(".grex").join("locks").join("foo.backend.lock");
    assert!(
        expected.is_file(),
        "v1.3.2 B11: backend lock must land at {} after first clone",
        expected.display()
    );
}

/// Slash-path child (`tools/foo`) verifies the intermediate `tools/`
/// directory is auto-created under `<parent>/.grex/locks/` on first
/// acquire. The lock filename appends `.backend.lock` to the LAST
/// segment, so `tools/foo` → `<parent>/.grex/locks/tools/foo.backend.lock`.
#[test]
fn backend_lock_slash_path_creates_intermediate_dirs() {
    let tmp = TempDir::new().unwrap();
    let bare = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);

    let parent_meta = tmp.path().join("parent");
    std::fs::create_dir_all(parent_meta.join("tools")).unwrap();
    let dest = parent_meta.join("tools").join("foo");
    let lock_ctx = BackendLockCtx::new(&parent_meta, "tools/foo");

    let backend = GixBackend::new();
    backend.clone(&url, &dest, Some("main"), lock_ctx).expect("clone");

    let expected = parent_meta.join(".grex").join("locks").join("tools").join("foo.backend.lock");
    assert!(
        expected.is_file(),
        "v1.3.2 B11: slash-path child backend lock must land at {} after first clone",
        expected.display()
    );
    // The intermediate `tools/` directory must exist as a directory.
    let tools_dir = parent_meta.join(".grex").join("locks").join("tools");
    assert!(
        tools_dir.is_dir(),
        "intermediate `tools/` lock dir must be auto-created on first acquire"
    );
}

/// Negative: the legacy sibling location `<dest>.grex-backend-<name>.lock`
/// (v1.2.x / v1.3.0/v1.3.1 placement) MUST NOT be created by any backend
/// operation post-v1.3.2.
#[test]
fn backend_lock_legacy_sibling_path_never_created() {
    let tmp = TempDir::new().unwrap();
    let bare = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);

    let parent_meta = tmp.path().join("parent");
    std::fs::create_dir_all(&parent_meta).unwrap();
    let dest = parent_meta.join("foo");
    let lock_ctx = BackendLockCtx::new(&parent_meta, "foo");

    let backend = GixBackend::new();
    backend.clone(&url, &dest, Some("main"), lock_ctx).expect("clone");

    // v1.2.x sibling — was `<parent>/.grex-backend-foo.lock`.
    let sibling = parent_meta.join(".grex-backend-foo.lock");
    assert!(
        !sibling.exists(),
        "v1.3.2 B11 regressed: legacy sibling lock at {} must never be created",
        sibling.display()
    );

    // Briefly-proposed inside-dest variant — `<dest>/.grex/.grex-backend.lock`.
    let inside_dest = dest.join(".grex").join(".grex-backend.lock");
    assert!(
        !inside_dest.exists(),
        "v1.3.2 B11: inside-dest backend lock variant at {} must never be created",
        inside_dest.display()
    );

    // Bare adjacent dot-prefix — `<dest>.grex-backend.lock`.
    let mut adjacent_dot = dest.clone().into_os_string();
    adjacent_dot.push(".grex-backend.lock");
    assert!(
        !PathBuf::from(&adjacent_dot).exists(),
        "v1.3.2 B11: legacy adjacent-dot lock at {:?} must never be created",
        adjacent_dot
    );
}

// ---------------------------------------------------------------------------
// 3. Workspace sync sidecar — verified via `grex_core::sync::run` end-to-end.
// ---------------------------------------------------------------------------

/// A real `sync::run` against a minimal declarative workspace materialises
/// the workspace sync sidecar at `<workspace>/.grex/.grex.sync.lock` —
/// NOT at `<workspace>/.grex.sync.lock` (legacy).
#[test]
fn workspace_sync_lock_lands_under_grex_dir() {
    use grex_core::sync::{self, SyncOptions};
    use tokio_util::sync::CancellationToken;

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    std::fs::create_dir_all(workspace.join(".grex")).unwrap();
    std::fs::write(
        workspace.join(".grex").join("pack.yaml"),
        "schema_version: \"1\"\nname: ws-root\ntype: declarative\nversion: \"0.0.1\"\nactions: []\n",
    )
    .unwrap();

    let opts = SyncOptions::new().with_workspace(Some(workspace.clone()));
    sync::run(&workspace, &opts, &CancellationToken::new()).expect("sync ok");

    let canonical = workspace.canonicalize().expect("canonicalise");
    let expected = canonical.join(".grex").join(".grex.sync.lock");
    assert!(
        expected.is_file(),
        "v1.3.2 B11: workspace sync lock must land at {}",
        expected.display()
    );

    // Negative: legacy workspace-root location must NEVER be created.
    let legacy = canonical.join(".grex.sync.lock");
    assert!(
        !legacy.is_file(),
        "v1.3.2 B11 regressed: legacy workspace sync lock at {} must never be created",
        legacy.display()
    );
}
