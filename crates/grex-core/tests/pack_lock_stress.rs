//! feat-m6-2 stress tests for [`grex_core::pack_lock::PackLock`].
//!
//! Exercises three high-level contracts:
//!
//! 1. 100 parallel tasks on the **same** pack serialize at the lock —
//!    peak concurrent holders == 1.
//! 2. 100 parallel tasks on **distinct** packs complete without
//!    serialization within a bounded wall-clock.
//! 3. Overlapping pack trees (shared children across two walks) do not
//!    deadlock — `tokio::timeout` wraps every acquire as a safety net.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use grex_core::pack_lock::PackLock;
use tempfile::TempDir;

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stress_100_parallel_same_pack_serialized() {
    let tmp = TempDir::new().unwrap();
    let pack_root: PathBuf = tmp.path().to_path_buf();

    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_observed = Arc::new(AtomicUsize::new(0));

    let mut handles = Vec::with_capacity(100);
    for _ in 0..100_usize {
        let pack_root = pack_root.clone();
        let in_flight = Arc::clone(&in_flight);
        let max_observed = Arc::clone(&max_observed);
        handles.push(tokio::spawn(async move {
            let lock = PackLock::open(&pack_root).expect("open");
            let _hold = lock.acquire_async().await.expect("acquire");

            // Record concurrency inside the critical section.
            let cur = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            max_observed.fetch_max(cur, Ordering::SeqCst);

            // Simulate a tiny bit of work so contention is real.
            tokio::time::sleep(Duration::from_millis(1)).await;

            in_flight.fetch_sub(1, Ordering::SeqCst);
        }));
    }

    for h in handles {
        h.await.expect("task joined");
    }
    assert_eq!(
        max_observed.load(Ordering::SeqCst),
        1,
        "same pack must be held by exactly one task at a time"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn stress_100_parallel_distinct_packs_no_contention() {
    // 100 tempdirs, one acquire each. Must complete quickly — no
    // serialization should occur across distinct packs.
    let tmps: Vec<TempDir> = (0..100).map(|_| TempDir::new().unwrap()).collect();

    let started = std::time::Instant::now();
    let mut handles = Vec::with_capacity(100);
    for tmp in &tmps {
        let pack_root = tmp.path().to_path_buf();
        handles.push(tokio::spawn(async move {
            let lock = PackLock::open(&pack_root).expect("open");
            let _hold = lock.acquire_async().await.expect("acquire");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }));
    }
    for h in handles {
        h.await.expect("task joined");
    }
    let elapsed = started.elapsed();
    // If distinct packs were serializing (~5ms each × 100) the total
    // would approach 500 ms. A generous upper bound of 2 seconds
    // catches any regression that collapses distinct packs onto one
    // lock.
    assert!(elapsed < Duration::from_secs(2), "distinct packs must not serialize: {elapsed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_overlapping_trees_no_deadlock() {
    // Two "trees": A = [p1, p2, p3], B = [p2, p3, p4]. The shared
    // p2 and p3 must serialize across the trees without deadlock.
    let p1 = TempDir::new().unwrap();
    let p2 = TempDir::new().unwrap();
    let p3 = TempDir::new().unwrap();
    let p4 = TempDir::new().unwrap();

    let tree_a = [p1.path().to_path_buf(), p2.path().to_path_buf(), p3.path().to_path_buf()];
    let tree_b = [p2.path().to_path_buf(), p3.path().to_path_buf(), p4.path().to_path_buf()];

    async fn walk(packs: [PathBuf; 3]) {
        for pack in packs {
            let lock = PackLock::open(&pack).expect("open");
            let _hold = lock.acquire_async().await.expect("acquire");
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    }

    let run = async {
        let (a, b) = tokio::join!(walk(tree_a), walk(tree_b));
        let _ = (a, b);
    };
    tokio::time::timeout(Duration::from_secs(10), run)
        .await
        .expect("overlapping trees must complete — no deadlock");
}
