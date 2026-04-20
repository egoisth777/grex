//! [`GitBackend`] implementation backed by the `gix` crate (pure-Rust git).
//!
//! Uses synchronous gix APIs — the `blocking-network-client` feature rewrites
//! `async fn` on `Remote::connect` / `Prepare::receive` into sync via
//! `maybe_async`. No tokio runtime is required or spawned here.
//!
//! Auth policy: relies on gix defaults (system SSH keys, HTTPS anonymous).
//! Credential prompting, keyring integration, and SSH-agent wiring are
//! deferred to a future slice and deliberately absent here.
//!
//! # Checkout scope
//!
//! [`GixBackend::checkout`] detaches HEAD at the resolved commit and then
//! materialises that commit's tree into the working directory + index via
//! `gix_worktree_state::checkout`. That sub-crate is already transitively in
//! the tree via `gix`, so it adds no net download.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use gix::progress::Discard;
use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
use gix::refs::Target;
use gix::remote::Direction;

use super::error::GitError;
use super::{ClonedRepo, GitBackend};
use crate::fs::ScopedLock;

/// Pure-Rust [`GitBackend`] driven by the `gix` crate.
///
/// All operations run on the calling thread. The backend holds no state — a
/// single instance can be shared behind `Arc` across the future walker/exec
/// pipelines.
///
/// Note: `Clone` is intentionally *not* derived; it would conflict with the
/// [`GitBackend::clone`] method at the method-resolution level. Callers that
/// need multiple handles should wrap in `Arc` or construct a fresh
/// [`GixBackend::new`] (the type is zero-sized).
#[derive(Debug, Default)]
pub struct GixBackend;

impl GixBackend {
    /// Construct a new backend. Equivalent to [`GixBackend::default`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl GitBackend for GixBackend {
    fn name(&self) -> &'static str {
        "gix"
    }

    fn clone(&self, url: &str, dest: &Path, r#ref: Option<&str>) -> Result<ClonedRepo, GitError> {
        // Per-repo lock: the sidecar lives in the parent dir (keyed by dest's
        // last component) so the clone can still require `dest` to be empty.
        // Once clone has happened, subsequent fetch/checkout continue to use
        // the *same* sidecar — its path is a pure function of `dest`.
        with_repo_lock(dest, || {
            ensure_dest_empty(dest)?;
            let repo = run_clone(url, dest, r#ref)?;
            let head_sha = read_head_sha(&repo)?;
            Ok(ClonedRepo { path: dest.to_path_buf(), head_sha })
        })
    }

    fn fetch(&self, dest: &Path) -> Result<(), GitError> {
        with_repo_lock(dest, || fetch_locked(dest))
    }

    fn checkout(&self, dest: &Path, r#ref: &str) -> Result<(), GitError> {
        // Per-repo lock held across the whole operation. Cleanliness is
        // validated AFTER the lock is acquired (a prior caller may have
        // left the tree dirty between `is_dirty()` at t=0 and us observing
        // it under the lock) and BEFORE HEAD is moved. Once we hold the
        // lock the worktree cannot be dirtied by a cooperating caller
        // before `materialise_tree`, so a single post-lock check is
        // sufficient to close the TOCTOU window.
        //
        // `materialise_tree` calls into gix with `overwrite_existing: true`.
        // That is safe here because cleanliness is enforced by this
        // function under the lock; we deliberately do not rely on gix's
        // `overwrite_existing: false` escape hatch (changing it to `false`
        // would break legitimate sync-after-stale-files recovery flows).
        with_repo_lock(dest, || {
            let repo = open_repo(dest)?;
            ensure_clean_worktree(&repo, dest)?;
            let target = resolve_ref(&repo, r#ref)?;
            update_head_detached(&repo, r#ref, target)?;
            materialise_tree(&repo, r#ref, target)
        })
    }

    fn head_sha(&self, dest: &Path) -> Result<String, GitError> {
        let repo = open_repo(dest)?;
        read_head_sha(&repo)
    }
}

// ---------------------------------------------------------------------------
// helpers — each kept small so trait methods stay under cyclomatic budget.
// ---------------------------------------------------------------------------

/// Lock sidecar path for per-repo serialisation. Kept in the parent dir so
/// the clone path can still require `dest` to be empty, and the sidecar
/// survives a `rm -rf <dest>` rebuild between retries.
fn repo_lock_path(dest: &Path) -> PathBuf {
    let parent = dest.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let stem = dest
        .file_name()
        .map_or_else(|| std::ffi::OsString::from("repo"), std::ffi::OsStr::to_os_string);
    let mut name = std::ffi::OsString::from(".grex-backend-");
    name.push(&stem);
    name.push(".lock");
    parent.join(name)
}

/// Run `op` while holding the per-repo filesystem lock for `dest`.
///
/// Lock is blocking (`fd_lock::RwLock::write`) — per-repo contention is
/// rare and waiting is the right UX when it happens (e.g. two sync runs
/// both wanting to `fetch` the same clone cooperate naturally). The
/// workspace-level lock in [`crate::sync::run`] is the fast-failing guard
/// that prevents two syncs from ever reaching this point concurrently on
/// the same workspace.
fn with_repo_lock<T, F>(dest: &Path, op: F) -> Result<T, GitError>
where
    F: FnOnce() -> Result<T, GitError>,
{
    let lock_path = repo_lock_path(dest);
    let mut lock = ScopedLock::open(&lock_path)
        .map_err(|e| GitError::Internal(format!("open repo lock {}: {e}", lock_path.display())))?;
    let _guard = lock.acquire().map_err(|e| {
        GitError::Internal(format!("acquire repo lock {}: {e}", lock_path.display()))
    })?;
    op()
}

/// Fetch body, factored out so the trait method stays a thin lock wrapper.
fn fetch_locked(dest: &Path) -> Result<(), GitError> {
    let repo = open_repo(dest)?;
    let remote = repo
        .find_default_remote(Direction::Fetch)
        .ok_or_else(|| {
            GitError::FetchFailed(dest.to_path_buf(), "no default remote configured".into())
        })?
        .map_err(|e| GitError::FetchFailed(dest.to_path_buf(), e.to_string()))?;

    let connection = remote
        .connect(Direction::Fetch)
        .map_err(|e| GitError::FetchFailed(dest.to_path_buf(), e.to_string()))?;

    let interrupt = AtomicBool::new(false);
    let prepare = connection
        .prepare_fetch(Discard, gix::remote::ref_map::Options::default())
        .map_err(|e| GitError::FetchFailed(dest.to_path_buf(), e.to_string()))?;

    prepare
        .receive(Discard, &interrupt)
        .map_err(|e| GitError::FetchFailed(dest.to_path_buf(), e.to_string()))?;

    Ok(())
}

/// Error unless `dest` is absent or an empty directory.
fn ensure_dest_empty(dest: &Path) -> Result<(), GitError> {
    if !dest.exists() {
        return Ok(());
    }
    let mut iter = std::fs::read_dir(dest)
        .map_err(|e| GitError::Internal(format!("read_dir({}): {e}", dest.display())))?;
    if iter.next().is_some() {
        return Err(GitError::DestinationNotEmpty(dest.to_path_buf()));
    }
    Ok(())
}

/// Run `gix::prepare_clone` → `fetch_then_checkout` → `main_worktree`.
fn run_clone(url: &str, dest: &Path, r#ref: Option<&str>) -> Result<gix::Repository, GitError> {
    let mut prepare = gix::prepare_clone(url, dest)
        .map_err(|e| GitError::CloneFailed { url: url.to_string(), detail: e.to_string() })?;

    if let Some(name) = r#ref {
        prepare = prepare
            .with_ref_name(Some(name))
            .map_err(|e| GitError::CloneFailed { url: url.to_string(), detail: e.to_string() })?;
    }

    let interrupt = AtomicBool::new(false);
    let (mut checkout, _) = prepare
        .fetch_then_checkout(Discard, &interrupt)
        .map_err(|e| GitError::CloneFailed { url: url.to_string(), detail: e.to_string() })?;

    let (repo, _) = checkout
        .main_worktree(Discard, &interrupt)
        .map_err(|e| GitError::CloneFailed { url: url.to_string(), detail: e.to_string() })?;
    Ok(repo)
}

/// Open an existing repo or map to [`GitError::NotARepository`].
fn open_repo(dest: &Path) -> Result<gix::Repository, GitError> {
    gix::open(dest).map_err(|_| GitError::NotARepository(dest.to_path_buf()))
}

/// Refuse checkout if the working tree has uncommitted changes.
fn ensure_clean_worktree(repo: &gix::Repository, dest: &Path) -> Result<(), GitError> {
    match repo.is_dirty() {
        Ok(false) => Ok(()),
        Ok(true) => Err(GitError::DirtyWorkingTree(dest.to_path_buf())),
        Err(e) => Err(GitError::Internal(format!("is_dirty({}): {e}", dest.display()))),
    }
}

/// Resolve a ref string (branch, tag, SHA) to a concrete object id.
fn resolve_ref(repo: &gix::Repository, r#ref: &str) -> Result<gix::ObjectId, GitError> {
    repo.rev_parse_single(r#ref)
        .map(|id| id.detach())
        .map_err(|_| GitError::RefNotFound(r#ref.to_string()))
}

/// Update `HEAD` to point at `target` in detached form, leaving a reflog
/// entry that credits grex.
fn update_head_detached(
    repo: &gix::Repository,
    r#ref: &str,
    target: gix::ObjectId,
) -> Result<(), GitError> {
    let edit = RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: format!("grex: checkout {ref_name}", ref_name = r#ref).into(),
            },
            expected: PreviousValue::Any,
            new: Target::Object(target),
        },
        name: "HEAD".try_into().expect("HEAD is a valid ref name"),
        deref: false,
    };
    repo.edit_reference(edit)
        .map(|_| ())
        .map_err(|e| GitError::CheckoutFailed { r#ref: r#ref.to_string(), detail: e.to_string() })
}

/// Materialise `target`'s tree into the working tree + index, overwriting
/// whatever is on disk. Precondition: the worktree was clean on entry (the
/// caller enforced that via [`ensure_clean_worktree`]).
///
/// Uses `gix_worktree_state::checkout` with `overwrite_existing = true` so
/// files from the previous HEAD that are absent in the target tree are
/// re-materialised atop. This is the equivalent of `git reset --hard` for a
/// clean working tree.
fn materialise_tree(
    repo: &gix::Repository,
    r#ref: &str,
    target: gix::ObjectId,
) -> Result<(), GitError> {
    let workdir = repo.work_dir().ok_or_else(|| GitError::CheckoutFailed {
        r#ref: r#ref.to_string(),
        detail: "bare repository has no working tree".into(),
    })?;

    let tree_id = tree_of_commit(repo, r#ref, target)?;
    let mut index = build_index_from_tree(repo, r#ref, tree_id)?;

    let objects = repo.objects.clone().into_arc().map_err(|e: std::io::Error| {
        GitError::CheckoutFailed { r#ref: r#ref.to_string(), detail: e.to_string() }
    })?;
    let interrupt = AtomicBool::new(false);

    let opts = gix_worktree_state::checkout::Options {
        overwrite_existing: true,
        destination_is_initially_empty: false,
        ..Default::default()
    };

    gix_worktree_state::checkout(
        &mut index,
        workdir.to_path_buf(),
        objects,
        &Discard,
        &Discard,
        &interrupt,
        opts,
    )
    .map_err(|e| GitError::CheckoutFailed { r#ref: r#ref.to_string(), detail: e.to_string() })?;

    index.write(Default::default()).map_err(|e| GitError::CheckoutFailed {
        r#ref: r#ref.to_string(),
        detail: e.to_string(),
    })?;
    Ok(())
}

/// Peel `commit_id` to its tree object id.
fn tree_of_commit(
    repo: &gix::Repository,
    r#ref: &str,
    commit_id: gix::ObjectId,
) -> Result<gix::ObjectId, GitError> {
    let object = repo.find_object(commit_id).map_err(|e| GitError::CheckoutFailed {
        r#ref: r#ref.to_string(),
        detail: e.to_string(),
    })?;
    let tree = object.peel_to_kind(gix::object::Kind::Tree).map_err(|e| {
        GitError::CheckoutFailed { r#ref: r#ref.to_string(), detail: e.to_string() }
    })?;
    Ok(tree.id)
}

/// Build a fresh index file from `tree_id`, ready for `gix_worktree_state::checkout`.
fn build_index_from_tree(
    repo: &gix::Repository,
    r#ref: &str,
    tree_id: gix::ObjectId,
) -> Result<gix::index::File, GitError> {
    let validate = gix::validate::path::component::Options::default();
    let state = gix::index::State::from_tree(&tree_id, &repo.objects, validate).map_err(|e| {
        GitError::CheckoutFailed { r#ref: r#ref.to_string(), detail: e.to_string() }
    })?;
    Ok(gix::index::File::from_state(state, repo.index_path()))
}

/// Return HEAD as a 40-char lowercase hex SHA.
fn read_head_sha(repo: &gix::Repository) -> Result<String, GitError> {
    let id = repo.head_id().map_err(|e| GitError::Internal(format!("head_id: {e}")))?;
    Ok(id.detach().to_hex().to_string())
}

/// Build a `file://` URL from an absolute path, normalising Windows
/// backslashes to forward slashes as gix/git require.
///
/// Exposed for tests; not part of the stable public API.
#[doc(hidden)]
#[must_use]
pub fn file_url_from_path(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}
