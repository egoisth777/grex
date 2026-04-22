//! L4 stress harness — concurrent saturation guards (feat-m7-2 Stage 6).
//!
//! Owned by `feat-m7-2-mcp-test-harness`. Stage 6 lands the **single**
//! failing case `stress_100x11_no_oversubscription` (RED). Stage 7 makes
//! it green by gating the MCP `sync` handler on
//! `ServerState::scheduler::acquire_cancellable(...)` so the in-flight
//! population is bounded by `--parallel` (today the MCP edge skips the
//! scheduler entirely — every concurrent `tools/call sync` enters
//! `run_with_cancel` and reaches the test-only barrier hook
//! simultaneously, so the high-water mark equals the request count).
//!
//! The whole binary is gated behind the `test-hooks` cargo feature
//! (mirrors `tests/cancellation.rs`); a plain `cargo test -p grex-mcp`
//! skips it. Run via `cargo test -p grex-mcp --features test-hooks
//! --test stress` (or `--all-features`).
//!
//! ## Barrier protocol
//!
//! Test installs a `tokio::sync::Barrier::new(N + 1)` where N is the
//! total spawned handlers (= 1100 = 100 × 11 per spec §L4). Each handler
//! invocation hits `tools::sync::__test_set_stress_barrier`'s installed
//! barrier inside `run_with_cancel` AFTER bumping the in-flight counter
//! and refreshing the monotone high-water mark. Once **all** N handlers
//! have arrived, the test thread does its own `Barrier::wait()` (the
//! `+1` slot), which releases every pinned handler simultaneously. At
//! that instant `__test_stress_high_water()` reflects the max number of
//! handlers ever in-flight at once — exactly the saturation contract
//! the spec §L4 invariant pair asserts:
//!
//! - `high_water >= PARALLEL` (we actually saturated, not under-loaded).
//! - `high_water <= PARALLEL` (we never over-subscribed past the limit).
//!
//! The two inequalities are asserted separately so a regression's
//! diagnostic localises to the specific direction (under-saturation vs
//! over-subscription) on the first failing run.
//!
//! ## RED-state expectation (Stage 6)
//!
//! Today the MCP `sync` handler does NOT acquire a `Scheduler` permit
//! at the MCP edge — `run_with_cancel` jumps straight from the
//! cancellation-hook check into `spawn_blocking(sync::run)`. Result:
//! every concurrent `tools/call sync` enters the body in lock-step,
//! the high-water mark equals N (= 1100), and the upper-bound assertion
//! `high_water <= PARALLEL (= 4)` fails loudly. Stage 7 wires the
//! permit-acquire to fix this; Stage 8 recalibrates the wall-clock
//! budget from CI p99.

#![cfg(feature = "test-hooks")]

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::Barrier;

use grex_mcp::{tools, GrexMcpServer, ServerState};

/// `--parallel` setting baked into the test fixture's `Scheduler`. Held
/// at 4 (rather than 1) so both inequalities in the saturation
/// invariant pair can fail independently — `high_water >= PARALLEL`
/// catches under-load, `high_water <= PARALLEL` catches over-subscription.
/// Stage 8 may revisit if CI capacity argues for a different value.
const PARALLEL: usize = 4;

/// Total concurrent in-flight `tools/call sync` invocations. Per spec
/// §L4 — "100 clients × 11 verbs"; today only `sync` carries the
/// barrier hook, so all 1100 calls are `sync`. Stage 7 will distribute
/// across all 11 verbs as the per-verb hooks land.
const N_TOTAL: usize = 100 * 11;

/// Initial wall-clock budget (placeholder per Stage 6 §6.3). Stage 8
/// recalibrates from the first CI green run's p99 × 1.5 — see
/// `openspec/changes/feat-m7-2-mcp-test-harness/tasks.md` §8.6.
// TODO(feat-m7-2 Stage 8.4 / §8.6): recalibrate from CI p99 × 1.5.
const STRESS_BUDGET: Duration = Duration::from_millis(5_000);

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stress_100x11_no_oversubscription() {
    // Reset the global high-water + in-flight counters so prior tests
    // (or a stale process re-run) cannot bleed into this run.
    tools::sync::__test_reset_stress_metrics();

    // `Barrier::new(N + 1)` — N handlers + the test thread. Once all
    // N handlers have parked at `b.wait().await`, the test thread's
    // `b.wait()` releases the herd. Snapshot of `high_water` taken
    // immediately afterwards reflects exactly the max simultaneous
    // in-flight handler population.
    let barrier = Arc::new(Barrier::new(N_TOTAL + 1));
    tools::sync::__test_set_stress_barrier(Some(barrier.clone()));

    // One shared server (per spec §L4 — "all clients share one
    // Arc<Server>"). Custom `ServerState` overrides the default
    // single-permit `for_tests()` scheduler so the saturation contract
    // can fail in either direction.
    let workspace = TempDir::new().expect("alloc test workspace tempdir");
    let state = ServerState::new(
        grex_core::Scheduler::new(PARALLEL),
        grex_core::Registry::default(),
        workspace.path().join("grex.jsonl"),
        workspace.path().to_path_buf(),
    );
    let server = GrexMcpServer::new(state);

    // Spawn N disjoint duplex client/server pairs against fresh server
    // *clones* — `GrexMcpServer` is `Clone` (state is all `Arc`); each
    // clone shares the same `Arc<Scheduler>` so the permit pool is
    // global to the test, exactly as a real session would see.
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

    // Release the herd. If the production code is correctly gated by
    // `Scheduler::acquire`, only PARALLEL handlers ever park at the
    // barrier — the remaining (N - PARALLEL) queue at the permit and
    // never reach the barrier counter, so `Barrier::wait(N+1)` would
    // deadlock. Stage 6 RED-state: no permit gate, so all N reach the
    // barrier and `wait()` returns; the upper-bound assertion below
    // then fires because `high_water == N != PARALLEL`. Stage 7 will
    // change the protocol (Barrier sized PARALLEL+1) once the gate is
    // in place.
    let release = tokio::time::timeout(STRESS_BUDGET, barrier.wait());
    release.await.expect(
        "barrier release exceeded STRESS_BUDGET — handlers never reached the in-handler hook \
         (check that #[cfg(any(test, feature = \"test-hooks\"))] gate compiled in)",
    );

    let high_water = tools::sync::__test_stress_high_water();

    // Drain the spawned handlers so the test does not leak tasks.
    for h in handlers {
        let _ = tokio::time::timeout(STRESS_BUDGET, h).await;
    }

    // Tear down the global barrier so subsequent tests (Stage 7+ adds
    // more cases) start from a clean slate.
    tools::sync::__test_set_stress_barrier(None);

    // Saturation invariant pair (per spec §L4). Both asserted
    // separately so a regression localises to the specific direction.
    assert!(
        high_water >= PARALLEL,
        "scheduler under-saturated: high_water={high_water} < PARALLEL={PARALLEL}",
    );
    assert!(
        high_water <= PARALLEL,
        "scheduler over-subscribed: high_water={high_water} > PARALLEL={PARALLEL} \
         (Stage 6 RED expectation — Stage 7 wires the permit-acquire gate at the MCP edge)",
    );
}

/// Drive one `initialize` → `notifications/initialized` → `tools/call
/// sync` exchange against `server` over a fresh duplex pair. Built in
/// this file (not via `tests/common/mod.rs::Client`) because the L4
/// case needs the server-side handle to share an `Arc<Scheduler>` —
/// `new_duplex_server()` constructs a fresh `ServerState::for_tests()`
/// per call, defeating the point.
async fn drive_one_sync(server: GrexMcpServer, pack_root: std::path::PathBuf, request_idx: usize) {
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

    // We do NOT await the response here — the handler is wedged at the
    // barrier, and the test thread will release it via `barrier.wait()`.
    // Once released, the response arrives with `isError=true` (the
    // pack root does not exist), but the stress assertion has already
    // captured `high_water` so the body is irrelevant. Drop the writer
    // to signal shutdown after the response drains.
    let _ = tokio::time::timeout(STRESS_BUDGET, read_frame_with_id(&mut reader, 2)).await;

    let _ = writer.shutdown().await;
    drop(writer);
    drop(reader);
    let _ = tokio::time::timeout(STRESS_BUDGET, server_task).await;

    // Tag the request idx into a log on failure paths if needed.
    let _ = request_idx;
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
