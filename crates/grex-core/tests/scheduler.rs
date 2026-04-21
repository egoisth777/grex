//! feat-m6-1 — unit tests for the bounded scheduler.
//!
//! These tests cover the public surface of `grex_core::scheduler::Scheduler`:
//!
//! 1. `new(N)` reports `max_parallelism() == N` (bounded).
//! 2. `new(0)` reports `Semaphore::MAX_PERMITS` (unbounded sentinel).
//! 3. Saturation: once all permits are out, `try_acquire_owned` on the
//!    permits handle returns `Err`; dropping a held permit lets a pending
//!    `acquire()` future complete.
//! 4. FIFO fairness: tokio's `Semaphore` is FIFO under contention; three
//!    waiters drain in submission order.
//! 5. 100-waiter stress under an 8-permit cap completes without panic.
//! 6. `permits()` returns a shared `Arc<Semaphore>` — the same pool,
//!    not a fresh allocation.
//!
//! Saturation is asserted via `try_acquire_owned()` on the raw permits
//! handle rather than `FutureExt::now_or_never`, so the test suite stays
//! free of a `futures` dev-dep.

use std::sync::Arc;
use std::time::Duration;

use grex_core::scheduler::Scheduler;
use tokio::sync::Semaphore;
use tokio::time::timeout;

#[test]
fn scheduler_new_bounded_has_n_permits() {
    let s = Scheduler::new(4);
    assert_eq!(s.max_parallelism(), 4);
}

#[test]
fn scheduler_new_zero_is_unbounded() {
    let s = Scheduler::new(0);
    assert_eq!(
        s.max_parallelism(),
        Semaphore::MAX_PERMITS,
        "parallel == 0 must map to Semaphore::MAX_PERMITS"
    );
}

#[tokio::test]
async fn scheduler_acquire_blocks_when_saturated() {
    let s = Arc::new(Scheduler::new(2));
    let p1 = s.acquire().await;
    let p2 = s.acquire().await;

    // With both permits held, `try_acquire_owned` on the shared permits
    // handle must fail — this is the semaphore-native saturation probe.
    let permits = s.permits();
    assert!(
        permits.clone().try_acquire_owned().is_err(),
        "saturated scheduler must refuse try_acquire"
    );

    // Release one permit; a fresh acquire must now succeed within a short
    // bounded timeout.
    drop(p1);
    let p3 = timeout(Duration::from_secs(1), s.acquire())
        .await
        .expect("acquire must resolve once a permit is released");

    drop(p2);
    drop(p3);
}

#[tokio::test]
async fn scheduler_acquire_is_fifo_fair() {
    // 1-permit scheduler with three sequentially-submitted waiters; the
    // tokio semaphore is strictly FIFO so completion order == submission.
    let s = Arc::new(Scheduler::new(1));
    let held = s.acquire().await;

    let s1 = Arc::clone(&s);
    let s2 = Arc::clone(&s);
    let s3 = Arc::clone(&s);

    let order = Arc::new(tokio::sync::Mutex::new(Vec::<u32>::new()));
    let (o1, o2, o3) = (Arc::clone(&order), Arc::clone(&order), Arc::clone(&order));

    let t1 = tokio::spawn(async move {
        let _p = s1.acquire().await;
        o1.lock().await.push(1);
    });
    // Yield so t1 is queued before t2 / t3 enqueue.
    tokio::task::yield_now().await;
    let t2 = tokio::spawn(async move {
        let _p = s2.acquire().await;
        o2.lock().await.push(2);
    });
    tokio::task::yield_now().await;
    let t3 = tokio::spawn(async move {
        let _p = s3.acquire().await;
        o3.lock().await.push(3);
    });
    tokio::task::yield_now().await;

    drop(held);

    timeout(Duration::from_secs(2), async {
        let _ = tokio::join!(t1, t2, t3);
    })
    .await
    .expect("all waiters drain under FIFO");

    let guard = order.lock().await;
    assert_eq!(*guard, vec![1_u32, 2, 3], "tokio semaphore is FIFO");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scheduler_100_parallel_no_panic() {
    let s: Arc<Scheduler> = Arc::new(Scheduler::new(8));
    let mut handles = Vec::with_capacity(100);
    for _ in 0..100_u32 {
        let s: Arc<Scheduler> = Arc::clone(&s);
        handles.push(tokio::spawn(async move {
            let _p = s.acquire().await;
            // Simulate a trivial unit of work.
            tokio::time::sleep(Duration::from_millis(1)).await;
        }));
    }
    for h in handles {
        h.await.expect("task joins without panic");
    }
}

#[tokio::test]
async fn scheduler_permits_clone_is_shared() {
    // feat-m6-1 spec §Test plan: acquiring from a cloned `Arc<Semaphore>`
    // drains the same pool — proves the permit handle is shared, not a
    // new per-call allocation.
    let s = Arc::new(Scheduler::new(1));
    let permits_a = s.permits();
    let permits_b = s.permits();

    assert!(Arc::ptr_eq(&permits_a, &permits_b), "permits() returns the same Arc across calls");

    // Drain from clone A; clone B must then refuse try_acquire.
    let p = permits_a.acquire_owned().await.expect("clone A acquires");
    assert!(
        permits_b.clone().try_acquire_owned().is_err(),
        "clone B must not acquire while clone A holds the only permit"
    );
    drop(p);
    // Clone B now succeeds.
    let _permit = timeout(Duration::from_secs(1), permits_b.acquire_owned())
        .await
        .expect("clone B resolves once clone A releases")
        .expect("permit acquisition succeeds");
}
