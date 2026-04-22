//! L4 stress harness — concurrent saturation guards (feat-m7-2 Stages 6 + 7).
//!
//! Owned by `feat-m7-2-mcp-test-harness`. Stage 6 landed the **single**
//! failing case `stress_100x11_no_oversubscription` (RED). Stage 7 turns
//! it green by gating the MCP `sync` handler on
//! `ServerState::scheduler::acquire_cancellable(...)` so the in-flight
//! population is bounded by `--parallel`. Stage 7 also adds two more
//! cases:
//!
//! - `stress_same_pack_serialises` — 8 concurrent `tools/call sync`
//!   against the SAME pack root. Asserts no-deadlock + bounded latency
//!   under same-pack contention. Strict serialisation observation
//!   (interleave-free critical sections) requires grex-core
//!   instrumentation and is deferred to feat-m7-3+ (see spec
//!   §"Known limitations" entry 6).
//! - `stress_no_deadlock_across_3_iterations` — wraps the saturation
//!   body in a 3-iter loop with metrics + barrier reset between
//!   iterations. Mirrors the spec §L4 CI repeat policy in-code.
//!
//! The whole binary is gated behind the `test-hooks` cargo feature
//! (mirrors `tests/cancellation.rs`); a plain `cargo test -p grex-mcp`
//! skips it. Run via `cargo test -p grex-mcp --features test-hooks
//! --test stress` (or `--all-features`).
//!
//! ## Barrier protocol (Stage 7)
//!
//! Test installs a `tokio::sync::Barrier::new(PARALLEL + 1)` — only
//! the PARALLEL handlers that hold a scheduler permit reach the
//! barrier; the remaining `N - PARALLEL` queue at
//! `Scheduler::acquire_cancellable` and never bump the in-flight
//! counter. The test thread does its own `Barrier::wait()` (the
//! `+1` slot), which releases every pinned handler simultaneously.
//! At that instant `__test_stress_high_water()` reflects the max
//! number of handlers ever in-flight at once — exactly the saturation
//! contract the spec §L4 invariant pair asserts:
//!
//! - `high_water >= PARALLEL` (we actually saturated, not under-loaded).
//! - `high_water <= PARALLEL` (we never over-subscribed past the limit).
//!
//! The two inequalities are asserted separately so a regression's
//! diagnostic localises to the specific direction (under-saturation vs
//! over-subscription) on the first failing run.
//!
//! ## Pacing the herd through the permit gate
//!
//! With `Barrier::new(PARALLEL + 1)`, the first PARALLEL handlers to
//! acquire a permit park at the barrier. The test thread releases
//! them, they exit, and only then can the next PARALLEL handlers
//! acquire permits — but those will not see a barrier (the test has
//! cleared it via `__test_set_stress_barrier(None)` after the first
//! release). They run their `spawn_blocking(sync::run)` against the
//! non-existent pack root, return `pack_op` envelopes, and drain.
//! High-water is captured immediately after the barrier releases, so
//! it reflects exactly the first saturated cohort.

#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Barrier;

use grex_mcp::{tools, GrexMcpServer, ServerState};

/// Process-wide serialisation guard. The stress hook in `tools::sync`
/// stores its barrier slot, IN_FLIGHT counter, and HIGH_WATER mark in
/// `static` atomics — shared across every test in this binary. Cargo
/// runs integration tests in parallel by default, which cross-pollutes
/// those globals. We serialise every stress case behind one
/// `tokio::sync::Mutex<()>` to keep the global state single-tenant.
///
/// `tokio::sync::Mutex` (rather than `std::sync::Mutex`) so the guard
/// can be held across `.await` points — clippy's `await_holding_lock`
/// lint correctly forbids that for `std::sync::Mutex`. The lock is
/// released at end-of-test when the guard drops.
static STRESS_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `--parallel` setting baked into the test fixture's `Scheduler`. Held
/// at 4 (rather than 1) so both inequalities in the saturation
/// invariant pair can fail independently — `high_water >= PARALLEL`
/// catches under-load, `high_water <= PARALLEL` catches over-subscription.
/// Stage 8 may revisit if CI capacity argues for a different value.
const PARALLEL: usize = 4;

/// Total concurrent in-flight `tools/call sync` invocations. Per spec
/// §L4 — "100 clients × 11 verbs"; today only `sync` carries the
/// barrier hook, so all 1100 calls are `sync`. Future stages may
/// distribute across all 11 verbs as the per-verb hooks land.
const N_TOTAL: usize = 100 * 11;

/// Per-pack contention fan-out for `stress_same_pack_serialises`. 8
/// matches spec §L4 ("8 concurrent tools/call sync against pack p1").
const SAME_PACK_FAN_OUT: usize = 8;

/// Repeat count for `stress_no_deadlock_across_3_iterations`. Spec §L4
/// CI repeat policy: each stress test runs 3× consecutively in one job.
const REPEAT_ITERATIONS: usize = 3;

/// Initial wall-clock budget (placeholder per Stage 6 §6.3). Stage 8
/// recalibrates from the first CI green run's p99 × 1.5 — see
/// `openspec/changes/feat-m7-2-mcp-test-harness/tasks.md` §8.6.
// TODO(feat-m7-2 Stage 8.4 / §8.6): recalibrate from CI p99 × 1.5.
const STRESS_BUDGET: Duration = Duration::from_millis(5_000);

/// Per-call budget for the same-pack contention test. Each call may
/// queue behind up to (SAME_PACK_FAN_OUT - 1) others through both the
/// scheduler permit and the in-`sync::run` per-pack mutex; the bound
/// is generous so transient CI jitter does not flake the test.
const SAME_PACK_BUDGET: Duration = Duration::from_millis(10_000);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_100x11_no_oversubscription() {
    let _serial = STRESS_SERIAL.lock().await;
    let high_water = run_saturation_iteration().await;
    assert_saturation_invariants(high_water);
}

/// Same-pack contention smoke test — 8 concurrent `tools/call sync`
/// against the SAME pack root.
///
/// **Contract (relaxed for m7-2)**: the test asserts no-deadlock +
/// bounded latency under same-pack contention. All 8 calls must
/// complete within `SAME_PACK_BUDGET` and return a structured
/// envelope (`isError=true` against the non-existent pack root is
/// expected and acceptable — the goal is "doesn't blow up", not
/// "returns OK").
///
/// **Strict serialisation observation deferred** to feat-m7-3+. The
/// spec §L4 originally called for `ActionStarted(p1, i+1)` strictly
/// follows `ActionCompleted(p1, i)` ordering, but that requires
/// instrumenting grex-core's `sync::run` (where the per-pack
/// `PackLock` actually serialises) — observing it from the MCP edge
/// would only see the scheduler permit-gate ordering, not the
/// pack-lock critical section ordering. See spec §"Known limitations"
/// entry 6.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_same_pack_serialises() {
    let _serial = STRESS_SERIAL.lock().await;

    // Reset any leftover stress metrics from prior tests so this case
    // does not double-count if the binary's tests run in alphabetical
    // order. No barrier installed — same-pack contention is observed
    // via wall-clock + envelope shape, not the barrier hook.
    tools::sync::__test_reset_stress_metrics();
    tools::sync::__test_set_stress_barrier(None);

    let workspace = TempDir::new().expect("alloc test workspace tempdir");
    let state = ServerState::new(
        // PARALLEL permits so the scheduler does not itself serialise
        // these 8 calls; the test specifically isolates the same-pack
        // dimension. With PARALLEL=4 < SAME_PACK_FAN_OUT=8, two waves
        // of permit-acquire still occur, but neither wave should
        // deadlock or exceed the budget.
        grex_core::Scheduler::new(PARALLEL),
        grex_core::Registry::default(),
        workspace.path().join("grex.jsonl"),
        workspace.path().to_path_buf(),
    );
    let server = GrexMcpServer::new(state);

    // Single shared pack_root — the contention dimension under test.
    let pack_root = workspace.path().join("p1-nonexistent");

    let started = Instant::now();
    let mut handlers = Vec::with_capacity(SAME_PACK_FAN_OUT);
    for i in 0..SAME_PACK_FAN_OUT {
        let server_i = server.clone();
        let pack_root = pack_root.clone();
        let h = tokio::spawn(async move {
            drive_one_sync_await_response(server_i, pack_root, i).await
        });
        handlers.push(h);
    }

    let mut completed = 0usize;
    for h in handlers {
        let frame = tokio::time::timeout(SAME_PACK_BUDGET, h)
            .await
            .expect("same-pack handler exceeded SAME_PACK_BUDGET — possible deadlock")
            .expect("same-pack handler task join error");
        // Envelope shape: must be a JSON-RPC response carrying either
        // `result` (CallToolResult — success or isError) or `error`
        // (top-level JSON-RPC error). Any malformed shape fails the
        // test loudly.
        assert!(
            frame.get("result").is_some() || frame.get("error").is_some(),
            "same-pack call returned neither result nor error: {frame}",
        );
        completed += 1;
    }
    assert_eq!(completed, SAME_PACK_FAN_OUT, "not all same-pack calls completed");
    let elapsed = started.elapsed();
    assert!(
        elapsed <= SAME_PACK_BUDGET,
        "same-pack saturation took {elapsed:?} > budget {SAME_PACK_BUDGET:?}",
    );
}

/// Spec §L4 CI repeat policy expressed in-code: run the saturation
/// body 3× back-to-back in one `#[tokio::test]`. Any iteration's
/// failure fails the whole test. Stress metrics + barrier slot are
/// reset between iterations inside `run_saturation_iteration`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_no_deadlock_across_3_iterations() {
    let _serial = STRESS_SERIAL.lock().await;

    for iteration in 0..REPEAT_ITERATIONS {
        let high_water = run_saturation_iteration().await;
        assert!(
            high_water >= PARALLEL,
            "iteration {iteration}: scheduler under-saturated: \
             high_water={high_water} < PARALLEL={PARALLEL}",
        );
        assert!(
            high_water <= PARALLEL,
            "iteration {iteration}: scheduler over-subscribed: \
             high_water={high_water} > PARALLEL={PARALLEL}",
        );
    }
}

// ---------- shared saturation body ----------

/// Run one full saturation iteration: install a fresh
/// `Barrier::new(PARALLEL + 1)`, spawn `N_TOTAL` concurrent
/// `tools/call sync` invocations, release the herd, snapshot
/// `high_water`, drain. Used by `stress_100x11_no_oversubscription`
/// (1×) and `stress_no_deadlock_across_3_iterations` (3×).
async fn run_saturation_iteration() -> usize {
    // Reset the global high-water + in-flight counters so prior
    // iterations / tests cannot bleed into this run.
    tools::sync::__test_reset_stress_metrics();

    // `Barrier::new(PARALLEL + 1)` — only PARALLEL handlers will
    // hold a permit and reach the barrier; the test thread is the
    // `+1` releaser. The remaining `N_TOTAL - PARALLEL` queue at
    // the scheduler permit and never park here. Once released, the
    // first cohort exits + drops permits, and the next cohort
    // acquires — but the barrier slot is cleared (post-release) so
    // they run straight through to `spawn_blocking(sync::run)`.
    let barrier = Arc::new(Barrier::new(PARALLEL + 1));
    tools::sync::__test_set_stress_barrier(Some(barrier.clone()));

    let workspace = TempDir::new().expect("alloc test workspace tempdir");
    let state = ServerState::new(
        grex_core::Scheduler::new(PARALLEL),
        grex_core::Registry::default(),
        workspace.path().join("grex.jsonl"),
        workspace.path().to_path_buf(),
    );
    let server = GrexMcpServer::new(state);

    let mut handlers = Vec::with_capacity(N_TOTAL);
    let pack_root = workspace.path().join("stress-sync-nonexistent");
    for i in 0..N_TOTAL {
        let server_i = server.clone();
        let pack_root = pack_root.clone();
        let h = tokio::spawn(async move {
            drive_one_sync(server_i, pack_root, i).await;
        });
        handlers.push(h);
    }

    // Release the first PARALLEL-sized cohort. They exit, drop their
    // permits, and the next cohort acquires — but we clear the
    // barrier slot before draining so subsequent cohorts run through
    // the no-op fast path of `stress_barrier_enter` (no installed
    // barrier → returns the guard immediately).
    let release = tokio::time::timeout(STRESS_BUDGET, barrier.wait());
    release.await.expect(
        "barrier release exceeded STRESS_BUDGET — handlers never reached the in-handler hook \
         (check that #[cfg(any(test, feature = \"test-hooks\"))] gate compiled in)",
    );
    let high_water = tools::sync::__test_stress_high_water();
    tools::sync::__test_set_stress_barrier(None);

    // Drain the spawned handlers so the iteration does not leak tasks.
    for h in handlers {
        let _ = tokio::time::timeout(STRESS_BUDGET, h).await;
    }

    high_water
}

fn assert_saturation_invariants(high_water: usize) {
    assert!(
        high_water >= PARALLEL,
        "scheduler under-saturated: high_water={high_water} < PARALLEL={PARALLEL}",
    );
    assert!(
        high_water <= PARALLEL,
        "scheduler over-subscribed: high_water={high_water} > PARALLEL={PARALLEL}",
    );
}

// ---------- duplex driver helpers ----------

/// Drive one `initialize` → `notifications/initialized` → `tools/call
/// sync` exchange against `server` over a fresh duplex pair. Built in
/// this file (not via `tests/common/mod.rs::Client`) because the L4
/// case needs the server-side handle to share an `Arc<Scheduler>` —
/// `new_duplex_server()` constructs a fresh `ServerState::for_tests()`
/// per call, defeating the point.
async fn drive_one_sync(server: GrexMcpServer, pack_root: std::path::PathBuf, request_idx: usize) {
    let _ = drive_one_sync_inner(server, pack_root, request_idx).await;
}

/// Same as [`drive_one_sync`] but returns the parsed `tools/call`
/// response frame so the caller can inspect envelope shape. Used by
/// `stress_same_pack_serialises` to assert each call returned a
/// structured response under the budget.
async fn drive_one_sync_await_response(
    server: GrexMcpServer,
    pack_root: std::path::PathBuf,
    request_idx: usize,
) -> Value {
    drive_one_sync_inner(server, pack_root, request_idx).await
}

async fn drive_one_sync_inner(
    server: GrexMcpServer,
    pack_root: std::path::PathBuf,
    request_idx: usize,
) -> Value {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_io).await;
    });

    let (read, write) = tokio::io::split(client_io);
    let mut reader = BufReader::new(read);
    let mut writer = write;

    // initialize
    let init = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "grex-mcp-stress", "version": "0.0.1" }
        }
    });
    write_frame(&mut writer, &init).await;
    let _ = read_frame_with_id(&mut reader, 1).await;

    // notifications/initialized
    let initialized = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    });
    write_frame(&mut writer, &initialized).await;

    // tools/call sync
    let call = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "sync",
            "arguments": {
                "packRoot": pack_root,
                "dryRun": true,
                "noValidate": true,
            }
        }
    });
    write_frame(&mut writer, &call).await;

    // Await the response. For the saturation test the handler is
    // wedged at the barrier; the test thread will release it via
    // `barrier.wait()` and the response then arrives with
    // `isError=true` (the pack root does not exist). For the
    // same-pack test there is no barrier — the handler runs straight
    // through, queues at PackLock inside `sync::run`, and eventually
    // returns. Either way, capturing the frame lets callers assert
    // envelope shape.
    let frame = tokio::time::timeout(STRESS_BUDGET, read_frame_with_id(&mut reader, 2))
        .await
        .unwrap_or(Value::Null);

    let _ = writer.shutdown().await;
    drop(writer);
    drop(reader);
    let _ = tokio::time::timeout(STRESS_BUDGET, server_task).await;

    // Tag the request idx into a log on failure paths if needed.
    let _ = request_idx;
    frame
}

async fn write_frame<W>(w: &mut W, frame: &Value)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut buf = serde_json::to_vec(frame).expect("serialise JSON-RPC frame");
    buf.push(b'\n');
    w.write_all(&buf).await.expect("write frame to duplex");
    w.flush().await.expect("flush duplex");
}

async fn read_frame_with_id<R>(r: &mut BufReader<R>, expected_id: i64) -> Value
where
    R: tokio::io::AsyncRead + Unpin,
{
    loop {
        let mut line = String::new();
        let n = r.read_line(&mut line).await.expect("read JSON-RPC line");
        if n == 0 {
            panic!("duplex EOF before response id={expected_id}");
        }
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let frame: Value =
            serde_json::from_str(trimmed).expect("frame is valid JSON-RPC line");
        if frame.get("id").and_then(Value::as_i64) == Some(expected_id) {
            return frame;
        }
    }
}
