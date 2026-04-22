//! Per-pack `.grex-lock` file lock — feat-m6-2.
//!
//! Acquires an exclusive `fd-lock` guard on `<pack_path>/.grex-lock` for the
//! full duration of a pack-type plugin lifecycle method. Prevents two
//! concurrent tasks (in-process) or processes (cross-process) from operating
//! on the same pack at the same time.
//!
//! ## Lock ordering (tier 3 of 5)
//!
//! The spec fixes the global acquire order as:
//!
//! 1. workspace-sync lock (`<workspace>/.grex.sync.lock`)
//! 2. scheduler semaphore permit (feat-m6-1)
//! 3. **per-pack `.grex-lock`** — this module
//! 4. per-repo backend lock (`<dest>.grex-backend.lock`)
//! 5. manifest RW lock (`grex.jsonl` sidecar)
//!
//! Plugins acquire tier 2 (permit) and tier 3 (pack lock) in that order
//! inside every `PackTypePlugin` method. In debug builds, [`with_tier`]
//! enforces strictly-increasing tiers on a thread-local stack; a reversal
//! panics in debug and logs `tracing::error!` in release.
//!
//! ## In-process vs cross-process serialisation
//!
//! `fd-lock`'s `write()` is synchronous and blocks the calling OS thread
//! until the kernel flock is free. Calling it directly inside an async
//! plugin method would block a tokio worker thread; with a multi-thread
//! runtime this scales poorly, and recursive re-entry on the same pack
//! (meta-plugin walking into a symlinked child that points back at the
//! parent) hangs the task outright because the second `write()` waits on
//! the first, which is still on-stack.
//!
//! To avoid both problems we layer a process-wide async mutex keyed by
//! canonical pack path **in front of** the fd-lock acquire:
//!
//! * [`PackLock::acquire_async`] first takes the canonical-path mutex
//!   (`tokio::sync::Mutex`), which serialises in-process tasks without
//!   blocking workers and detects same-task re-entry as a
//!   [`PackLockError::Busy`] via `try_lock`.
//! * Inside the async mutex it calls the blocking fd-lock `write()` —
//!   fast because the only remaining contention is cross-process, which
//!   is rare.
//! * On `Drop` it releases the fd-lock guard first, then the async
//!   mutex guard — reverse acquire order.

#![allow(unsafe_code)]

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use fd_lock::{RwLock, RwLockWriteGuard};

/// Stable name of the per-pack lock file created inside every pack root.
/// Exported so the managed-gitignore writer can hide it from `git status`.
pub const PACK_LOCK_FILE_NAME: &str = ".grex-lock";

/// Error surfaced by [`PackLock::open`], [`PackLock::acquire`], and
/// [`PackLock::try_acquire`].
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum PackLockError {
    /// I/O error opening or locking the sidecar file.
    #[error("pack lock i/o on `{}`: {source}", path.display())]
    Io {
        /// Resolved `<pack_path>/.grex-lock` path.
        path: PathBuf,
        /// Underlying OS error.
        #[source]
        source: io::Error,
    },
    /// Non-blocking probe returned busy. The blocking path
    /// ([`PackLock::acquire_async`]) never produces this for cross-pack
    /// contention — it waits. Emitted by:
    ///
    /// * [`PackLock::try_acquire`] on any contention.
    /// * [`PackLock::acquire_async`] on same-process re-entry (a plugin
    ///   that recurses back into the same pack root). Cross-task
    ///   contention blocks on the async mutex and never surfaces here.
    #[error("pack lock `{}` is busy", path.display())]
    Busy {
        /// Lock path that was contended.
        path: PathBuf,
    },
}

/// Outcome of [`PackLock::acquire_cancellable`] — either the
/// underlying lock acquire failed, or the supplied cancellation token
/// fired before the guard was returned. Distinct from
/// [`crate::scheduler::Cancelled`]: that ZST signals semaphore-permit
/// cancellation; this variant signals pack-lock cancellation. Verb
/// bodies translate either into the same `PluginError::Cancelled` at
/// the call site (feat-m7-1 Stages 6-7).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum PackLockErrorOrCancelled {
    /// The cancellation token fired before the lock was acquired.
    /// The spawned blocking thread (if launched) may still be parked
    /// in `fd_lock::write()` — see [`PackLock::acquire_cancellable`]
    /// for the OS-thread leak-window contract.
    #[error("pack lock acquire cancelled")]
    Cancelled,
    /// The underlying lock acquire failed before cancellation fired.
    #[error(transparent)]
    Lock(#[from] PackLockError),
}

use std::sync::Weak;

fn path_mutex_registry() -> &'static Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>> {
    static REG: OnceLock<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

/// feat-m6 H3 — prune entries whose only remaining reference was the
/// registry itself. Called opportunistically on every `mutex_for` so
/// long-running processes that open many distinct pack paths do not
/// accumulate unbounded registry entries. Runs under the registry
/// mutex so there is no race against a concurrent `mutex_for`.
fn prune_dead(reg: &mut HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>) {
    reg.retain(|_, weak| weak.strong_count() > 0);
}

fn mutex_for(canonical: &Path) -> Arc<tokio::sync::Mutex<()>> {
    let mut reg = path_mutex_registry()
        .lock()
        .expect("pack lock path registry poisoned — this indicates a prior panic");
    // Try to reuse an existing live entry. If the Weak is dead
    // (no outstanding holders) fall through to insert a fresh Arc.
    if let Some(weak) = reg.get(canonical) {
        if let Some(existing) = weak.upgrade() {
            return existing;
        }
    }
    prune_dead(&mut reg);
    let m = Arc::new(tokio::sync::Mutex::new(()));
    reg.insert(canonical.to_path_buf(), Arc::downgrade(&m));
    m
}

fn canonical_or_raw(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Per-pack file lock wrapper.
///
/// Construction via [`PackLock::open`] creates (or re-opens) the sidecar
/// `<pack_path>/.grex-lock` but does **not** acquire the lock — call
/// [`PackLock::acquire_async`] for the async-safe blocking path,
/// [`PackLock::acquire`] for the thread-blocking synchronous path, or
/// [`PackLock::try_acquire`] for a fail-fast probe.
pub struct PackLock {
    inner: RwLock<File>,
    path: PathBuf,
    canonical: PathBuf,
}

impl PackLock {
    /// Open (and create if missing) the sidecar `<pack_path>/.grex-lock`.
    /// Does **not** acquire the lock.
    ///
    /// # Errors
    ///
    /// Returns [`PackLockError::Io`] if the sidecar cannot be opened or
    /// its parent directory cannot be created.
    pub fn open(pack_path: &Path) -> Result<Self, PackLockError> {
        let path = pack_path.join(PACK_LOCK_FILE_NAME);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| PackLockError::Io { path: path.clone(), source })?;
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .map_err(|source| PackLockError::Io { path: path.clone(), source })?;
        let canonical = canonical_or_raw(pack_path);
        Ok(Self { inner: RwLock::new(file), path, canonical })
    }

    /// Async acquire — pairs a process-wide [`tokio::sync::Mutex`] keyed
    /// by canonical pack path with the sidecar `fd-lock`. Safe to call
    /// from any tokio worker without blocking the runtime. Same-thread
    /// re-entry (nested synchronous call chain that re-enters the same
    /// pack root, e.g. meta-plugin recursion through a `..` child that
    /// points back at the parent) returns [`PackLockError::Busy`] rather
    /// than deadlocking.
    ///
    /// The returned [`PackLockHold`] drops the fd-lock guard and the
    /// async mutex guard in reverse acquire order at end-of-scope.
    ///
    /// # Errors
    ///
    /// * [`PackLockError::Busy`] on same-thread re-entry.
    /// * [`PackLockError::Io`] on any OS-level lock failure.
    pub async fn acquire_async(self) -> Result<PackLockHold, PackLockError> {
        // Same-thread re-entry detection — see module-level note on
        // tokio tasks and thread affinity. This covers the common
        // case where a nested plugin call runs on the same worker
        // thread between `.await` points (meta-plugin recursion).
        // Different threads holding the same pack root's mutex will
        // queue on `lock_owned().await` below instead.
        // Serialise in-process tasks on the canonical path via an
        // async mutex — safe across tokio workers and non-blocking on
        // the async runtime. Same-task re-entry (recursive plugin
        // invocation on the same pack root) is the caller's
        // responsibility to prevent via cycle detection; a same-task
        // re-entry here would hang at `lock_owned().await` because
        // tokio mutexes are non-reentrant.
        //
        // [`crate::plugin::pack_type::MetaPlugin`] threads the
        // `visited_meta` set through recursion and inserts the pack
        // root at every lifecycle entry so cycles halt with
        // [`crate::execute::ExecError::MetaCycle`] before this mutex
        // acquire runs. Other pack-type plugins are leaf by design
        // (declarative, scripted) and cannot re-enter.
        let mtx = mutex_for(&self.canonical);
        let mutex_guard = Arc::clone(&mtx).lock_owned().await;

        // Box `self` so its address is stable for the transmuted
        // `'static` guard lifetime. Take the fd-lock guard from the
        // boxed lock's `inner`.
        let boxed = Box::new(self);
        // feat-m6 H1 — `fd_lock::RwLock::write` is a synchronous
        // blocking call that waits on the kernel flock. Running it
        // directly on a tokio worker would park that worker until
        // the kernel released the lock, starving the runtime when
        // the only remaining contention is cross-process. Hop onto
        // the blocking-thread pool so async workers stay free. The
        // acquire happens inside `spawn_blocking` and the guard is
        // transmuted to `'static` before leaving the closure so the
        // box + guard can be returned as a pair.
        let join = tokio::task::spawn_blocking(
            move || -> Result<(Box<PackLock>, RwLockWriteGuard<'static, File>), PackLockError> {
                let mut boxed = boxed;
                // SAFETY: see outer comment block — `boxed` is moved
                // into the returned pair and never freed while the
                // guard is live; field order in `PackLockHold` makes
                // the guard drop first. Transmuting here (inside the
                // closure) lets us return both the box and the guard.
                let guard_ref = boxed
                    .inner
                    .write()
                    .map_err(|source| PackLockError::Io { path: boxed.path.clone(), source })?;
                let guard_static: RwLockWriteGuard<'static, File> = unsafe {
                    std::mem::transmute::<
                        RwLockWriteGuard<'_, File>,
                        RwLockWriteGuard<'static, File>,
                    >(guard_ref)
                };
                Ok((boxed, guard_static))
            },
        )
        .await;
        let (boxed, guard_static) = match join {
            Ok(res) => res?,
            Err(join_err) => {
                return Err(PackLockError::Io {
                    path: PathBuf::new(),
                    source: io::Error::other(join_err.to_string()),
                });
            }
        };

        Ok(PackLockHold {
            _fd_guard: Some(guard_static),
            _mutex_guard: Some(mutex_guard),
            _lock: boxed,
        })
    }

    /// Cancellable async acquire — same semantics as
    /// [`PackLock::acquire_async`] but races the acquire against a
    /// [`tokio_util::sync::CancellationToken`]. Used by the embedded
    /// MCP server (feat-m7-1) so a `notifications/cancelled` from the
    /// client unblocks tool handlers that are parked on a contended
    /// pack lock.
    ///
    /// **Consumes `self`** to mirror [`PackLock::acquire_async`] — the
    /// same boxed-fd + transmute lifetime dance is needed to hand the
    /// guard back across a `spawn_blocking` boundary, and reusing the
    /// consumes-self contract preserves drop ordering against
    /// [`PackLockHold`].
    ///
    /// ## OS-thread leak window — contract
    ///
    /// `fd_lock::write()` is a synchronous syscall that parks the
    /// calling OS thread until the kernel releases the flock. Once the
    /// blocking call has been launched on the
    /// [`tokio::task::spawn_blocking`] pool, **the runtime cannot
    /// interrupt it** — there is no portable way to unpark a thread
    /// blocked in `flock(2)`. When the cancellation token fires we
    /// resolve the outer `select!` with [`PackLockErrorOrCancelled::Cancelled`]
    /// immediately, but the spawned OS thread stays parked until the
    /// holder eventually releases. When that happens, the spawned
    /// thread acquires the guard, the `JoinHandle` resolves to
    /// `Ok((boxed, guard))`, and the tuple is dropped on the spot
    /// (because the `select!` arm has already won) — at which point
    /// the guard's `Drop` releases the kernel flock and a subsequent
    /// acquirer can proceed.
    ///
    /// In other words: **cancellation is observable to the caller
    /// instantly, but the underlying OS thread holds the lock briefly
    /// past the cancel point, until the syscall returns**. Callers
    /// that immediately re-attempt acquire on the same path may see
    /// transient contention until that thread drains. See
    /// `.omne/cfg/mcp.md` §Cancellation.
    ///
    /// # Errors
    ///
    /// * [`PackLockErrorOrCancelled::Cancelled`] — the token fired
    ///   before a guard was returned.
    /// * [`PackLockErrorOrCancelled::Lock`] wrapping
    ///   [`PackLockError::Busy`] on same-thread re-entry, or
    ///   [`PackLockError::Io`] on any OS-level lock failure.
    pub async fn acquire_cancellable(
        self,
        cancel: &::tokio_util::sync::CancellationToken,
    ) -> Result<PackLockHold, PackLockErrorOrCancelled> {
        // Mirror `acquire_async`: serialise on the canonical-path
        // async mutex first. Race the mutex acquire itself against
        // cancel — same-task re-entry would normally hang here, but
        // the cancel arm gives the caller an out.
        let mtx = mutex_for(&self.canonical);
        let mutex_guard = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(PackLockErrorOrCancelled::Cancelled),
            g = Arc::clone(&mtx).lock_owned() => g,
        };

        // Box `self` so the address is stable for the transmuted
        // `'static` guard lifetime — same dance as `acquire_async`.
        let boxed = Box::new(self);
        // Capture the sidecar path before the move into the closure
        // so the JoinError arm below can report it (the closure
        // consumes `boxed`, so we cannot read it from there).
        let join_err_path = boxed.path.clone();
        // feat-m7-1 — replicates the `acquire_async` blocking-pool
        // hop. The closure body is intentionally identical (do NOT
        // refactor — see the SAFETY note in `acquire_async`).
        let join = tokio::task::spawn_blocking(
            move || -> Result<(Box<PackLock>, RwLockWriteGuard<'static, File>), PackLockError> {
                let mut boxed = boxed;
                // SAFETY: see `acquire_async` — `boxed` is moved into
                // the returned pair and never freed while the guard
                // is live; field order in `PackLockHold` makes the
                // guard drop first.
                let guard_ref = boxed
                    .inner
                    .write()
                    .map_err(|source| PackLockError::Io { path: boxed.path.clone(), source })?;
                let guard_static: RwLockWriteGuard<'static, File> = unsafe {
                    std::mem::transmute::<
                        RwLockWriteGuard<'_, File>,
                        RwLockWriteGuard<'static, File>,
                    >(guard_ref)
                };
                Ok((boxed, guard_static))
            },
        );

        // Race the blocking acquire against the cancellation token.
        // If cancel wins, the JoinHandle is dropped on the spot — the
        // spawned OS thread stays parked in `fd_lock::write()` until
        // the kernel releases, at which point the returned tuple is
        // dropped (see contract note above) and the flock is freed.
        let join = tokio::select! {
            biased;
            () = cancel.cancelled() => return Err(PackLockErrorOrCancelled::Cancelled),
            res = join => res,
        };

        let (boxed, guard_static) = match join {
            Ok(res) => res.map_err(PackLockErrorOrCancelled::Lock)?,
            Err(join_err) => {
                return Err(PackLockErrorOrCancelled::Lock(PackLockError::Io {
                    path: join_err_path,
                    source: io::Error::other(join_err.to_string()),
                }));
            }
        };

        Ok(PackLockHold {
            _fd_guard: Some(guard_static),
            _mutex_guard: Some(mutex_guard),
            _lock: boxed,
        })
    }

    /// Thread-blocking acquire (no tokio integration). Waits on the
    /// fd-lock synchronously. Suitable for synchronous call sites only
    /// — async plugin methods MUST use [`PackLock::acquire_async`] to
    /// avoid blocking a tokio worker.
    ///
    /// Returns a borrowed [`RwLockWriteGuard`]; the caller owns both
    /// the outer [`PackLock`] and the guard in scope. Mirrors the
    /// [`crate::fs::ScopedLock`] shape.
    ///
    /// # Errors
    ///
    /// Returns [`PackLockError::Io`] if the OS lock call fails.
    pub fn acquire(&mut self) -> Result<RwLockWriteGuard<'_, File>, PackLockError> {
        self.inner.write().map_err(|source| PackLockError::Io { path: self.path.clone(), source })
    }

    /// Non-blocking probe: return [`PackLockError::Busy`] instead of
    /// waiting when another holder has the lock. Does not engage the
    /// async mutex — purely a fail-fast diagnostics hook.
    ///
    /// # Errors
    ///
    /// * [`PackLockError::Busy`] when a concurrent holder owns the lock.
    /// * [`PackLockError::Io`] on any other OS-level lock failure.
    pub fn try_acquire(&mut self) -> Result<RwLockWriteGuard<'_, File>, PackLockError> {
        match self.inner.try_write() {
            Ok(g) => Ok(g),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                Err(PackLockError::Busy { path: self.path.clone() })
            }
            Err(source) => Err(PackLockError::Io { path: self.path.clone(), source }),
        }
    }

    /// Sidecar path — `<pack_path>/.grex-lock`.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl std::fmt::Debug for PackLock {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackLock").field("path", &self.path).finish()
    }
}

/// RAII guard returned by [`PackLock::acquire_async`]. Holds the
/// sidecar-file `fd-lock` guard plus the process-wide async mutex
/// guard. Drops in reverse acquire order.
#[repr(C)]
pub struct PackLockHold {
    // Field order is load-bearing: `_fd_guard` drops first (releasing
    // the kernel flock), then `_mutex_guard` (releasing the async
    // serialisation slot), then `_lock` (closing the file handle).
    // `#[repr(C)]` pins source order to layout order so `offset_of!`
    // assertions below stay meaningful.
    _fd_guard: Option<RwLockWriteGuard<'static, File>>,
    _mutex_guard: Option<tokio::sync::OwnedMutexGuard<()>>,
    _lock: Box<PackLock>,
}

impl PackLockHold {
    /// Sidecar path for diagnostics.
    #[must_use]
    pub fn path(&self) -> &Path {
        self._lock.path()
    }
}

impl std::fmt::Debug for PackLockHold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PackLockHold").field("path", &self._lock.path()).finish()
    }
}

// Field-order static assertion (feat-m6 B3) — the safety argument for the
// unsafe lifetime extension in `acquire_async` depends on `_fd_guard`
// dropping before `_lock`. Rust drops struct fields in declaration order,
// so `_fd_guard` must sit at the lowest offset, then `_mutex_guard`, then
// `_lock`. A refactor that reorders these fields would silently break the
// Drop ordering and the transmuted `'static` borrow would outlive its box.
const _: () = {
    assert!(
        std::mem::offset_of!(PackLockHold, _fd_guard)
            < std::mem::offset_of!(PackLockHold, _mutex_guard),
        "PackLockHold field order: _fd_guard must precede _mutex_guard"
    );
    assert!(
        std::mem::offset_of!(PackLockHold, _mutex_guard)
            < std::mem::offset_of!(PackLockHold, _lock),
        "PackLockHold field order: _mutex_guard must precede _lock"
    );
};

impl Drop for PackLockHold {
    fn drop(&mut self) {
        // Explicit take() on fd-lock guard first — the transmuted
        // `'static` lifetime must expire before `_lock` drops.
        self._fd_guard.take();
        self._mutex_guard.take();
        // `_lock` drops last when the struct itself drops, closing the
        // underlying file handle.
    }
}

// ---------------------------------------------------------------------------
// Lock-ordering enforcement (debug builds).
// ---------------------------------------------------------------------------

/// Lock tier ordinals matching `.omne/cfg/concurrency.md`. Acquisitions
/// must strictly increase; reversed order risks the deadlock class the
/// feat-m6-3 Lean proof rules out.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// Workspace sync lock — `<workspace>/.grex.sync.lock`.
    WorkspaceSync = 0,
    /// Scheduler semaphore permit — feat-m6-1.
    Semaphore = 1,
    /// Per-pack `.grex-lock` — feat-m6-2 (this module).
    PerPack = 2,
    /// Per-repo backend lock — `<dest>.grex-backend.lock`.
    Backend = 3,
    /// Manifest RW lock — `grex.jsonl` sidecar.
    Manifest = 4,
}

/// Run `f` while `tier` is pushed on the current thread's tier stack.
/// Debug builds enforce strictly-increasing order across nested calls;
/// release builds skip the check entirely.
#[cfg(debug_assertions)]
pub fn with_tier<R>(tier: Tier, f: impl FnOnce() -> R) -> R {
    tier::push(tier);
    let out = f();
    tier::pop_if_top(tier);
    out
}

/// Release-build no-op mirror of [`with_tier`].
#[cfg(not(debug_assertions))]
#[inline]
pub fn with_tier<R>(_tier: Tier, f: impl FnOnce() -> R) -> R {
    f()
}

/// Wrap an async future in a task-local tier-stack scope so any
/// [`TierGuard`] pushes inside it land in the correct frame even when
/// the task migrates across tokio workers after `.await`. Release
/// builds compile this down to the raw future — no scope, no cost.
///
/// Callers should wrap every top-level async dispatch (e.g. the
/// per-pack plugin lifecycle calls driven by `rt.block_on(...)`) so
/// the tier check can operate on a fresh stack per dispatch.
#[cfg(debug_assertions)]
pub async fn with_tier_scope<F: std::future::Future>(f: F) -> F::Output {
    tier::TIER_STACK.scope(std::cell::RefCell::new(Vec::new()), f).await
}

/// Release-build no-op mirror of [`with_tier_scope`].
#[cfg(not(debug_assertions))]
#[inline]
pub async fn with_tier_scope<F: std::future::Future>(f: F) -> F::Output {
    f.await
}

/// RAII guard — pushes a tier onto the current-thread stack on
/// construction, pops on drop. Lets lifecycle prologues enforce
/// tier ordering across `.await` points without nesting the rest of
/// the body inside a `with_tier` closure. Debug builds carry the
/// ordering check; release builds compile to a zero-sized no-op.
///
/// Field/declaration-order note: callers must declare the guard
/// **before** the permit/hold it is scoping. Rust drops locals in
/// reverse declaration order, so `_tier` declared first drops last —
/// after the lock/permit releases — matching `with_tier` semantics.
#[must_use]
pub struct TierGuard {
    #[cfg(debug_assertions)]
    tier: Tier,
    // Sized placeholder for release so the type is still `Sized` and
    // `must_use`-meaningful. Zero-sized once inlined.
    #[cfg(not(debug_assertions))]
    _private: (),
}

impl TierGuard {
    /// Push `tier` onto the current-thread tier stack. Debug builds
    /// assert strictly-increasing order against the existing top.
    #[cfg(debug_assertions)]
    pub fn push(tier: Tier) -> Self {
        tier::push(tier);
        TierGuard { tier }
    }

    /// Release-build no-op constructor.
    #[cfg(not(debug_assertions))]
    #[inline]
    pub fn push(_tier: Tier) -> Self {
        TierGuard { _private: () }
    }
}

#[cfg(debug_assertions)]
impl Drop for TierGuard {
    fn drop(&mut self) {
        tier::pop_if_top(self.tier);
    }
}

#[cfg(debug_assertions)]
pub(crate) mod tier {
    use super::Tier;
    use std::cell::RefCell;

    // feat-m6 CI fix — previously this used `thread_local!`, but under a
    // tokio multi-thread runtime a task can resume on a different worker
    // after `.await`. A push on worker A followed by a yield and a pop on
    // worker B left A's stack polluted and tripped the tier-ordering
    // assertion on the next acquire. Migrating to `tokio::task_local!`
    // pins the stack to the *task*, not the worker, so nested
    // `TierGuard` bookkeeping follows the task across workers.
    //
    // `try_with` silently no-ops outside a `TIER_STACK.scope(...)`
    // frame — that makes the module safe to use from synchronous
    // (non-tokio) test harnesses and the module's own unit tests at
    // the cost of debug-only tier enforcement being disabled there.
    // Production dispatch wraps every pack-type plugin call in a scope
    // (see `sync::dispatch_*`), so real runs retain enforcement.
    tokio::task_local! {
        pub(crate) static TIER_STACK: RefCell<Vec<Tier>>;
    }

    pub fn push(next: Tier) {
        let _ = TIER_STACK.try_with(|s| {
            let mut s = s.borrow_mut();
            if let Some(&top) = s.last() {
                assert!(
                    next > top,
                    "lock tier violation: trying to acquire {next:?} while already holding {top:?} \
                     (tiers must be strictly increasing — see .omne/cfg/concurrency.md)"
                );
            }
            s.push(next);
        });
    }

    pub fn pop_if_top(expected: Tier) {
        let _ = TIER_STACK.try_with(|s| {
            let mut s = s.borrow_mut();
            if s.last().copied() == Some(expected) {
                s.pop();
            } else {
                tracing::error!(
                    target: "grex::concurrency",
                    "tier pop mismatch: expected {:?} at top, stack = {:?}",
                    expected,
                    *s
                );
            }
        });
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn pack_lock_acquires_creates_file() {
        let dir = tempdir().unwrap();
        let mut plock = PackLock::open(dir.path()).unwrap();
        let expected = plock.path().to_path_buf();
        let _guard = plock.acquire().unwrap();
        assert!(expected.exists(), "open must create the sidecar file");
        assert_eq!(expected, dir.path().join(PACK_LOCK_FILE_NAME));
    }

    #[test]
    fn pack_lock_second_try_acquire_reports_busy_while_held() {
        let dir = tempdir().unwrap();
        let mut first = PackLock::open(dir.path()).unwrap();
        let _held = first.acquire().unwrap();
        let mut second = PackLock::open(dir.path()).unwrap();
        let err = second.try_acquire().unwrap_err();
        match err {
            PackLockError::Busy { path } => {
                assert_eq!(path, dir.path().join(PACK_LOCK_FILE_NAME));
            }
            other => panic!("expected Busy, got {other:?}"),
        }
    }

    #[test]
    fn pack_lock_release_on_drop() {
        let dir = tempdir().unwrap();
        {
            let mut first = PackLock::open(dir.path()).unwrap();
            let _g = first.acquire().unwrap();
        }
        let mut second = PackLock::open(dir.path()).unwrap();
        let _g = second.acquire().unwrap();
    }

    #[test]
    fn pack_lock_path_contains_pack_path() {
        let dir = tempdir().unwrap();
        let plock = PackLock::open(dir.path()).unwrap();
        let p = plock.path();
        assert!(p.starts_with(dir.path()));
        assert_eq!(p.file_name().and_then(|s| s.to_str()), Some(PACK_LOCK_FILE_NAME));
    }

    #[test]
    fn pack_lock_blocking_acquire_waits_for_holder() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let barrier = Arc::new(Barrier::new(2));
        let holder_barrier = Arc::clone(&barrier);
        let holder_path = path.clone();

        let holder = thread::spawn(move || {
            let mut lock = PackLock::open(&holder_path).unwrap();
            let _g = lock.acquire().unwrap();
            holder_barrier.wait();
            thread::sleep(Duration::from_millis(100));
        });

        barrier.wait();
        let start = Instant::now();
        let mut second = PackLock::open(&path).unwrap();
        let _g = second.acquire().unwrap();
        let waited = start.elapsed();
        holder.join().unwrap();
        assert!(
            waited >= Duration::from_millis(40),
            "blocking acquire must have waited (observed {waited:?})"
        );
    }

    #[test]
    fn pack_lock_distinct_paths_do_not_contend() {
        let a = tempdir().unwrap();
        let b = tempdir().unwrap();
        let mut la = PackLock::open(a.path()).unwrap();
        let _ga = la.acquire().unwrap();
        let mut lb = PackLock::open(b.path()).unwrap();
        let _gb = lb.try_acquire().unwrap();
    }

    #[tokio::test]
    async fn async_acquire_serialises_in_process() {
        // Two concurrent acquire_async calls on the same pack path
        // must serialise cleanly (no hang).
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let path_clone = path.clone();
        let h1 = tokio::spawn(async move {
            let lock = PackLock::open(&path).unwrap();
            let _hold = lock.acquire_async().await.unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
        });
        let h2 = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(5)).await;
            let lock = PackLock::open(&path_clone).unwrap();
            let _hold = lock.acquire_async().await.unwrap();
        });
        h1.await.unwrap();
        h2.await.unwrap();
    }

    // --- tier ordering (debug-only) -----------------------------------------

    // feat-m6 CI fix — tier enforcement now lives in a `tokio::task_local!`
    // stack, so these tests drive the check through a scoped task to
    // establish the frame. `try_with` outside a scope silently no-ops.

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn tier_strictly_increasing_ok() {
        tier::TIER_STACK
            .scope(std::cell::RefCell::new(Vec::new()), async {
                with_tier(Tier::Semaphore, || {
                    with_tier(Tier::PerPack, || {
                        with_tier(Tier::Backend, || {
                            with_tier(Tier::Manifest, || {});
                        });
                    });
                });
            })
            .await;
    }

    #[cfg(debug_assertions)]
    #[tokio::test]
    async fn tier_reversed_panics_in_debug() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let result = tier::TIER_STACK
            .scope(std::cell::RefCell::new(Vec::new()), async {
                catch_unwind(AssertUnwindSafe(|| {
                    with_tier(Tier::PerPack, || {
                        with_tier(Tier::Semaphore, || {});
                    });
                }))
            })
            .await;
        assert!(result.is_err(), "reversed tier order must panic in debug builds");
    }

    // --- acquire_cancellable (feat-m7-1 Stage 4) -----------------------------

    /// 4.T1 — uncontended path returns Ok(PackLockHold).
    #[tokio::test]
    async fn acquire_cancellable_happy_path() {
        let dir = tempdir().unwrap();
        let lock = PackLock::open(dir.path()).unwrap();
        let token = CancellationToken::new();
        let result = lock.acquire_cancellable(&token).await;
        assert!(result.is_ok(), "expected Ok(PackLockHold) on uncontended pack");
    }

    /// 4.T2 — task A holds the lock; task B's token fires after 10 ms;
    /// B must surface `Err(Cancelled)` within 50 ms.
    #[tokio::test]
    async fn acquire_cancellable_cancel_during_blocking_fd_lock_returns_cancelled() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let path_b = path.clone();

        // Task A: acquire and hold for 500 ms (long enough to cover B's window).
        let a_started = Arc::new(tokio::sync::Notify::new());
        let a_started_clone = Arc::clone(&a_started);
        let a = tokio::spawn(async move {
            let lock = PackLock::open(&path).unwrap();
            let _hold = lock.acquire_async().await.unwrap();
            a_started_clone.notify_one();
            tokio::time::sleep(Duration::from_millis(500)).await;
        });
        a_started.notified().await;

        let token = CancellationToken::new();
        let cancel_handle = token.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_handle.cancel();
        });

        let started = Instant::now();
        let lock_b = PackLock::open(&path_b).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            lock_b.acquire_cancellable(&token),
        )
        .await
        .expect("acquire_cancellable must return within 50 ms after cancel");

        let waited = started.elapsed();
        assert!(
            matches!(result, Err(PackLockErrorOrCancelled::Cancelled)),
            "expected Err(Cancelled), got {result:?} after {waited:?}"
        );

        canceller.await.unwrap();
        a.abort();
        let _ = a.await;
    }

    // --- helpers for 4.T3 / 4.T4 -------------------------------------------

    /// Spawn a "holder" task that acquires `path` and releases on
    /// signal. Returns `(JoinHandle, started_notify, release_notify)`.
    /// Caller awaits `started` to know the lock is held, then fires
    /// `release` when ready to let the holder drop its guard.
    fn spawn_holder(
        path: PathBuf,
    ) -> (
        tokio::task::JoinHandle<()>,
        Arc<tokio::sync::Notify>,
        Arc<tokio::sync::Notify>,
    ) {
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let started_c = Arc::clone(&started);
        let release_c = Arc::clone(&release);
        let h = tokio::spawn(async move {
            let lock = PackLock::open(&path).unwrap();
            let _hold = lock.acquire_async().await.unwrap();
            started_c.notify_one();
            release_c.notified().await;
        });
        (h, started, release)
    }

    /// Poll `acquire_async` against `path` until it succeeds or the
    /// outer deadline expires. Returns `Ok(())` on success, `Err(())`
    /// on timeout.
    async fn poll_acquire_until_free(path: PathBuf, deadline: Duration) -> Result<(), ()> {
        tokio::time::timeout(deadline, async move {
            loop {
                let lock = PackLock::open(&path).unwrap();
                if let Ok(Ok(_hold)) =
                    tokio::time::timeout(Duration::from_millis(100), lock.acquire_async()).await
                {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .map_err(|_| ())
    }

    /// 4.T3 — regression for the documented OS-thread leak window:
    /// after B is cancelled, A releases its lock; the spawn_blocking
    /// thread that B kicked off must eventually unblock and drop its
    /// guard. We observe this by polling from a third task C — if B's
    /// blocking thread leaked its guard, C would wait forever.
    #[tokio::test]
    async fn acquire_cancellable_spawn_blocking_thread_releases_guard_when_it_finally_unblocks() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (a, a_started, release_a) = spawn_holder(path.clone());
        a_started.notified().await;

        // Task B: race against cancel while A holds the fd-lock.
        let token = CancellationToken::new();
        let cancel_handle = token.clone();
        let path_b = path.clone();
        let b = tokio::spawn(async move {
            let lock = PackLock::open(&path_b).unwrap();
            lock.acquire_cancellable(&token).await
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_handle.cancel();
        let b_result = tokio::time::timeout(Duration::from_millis(100), b)
            .await
            .expect("B must resolve quickly after cancel")
            .expect("B task panicked");
        assert!(
            matches!(b_result, Err(PackLockErrorOrCancelled::Cancelled)),
            "expected B to see Cancelled, got {b_result:?}"
        );

        // Release A — B's parked OS thread should drain.
        release_a.notify_one();
        a.await.unwrap();

        assert!(
            poll_acquire_until_free(path, Duration::from_millis(2_000)).await.is_ok(),
            "task C never acquired — spawn_blocking thread leaked its fd-lock guard"
        );
    }

    /// 4.T4 — covers the outer-mutex cancel arm of the `select!` in
    /// `acquire_cancellable`. Task A holds the in-process async mutex
    /// (via `acquire_async`, which acquires both tiers); task B calls
    /// `acquire_cancellable` and is parked on `lock_owned()` (NOT yet
    /// at the fd-lock blocking call). Cancelling B's token must
    /// short-circuit the mutex wait and return `Err(Cancelled)`.
    #[tokio::test]
    async fn acquire_cancellable_cancel_during_async_mutex_wait_returns_cancelled() {
        let dir = tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let (a, a_started, release_a) = spawn_holder(path.clone());
        a_started.notified().await;

        let token = CancellationToken::new();
        let cancel_handle = token.clone();
        let canceller = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_handle.cancel();
        });

        let lock_b = PackLock::open(&path).unwrap();
        let result = tokio::time::timeout(
            Duration::from_millis(50),
            lock_b.acquire_cancellable(&token),
        )
        .await
        .expect("acquire_cancellable must return within 50 ms after cancel");

        assert!(
            matches!(result, Err(PackLockErrorOrCancelled::Cancelled)),
            "expected Err(Cancelled) from outer-mutex cancel arm, got {result:?}"
        );

        canceller.await.unwrap();
        release_a.notify_one();
        a.await.unwrap();
    }
}
