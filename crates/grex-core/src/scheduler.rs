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
use tokio_util::sync::CancellationToken;

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

    /// Acquire one permit, racing against a [`CancellationToken`].
    ///
    /// Resolves to:
    /// * `Ok(permit)` — a permit was granted before cancellation fired.
    /// * `Err(Cancelled)` — the token fired (or was already fired) before
    ///   a permit could be granted.
    ///
    /// Drop-safe: if the returned future is dropped before either branch
    /// resolves, the in-progress `acquire_owned` future is dropped too,
    /// removing the waiter from the semaphore wait list and leaving the
    /// permit pool intact (verified by `acquire_cancellable_dropped_future_does_not_leak_permit`).
    ///
    /// `biased` ensures the cancellation branch is polled first, so a
    /// pre-fired token never accidentally consumes a permit.
    pub async fn acquire_cancellable(
        &self,
        cancel: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, Cancelled> {
        tokio::select! {
            biased;
            () = cancel.cancelled() => Err(Cancelled),
            permit = Arc::clone(&self.permits).acquire_owned() => {
                permit.map_err(|_| Cancelled)
            }
        }
    }
}

/// Returned by [`Scheduler::acquire_cancellable`] when the supplied
/// [`CancellationToken`] fires before a permit is granted.
///
/// Carries no payload — the cancellation source is owned by the caller and
/// they already know why they cancelled. Verb bodies translate this into
/// the appropriate `PluginError::Cancelled` at the call site (feat-m7-1
/// Stages 6-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    /// 3.T1 — uncontended path returns Ok(permit).
    #[tokio::test]
    async fn acquire_cancellable_returns_permit_when_not_cancelled() {
        let s = Scheduler::new(2);
        let token = CancellationToken::new();
        let result = s.acquire_cancellable(&token).await;
        assert!(result.is_ok(), "expected Ok(permit) on uncontended scheduler");
    }

    /// 3.T2 — 0-permit scheduler, cancel after 10 ms, must return
    /// Err(Cancelled) within 30 ms.
    #[tokio::test]
    async fn acquire_cancellable_returns_cancelled_if_token_fires_before_permit() {
        // A scheduler with 1 permit, but we drain it so the next acquire
        // blocks indefinitely. (Scheduler::new(0) means *unbounded* per
        // module docs — that would never block.)
        let s = Scheduler::new(1);
        let _hold = s.acquire().await;

        let token = CancellationToken::new();
        let cancel_handle = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(10)).await;
            cancel_handle.cancel();
        });

        let result = tokio::time::timeout(
            Duration::from_millis(30),
            s.acquire_cancellable(&token),
        )
        .await
        .expect("acquire_cancellable must resolve within 30 ms after cancel");

        assert!(matches!(result, Err(Cancelled)), "expected Err(Cancelled)");
    }

    /// 3.T3 — dropped futures must not leak permits. 4 permits, 100
    /// waiters each with their own token, cancel all → available == 4.
    #[tokio::test]
    async fn acquire_cancellable_dropped_future_does_not_leak_permit() {
        let s = Arc::new(Scheduler::new(4));

        // Drain the 4 permits so all 100 waiters are forced into the
        // pending branch of the select.
        let _h1 = s.acquire().await;
        let _h2 = s.acquire().await;
        let _h3 = s.acquire().await;
        let _h4 = s.acquire().await;

        let mut tokens = Vec::with_capacity(100);
        let mut handles = Vec::with_capacity(100);
        for _ in 0..100 {
            let token = CancellationToken::new();
            let token_for_task = token.clone();
            let sched = Arc::clone(&s);
            handles.push(tokio::spawn(async move {
                let _ = sched.acquire_cancellable(&token_for_task).await;
            }));
            tokens.push(token);
        }

        // Give the waiters a tick to register on the semaphore wait list.
        tokio::time::sleep(Duration::from_millis(5)).await;

        for token in &tokens {
            token.cancel();
        }
        for h in handles {
            h.await.expect("waiter task panicked");
        }

        // Drop the four held permits, returning the pool to 4 free.
        drop(_h1);
        drop(_h2);
        drop(_h3);
        drop(_h4);

        assert_eq!(
            s.permits.available_permits(),
            4,
            "cancelled waiters must not leak permits"
        );
    }

    /// 3.T4 — cancelling after a successful acquire is a no-op; the
    /// permit remains valid until it is dropped.
    #[tokio::test]
    async fn acquire_cancellable_cancel_after_success_is_no_op() {
        let s = Scheduler::new(1);
        let token = CancellationToken::new();
        let permit = s.acquire_cancellable(&token).await.expect("permit");

        // Cancelling now must not affect the already-granted permit.
        token.cancel();
        assert_eq!(s.permits.available_permits(), 0, "permit still held");

        drop(permit);
        assert_eq!(s.permits.available_permits(), 1, "permit released on drop");
    }
}
