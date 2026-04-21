//! Bounded parallel scheduler for `grex sync` — feat-m6-1.
//!
//! Thin wrapper around [`tokio::sync::Semaphore`] that caps in-flight pack
//! operations across the process. This module only owns the permit pool and
//! exposes the handle surface; actual `acquire()` sites (where the permit is
//! paired with a per-pack `.grex-lock`) land in feat-m6-2.
//!
//! ## Semantics
//!
//! * `Scheduler::new(0)` → unbounded (`Semaphore::MAX_PERMITS`). The CLI
//!   surfaces this as `--parallel 0`; it is the documented "no cap" sentinel.
//! * `Scheduler::new(1)` → serial. One permit in flight; preserves the
//!   pre-M6 wall-order for operators who opt out of parallelism.
//! * `Scheduler::new(N)` for `N >= 2` → bounded parallel with `N` permits.
//!
//! ## Why `Arc<Semaphore>` rather than `Semaphore` directly
//!
//! `ExecCtx` threads the scheduler through `async` plugin dispatch; plugins
//! may `Arc::clone` the inner permits handle into spawned sub-tasks (for
//! future meta-pack parallelisation). Owning the semaphore behind an `Arc`
//! lets every acquire site share the same permit pool without ceremony.
//!
//! The `Scheduler` struct itself is typically wrapped in an `Arc` by the
//! caller (`sync::run`) so the `ExecCtx::scheduler` slot can be a
//! `&'a Arc<Scheduler>` — see `concurrency.md` §Scheduler pseudocode.

use std::sync::Arc;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Bounded parallel scheduler — caps concurrent pack operations.
///
/// Construction is cheap (one `Arc` + one `Semaphore` allocation). The
/// struct is `Clone`-free on purpose: callers clone the inner [`Arc`] via
/// [`Scheduler::permits`] or wrap the whole struct in their own `Arc`.
///
/// ## Example
///
/// ```no_run
/// use std::sync::Arc;
/// use grex_core::scheduler::Scheduler;
///
/// # async fn example() {
/// let s = Arc::new(Scheduler::new(4));
/// let _permit = s.acquire().await;
/// // …do work…
/// # }
/// ```
#[derive(Debug)]
pub struct Scheduler {
    permits: Arc<Semaphore>,
    max: usize,
}

impl Scheduler {
    /// Construct a scheduler with `parallel` permits.
    ///
    /// `parallel == 0` maps to [`Semaphore::MAX_PERMITS`] — the tokio-native
    /// "effectively unbounded" sentinel. `parallel == 1` yields the serial
    /// fast-path. `parallel >= 2` is bounded parallel.
    ///
    /// Values above [`Semaphore::MAX_PERMITS`] are clamped (cannot occur in
    /// practice; included for defense-in-depth against `usize::MAX` inputs).
    #[must_use]
    pub fn new(parallel: usize) -> Self {
        let max = if parallel == 0 {
            // feat-m6 H4 — non-fatal warning so operators who set
            // `--parallel 0` (or `GREX_PARALLEL=0`) know they've
            // opted into effectively-unbounded concurrency. The
            // cap-free mode is documented but easy to footgun on
            // shared hosts.
            tracing::warn!(
                target: "grex::scheduler",
                max_permits = Semaphore::MAX_PERMITS,
                "scheduler: parallel=0 → unbounded; set --parallel N to cap concurrency"
            );
            Semaphore::MAX_PERMITS
        } else {
            parallel.min(Semaphore::MAX_PERMITS)
        };
        Self { permits: Arc::new(Semaphore::new(max)), max }
    }

    /// Clone of the inner permits handle. Every clone shares the same
    /// permit pool. Returned as an owned [`Arc`] so consumers can pass it
    /// into spawned tasks without borrowing from `self`.
    #[must_use]
    pub fn permits(&self) -> Arc<Semaphore> {
        Arc::clone(&self.permits)
    }

    /// Acquire one permit. The returned [`OwnedSemaphorePermit`] releases
    /// its slot on `Drop`. Awaits if the pool is saturated.
    ///
    /// # Panics
    ///
    /// Panics only if the underlying semaphore has been closed, which
    /// this crate never does (no `Scheduler::close` API is exposed).
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.permits).acquire_owned().await.expect("scheduler semaphore never closes")
    }

    /// Maximum parallelism — useful for diagnostics and tests.
    #[must_use]
    pub fn max_parallelism(&self) -> usize {
        self.max
    }
}
