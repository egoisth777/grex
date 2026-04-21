//! feat-m6-2 dispatch-level parallelism tests.
//!
//! Exercises the scheduler + per-pack lock combo under load:
//!
//! * `dispatch_respects_semaphore_bound` — an 8-permit scheduler with
//!   32 packs caps in-flight "plugin-method invocations" at 8.
//! * `dispatch_serial_with_parallel_1` — a 1-permit scheduler
//!   serialises every dispatch, matching the pre-M6 wall order.
//!
//! The tests simulate plugin dispatch without engaging the full
//! `MetaPlugin`/`DeclarativePlugin` surface — each spawned task
//! acquires a scheduler permit (tier 2) then a [`PackLock`] (tier 3)
//! and records its concurrency inside the critical section.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use grex_core::pack_lock::PackLock;
use grex_core::scheduler::Scheduler;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_respects_semaphore_bound() {
    let scheduler = Arc::new(Scheduler::new(8));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    // 32 distinct packs so there's no contention from the per-pack
    // lock — the semaphore is the only serialisation source under
    // test here.
    let tmps: Vec<TempDir> = (0..32).map(|_| TempDir::new().unwrap()).collect();

    let mut handles = Vec::with_capacity(32);
    for tmp in &tmps {
        let pack_root = tmp.path().to_path_buf();
        let sched = Arc::clone(&scheduler);
        let in_flight = Arc::clone(&in_flight);
        let peak = Arc::clone(&peak);
        handles.push(tokio::spawn(async move {
            // Tier 2 — semaphore permit.
            let _permit = sched.acquire().await;
            // Tier 3 — per-pack lock.
            let lock = PackLock::open(&pack_root).expect("open");
            let _hold = lock.acquire_async().await.expect("acquire");

            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(cur, Ordering::SeqCst);

            tokio::time::sleep(Duration::from_millis(5)).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for h in handles {
        h.await.expect("task joined");
    }
    let observed = peak.load(Ordering::SeqCst);
    assert!(
        observed <= 8,
        "peak concurrent dispatches {observed} must not exceed semaphore bound 8"
    );
    assert!(observed >= 2, "test fixture must actually exercise parallelism: {observed}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dispatch_serial_with_parallel_1() {
    let scheduler = Arc::new(Scheduler::new(1));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let tmps: Vec<TempDir> = (0..8).map(|_| TempDir::new().unwrap()).collect();

    let mut handles = Vec::with_capacity(8);
    for tmp in &tmps {
        let pack_root = tmp.path().to_path_buf();
        let sched = Arc::clone(&scheduler);
        let in_flight = Arc::clone(&in_flight);
        let peak = Arc::clone(&peak);
        handles.push(tokio::spawn(async move {
            let _permit = sched.acquire().await;
            let lock = PackLock::open(&pack_root).expect("open");
            let _hold = lock.acquire_async().await.expect("acquire");

            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(cur, Ordering::SeqCst);

            tokio::time::sleep(Duration::from_millis(2)).await;
            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for h in handles {
        h.await.expect("task joined");
    }
    assert_eq!(
        peak.load(Ordering::SeqCst),
        1,
        "--parallel 1 must produce serial dispatch — peak concurrency 1"
    );
}
