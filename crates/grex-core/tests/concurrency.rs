//! M3 post-review Fix C: concurrency locks.
//!
//! Pins three invariants that closed CRITICAL/HIGH races in the sync
//! pipeline:
//!
//! 1. Workspace-level try-lock in [`grex_core::sync::run`] — two concurrent
//!    sync runs on the same workspace must not both clone into the same
//!    child path. The second caller observes [`SyncError::WorkspaceBusy`].
//! 2. Per-repo blocking lock inside [`grex_core::git::GixBackend`] — two
//!    concurrent `fetch(same_dest)` calls must serialise, both succeed,
//!    and leave the repo in a consistent state.
//! 3. `checkout` re-validates working-tree cleanliness **after** acquiring
//!    the per-repo lock (closes a TOCTOU window where a second process
//!    could dirty the tree between the first caller's dirty-check and its
//!    write phase).
//!
//! Timing-sensitive tests gate both threads behind a [`std::sync::Barrier`]
//! so they reach their lock-attempt call simultaneously.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, OnceLock};
use std::thread;

use grex_core::git::gix_backend::file_url_from_path;
use grex_core::sync::{run as sync_run, SyncError, SyncOptions};
use grex_core::{GitBackend, GixBackend};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Fixture helpers (small duplicate of `git_backend.rs` — we need just
// enough of a bare-repo seed to exercise clone/fetch/checkout under lock).
// ---------------------------------------------------------------------------

fn init_git_identity() {
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
    fs::create_dir_all(&work).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    fs::write(work.join("README.md"), b"hello grex\n").unwrap();
    run_git(&work, &["add", "README.md"]);
    run_git(&work, &["commit", "-q", "-m", "initial"]);

    let bare = tmp.join("seed.git");
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

// ---------------------------------------------------------------------------
// Fix 1 — workspace-level lock in sync
// ---------------------------------------------------------------------------

/// Two threads with the same workspace; the first holds the workspace lock
/// externally (simulating an in-flight sync), the second calls `sync::run`
/// and must return `SyncError::WorkspaceBusy`.
///
/// We pre-acquire the lock via [`grex_core::fs::ScopedLock`] rather than
/// kicking off a full second `sync::run`; this gives us deterministic
/// control of the contention window without needing a mock executor.
#[test]
fn two_syncs_same_workspace_second_errors_busy() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();

    // Thread 1 acquires the workspace lock and holds it until we release it.
    // We use a channel-free sync via Barrier: t1 grabs lock → signals →
    // t2 attempts sync → t1 releases.
    let gate = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));

    let ws_for_t1 = workspace.clone();
    let gate_t1 = Arc::clone(&gate);
    let release_t1 = Arc::clone(&release);
    let holder = thread::spawn(move || {
        let lock_path = ws_for_t1.join(".grex.sync.lock");
        let mut lock = grex_core::fs::ScopedLock::open(&lock_path).expect("open lock");
        let _g = lock.try_acquire().expect("no io err").expect("lock acquired by t1");
        gate_t1.wait(); // t2 may now attempt sync
        release_t1.wait(); // hold until t2 has observed busy
                           // guard drops here
    });

    // Build a minimal pack tree so `run` reaches the workspace-lock
    // acquisition. An empty actions list keeps it deterministic.
    let pack_root = tmp.path().join("pack");
    fs::create_dir_all(pack_root.join(".grex")).unwrap();
    fs::write(
        pack_root.join(".grex").join("pack.yaml"),
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\nactions: []\n",
    )
    .unwrap();

    gate.wait(); // t1 is holding the lock
    let opts = SyncOptions { workspace: Some(workspace.clone()), ..Default::default() };
    let err = sync_run(&pack_root, &opts).expect_err("must be busy");
    match err {
        SyncError::WorkspaceBusy { workspace: ws, lock_path } => {
            assert_eq!(ws, workspace);
            assert_eq!(lock_path, workspace.join(".grex.sync.lock"));
        }
        other => panic!("expected WorkspaceBusy, got {other:?}"),
    }
    release.wait();
    holder.join().unwrap();
}

/// After the first sync finishes (lock released on drop), a second sync on
/// the same workspace succeeds — the busy condition is transient, not a
/// sticky error state.
#[test]
fn sync_lock_releases_on_completion() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let pack_root = tmp.path().join("pack");
    fs::create_dir_all(pack_root.join(".grex")).unwrap();
    fs::write(
        pack_root.join(".grex").join("pack.yaml"),
        "schema_version: \"1\"\nname: root\ntype: declarative\nversion: \"0.0.1\"\nactions: []\n",
    )
    .unwrap();

    let opts = SyncOptions { workspace: Some(workspace.clone()), ..Default::default() };
    sync_run(&pack_root, &opts).expect("first sync");
    sync_run(&pack_root, &opts).expect("second sync after first drops lock");
}

// ---------------------------------------------------------------------------
// Fix 2 — per-repo lock inside GixBackend
// ---------------------------------------------------------------------------

/// Two threads call `backend.fetch(same_dest)` simultaneously. Both must
/// succeed (blocking lock serialises them); the repo must parse afterwards
/// and HEAD must resolve.
#[test]
fn git_backend_concurrent_fetch_serialized() {
    let tmp = TempDir::new().unwrap();
    let bare = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);

    let backend = Arc::new(GixBackend::new());
    let dest = tmp.path().join("clone");
    <GixBackend as GitBackend>::clone(&*backend, &url, &dest, Some("main")).expect("initial clone");

    let barrier = Arc::new(Barrier::new(2));
    let mut handles = Vec::new();
    for _ in 0..2 {
        let b = Arc::clone(&backend);
        let d = dest.clone();
        let bar = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            bar.wait();
            b.fetch(&d)
        }));
    }
    for h in handles {
        h.join().unwrap().expect("fetch succeeds under lock");
    }

    // Final state consistent — HEAD resolves cleanly.
    backend.head_sha(&dest).expect("HEAD readable after concurrent fetch");
}

/// Concurrent clones into two *different* destinations proceed in parallel
/// (no global lock) and both succeed. This guards against over-serialisation
/// if the per-repo lock were accidentally made global.
#[test]
fn git_backend_concurrent_clone_distinct_dests() {
    let tmp = TempDir::new().unwrap();
    let bare = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);

    let backend = Arc::new(GixBackend::new());
    let dest_a = tmp.path().join("a");
    let dest_b = tmp.path().join("b");
    let url = Arc::new(url);

    let barrier = Arc::new(Barrier::new(2));
    let (ba, bb) = (Arc::clone(&barrier), Arc::clone(&barrier));
    let (ua, ub) = (Arc::clone(&url), Arc::clone(&url));
    let ka = Arc::clone(&backend);
    let kb = Arc::clone(&backend);
    // `Arc<GixBackend>::clone` resolves to `Arc::clone`, not the
    // `GitBackend::clone` method. Disambiguate with UFCS.
    let ta = thread::spawn(move || {
        ba.wait();
        <GixBackend as GitBackend>::clone(&*ka, &ua, &dest_a, Some("main"))
    });
    let tb = thread::spawn(move || {
        bb.wait();
        <GixBackend as GitBackend>::clone(&*kb, &ub, &dest_b, Some("main"))
    });
    ta.join().unwrap().expect("clone a");
    tb.join().unwrap().expect("clone b");
}

// ---------------------------------------------------------------------------
// Fix 3 — checkout re-validates dirty under lock
// ---------------------------------------------------------------------------

/// Dirty worktree before checkout → `DirtyWorkingTree` error. This is the
/// same guarantee as before the fix, but now enforced under the per-repo
/// lock so it cannot be bypassed by a concurrent caller who dirties the
/// tree between the original `is_dirty()` probe and the write phase.
#[test]
fn git_backend_checkout_rejects_dirty_after_lock() {
    let tmp = TempDir::new().unwrap();
    let bare = create_bare_repo(tmp.path());
    let url = file_url_from_path(&bare);

    let backend = GixBackend::new();
    let dest = tmp.path().join("clone");
    backend.clone(&url, &dest, Some("main")).expect("clone");

    // Dirty the worktree: modify a tracked file.
    fs::write(dest.join("README.md"), b"dirty edit\n").unwrap();

    let err = backend.checkout(&dest, "main").expect_err("must refuse");
    assert!(
        matches!(err, grex_core::GitError::DirtyWorkingTree(_)),
        "expected DirtyWorkingTree, got {err:?}"
    );

    // Secondary invariant: the locally-dirty file was NOT overwritten
    // despite `materialise_tree` using `overwrite_existing: true`. The
    // guard is the cleanliness check under lock, not gix.
    let contents = fs::read_to_string(dest.join("README.md")).unwrap();
    assert_eq!(contents, "dirty edit\n", "dirty file preserved on rejected checkout");
}
