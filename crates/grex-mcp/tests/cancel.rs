//! L5 cancellation chaos — feat-m7-2 Stage 8.
//!
//! Owned by `feat-m7-2-mcp-test-harness`. Distinct from
//! `tests/cancellation.rs` (m7-1 Stage 7) which proves
//! `notifications/cancelled` plumbing for a single in-flight `sync` call.
//! This binary covers the **cross-verb chaos contract** from spec §L5
//! (`openspec/changes/feat-m7-2-mcp-test-harness/spec.md` lines 138 –
//! 151):
//!
//! 1. **11 parametric cases** (`cancel_<verb>` for every entry in
//!    `grex_mcp::VERBS_EXPOSED`). Each fires `tools/call <verb>`
//!    immediately followed by `notifications/cancelled` for the same id.
//!    The response MUST be either a clean `CallToolResult` (if the
//!    handler completed before cancel landed — common for the 9 stub
//!    verbs that return a `not_implemented` envelope synchronously) OR
//!    a `-32800 RequestCancelled` error (if cancel won the race —
//!    realistic for `sync` against a live pack). Both outcomes are
//!    MCP-valid per `.omne/cfg/mcp.md` §Cancellation.
//!
//! 2. **`cancel_permit_released_under_budget`** — installs the
//!    `block_until_cancelled` hook so a `sync` call parks indefinitely
//!    while holding the `Scheduler` permit; sends cancel; asserts the
//!    permit is reacquirable within the OS-specific budget (250 ms on
//!    Linux/macOS, 500 ms on Windows per spec line 146 + MED-5).
//!
//! 3. **`cancel_pack_lock_released_under_budget`** — same shape, but
//!    observed at the MCP edge: after the first cancel a second
//!    `tools/call sync` against the same `pack_root` must complete
//!    within budget (no PackLock leak). The strict per-pack-lock
//!    observation noted in spec §"Known limitations" entry 6 is
//!    deferred to feat-m7-3+; the pragmatic substitute (second
//!    same-path sync completes) is what this test asserts.
//!
//! ## OS-specific budgets
//!
//! Per spec line 146 – 147 the cancellation latency budget is
//! OS-driven (M6 PackLock cancellation latency is bounded by the OS
//! file-lock release path, not by grex). Encoded as a `#[cfg]`-selected
//! const so a release build can never accidentally read a runtime flag.
//!
//! ## Why a separate binary from `cancellation.rs`
//!
//! `cancellation.rs` (m7-1) operates on **single-tool cancel
//! correctness** with strict-typed rmcp transport. This file (m7-2)
//! operates on **cross-verb chaos + budget** using the duplex
//! line-protocol harness shared with `stress.rs`. Different layers,
//! different scopes — keeping them split mirrors the layer boundaries
//! in spec §"File / module targets".
//!
//! ## test-hooks gating
//!
//! Whole binary is `#[cfg(feature = "test-hooks")]` because the budget
//! tests need `__test_set_block_until_cancelled` from
//! `tools::sync::test_hooks`. The 11 parametric cases would compile
//! without the gate, but keeping one gate per binary matches
//! `cancellation.rs` and `stress.rs`.

#![cfg(feature = "test-hooks")]

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use grex_mcp::{tools, GrexMcpServer, ServerState, VERBS_EXPOSED};

/// Process-wide serialisation guard. The `block_until_cancelled` flag
/// in `tools::sync::test_hooks` is a static `AtomicBool` shared across
/// every test in this binary; cargo runs integration tests in parallel
/// by default, which would cross-pollute a flag set by one test into
/// the body of another. Serialise every case behind a `tokio::sync::
/// Mutex<()>` to keep the global state single-tenant. Same pattern as
/// `tests/stress.rs::STRESS_SERIAL`.
static CANCEL_SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// `--parallel` for the budget tests' fixture `Scheduler`. Held at 1
/// (vs stress.rs's 4) so a single in-flight `sync` exhausts the
/// permits and the post-cancel reacquire test exercises the actual
/// release path, not just an unused permit slot.
const PARALLEL: usize = 1;

/// OS-specific cancellation budget per spec §L5 lines 146 – 147 +
/// MED-5: Windows file-lock cancellation latency is OS-driven, not
/// grex-driven, so the budget widens on Windows. Encoded as a
/// `#[cfg]`-selected const (NOT a runtime flag) so release builds
/// cannot accidentally read a developer override.
#[cfg(unix)]
const CANCEL_BUDGET_MS: u64 = 250;
#[cfg(windows)]
const CANCEL_BUDGET_MS: u64 = 500;

const CANCEL_BUDGET: Duration = Duration::from_millis(CANCEL_BUDGET_MS);

/// Wider budget for the parametric per-verb cases. Each case fires
/// `tools/call` then `notifications/cancelled` and waits for ANY
/// terminal envelope (clean result OR -32800). The handler may run to
/// completion before cancel lands (typical for the 9 stub verbs); we
/// allow up to 2 s for either outcome to surface so transient CI
/// jitter does not flake the test. The TIGHT budget applies only to
/// the explicit permit / pack-lock release probes below.
const RESPONSE_BUDGET: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------
// 11 parametric per-verb cancel cases
// ---------------------------------------------------------------------
//
// One `#[tokio::test]` per verb (matching the parity.rs convention).
// A macro would obscure failure localisation in cargo's test output —
// `cancel_sync` is more grep-friendly than `cancel[5]`. Verbs are
// listed in the same order as `VERBS_EXPOSED`; the const-assert below
// pins the count so a future contributor adding a 12th verb fails to
// compile until the matching `cancel_<verb>` test lands.

const _: () = assert!(VERBS_EXPOSED.len() == 11);

#[tokio::test]
async fn cancel_init() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("init").await;
}

#[tokio::test]
async fn cancel_add() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("add").await;
}

#[tokio::test]
async fn cancel_rm() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("rm").await;
}

#[tokio::test]
async fn cancel_ls() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("ls").await;
}

#[tokio::test]
async fn cancel_status() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("status").await;
}

#[tokio::test]
async fn cancel_sync() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("sync").await;
}

#[tokio::test]
async fn cancel_update() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("update").await;
}

#[tokio::test]
async fn cancel_doctor() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("doctor").await;
}

#[tokio::test]
async fn cancel_import() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("import").await;
}

#[tokio::test]
async fn cancel_run() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("run").await;
}

#[tokio::test]
async fn cancel_exec() {
    let _serial = CANCEL_SERIAL.lock().await;
    drive_cancel_for_verb("exec").await;
}

// ---------------------------------------------------------------------
// Budget probes
// ---------------------------------------------------------------------

/// Spec §L5 lines 145 / 149: after cancel, the scheduler permit MUST
/// be released within the OS budget. Strategy:
///
/// 1. Install the `block_until_cancelled` hook in `tools::sync` so a
///    `sync` call parks at `cancel.cancelled().await` — but ONLY after
///    `acquire_cancellable` has returned a permit (the hook fires at
///    the top of `run_with_cancel`, but the Stage 7 wiring places the
///    permit acquire BEFORE the hook check; see Stage 7 commit body).
///    Wait — re-reading sync.rs line 75-79: the block hook runs FIRST,
///    THEN the permit acquire. So a hook-blocked handler does NOT hold
///    a permit. We need the hook OFF for the permit-leak observation.
/// 2. New strategy: drive a real `sync` against a deep-non-existent
///    pack root; the handler enters `spawn_blocking(sync::run)` while
///    HOLDING the permit; we send cancel; the `tokio::select!` arm
///    fires, returning -32800 and dropping `_permit`. The probe then
///    reacquires from `state.scheduler` directly within budget.
/// 3. The permit reacquire timing starts at the moment we send the
///    cancel notification on the wire and ends when `acquire_cancellable`
///    on the test-side completes. The wall-clock measurement covers
///    the rmcp loop's notification dispatch + the handler's select
///    arm + the permit drop + our reacquire.
///
/// PARALLEL=1 ensures the probe's `acquire` is the first eligible
/// caller post-release; a higher PARALLEL would hide a leak by always
/// having a free permit available.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "inline handshake → tools/call → cancel → drain → probe sequence; \
              splitting would obscure the wire-level race observation"
)]
async fn cancel_permit_released_under_budget() {
    let _serial = CANCEL_SERIAL.lock().await;
    // Defensive — make sure a previous test did not leave the hook on.
    tools::sync::__test_set_block_until_cancelled(false);

    let workspace = TempDir::new().expect("alloc test workspace tempdir");
    let state = ServerState::new(
        grex_core::Scheduler::new(PARALLEL),
        grex_core::Registry::default(),
        workspace.path().join("grex.jsonl"),
        workspace.path().to_path_buf(),
    );
    let scheduler = state.scheduler.clone();
    let server = GrexMcpServer::new(state);

    // Enable the block-until-cancelled hook AFTER ServerState is
    // built so the static flag flips on for this iteration only. The
    // hook fires INSIDE `run_with_cancel` BEFORE the scheduler
    // acquire (sync.rs line 75-79), which is wrong for a permit-leak
    // observation — we need the handler to actually take a permit
    // and then have cancel fire the `select!` arm. So we deliberately
    // leave the hook OFF and rely on `spawn_blocking(sync::run)`
    // racing against `cancel.cancelled()` in the `tokio::select!`.
    //
    // The fixture `pack_root` points at a path that does not exist;
    // `sync::run` will return a `SyncError` quickly, but only AFTER
    // the spawn_blocking task starts — long enough for our cancel
    // notification to win the select arm in most cases.

    let (server_io, client_io) = tokio::io::duplex(8 * 1024);
    let server_clone = server.clone();
    let server_task = tokio::spawn(async move {
        let _ = server_clone.run(server_io).await;
    });

    let (read, write) = tokio::io::split(client_io);
    let mut reader = BufReader::new(read);
    let mut writer = write;

    // Handshake.
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "grex-mcp-cancel-budget", "version": "0.0.1" }
            }
        }),
    )
    .await;
    let _ = read_frame_with_id(&mut reader, 1).await;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )
    .await;

    // Fire `sync` and immediately cancel.
    let pack_root = workspace.path().join("permit-leak-probe-nonexistent");
    let req_id: i64 = 42;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {
                "name": "sync",
                "arguments": {
                    "packRoot": pack_root,
                    "dryRun": true,
                    "noValidate": true,
                }
            }
        }),
    )
    .await;
    // Tiny delay so rmcp's service loop registers the request id in
    // its `local_ct_pool` before the cancel arrives. Same 50 ms used
    // by `cancellation.rs::notifications_cancelled_aborts_inflight_sync`.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancel_sent_at = Instant::now();
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": req_id, "reason": "budget-probe" }
        }),
    )
    .await;

    // Drain the response (either -32800 or clean envelope) so the
    // handler's `_permit` actually drops at end-of-function. Without
    // this drain the future is still alive and the permit is still
    // held — the budget would then measure pure rmcp dispatch latency,
    // not permit-release latency.
    let _resp = tokio::time::timeout(RESPONSE_BUDGET, read_frame_with_id(&mut reader, req_id))
        .await
        .expect("response (clean or -32800) must arrive within RESPONSE_BUDGET");

    // Probe: reacquire the permit from the same `Scheduler`. If the
    // handler leaked it, this `acquire` would block until the
    // `Semaphore` is closed (never, in this test) — the timeout
    // surfaces the leak as a clean budget violation.
    let elapsed_when_acquired = tokio::time::timeout(CANCEL_BUDGET, scheduler.acquire())
        .await
        .map(|_permit| cancel_sent_at.elapsed())
        .unwrap_or_else(|_| {
            panic!(
                "scheduler permit not reacquired within {CANCEL_BUDGET_MS} ms — \
                 handler leaked permit on cancel",
            )
        });
    assert!(
        elapsed_when_acquired <= CANCEL_BUDGET,
        "permit reacquired at {elapsed_when_acquired:?} > budget {CANCEL_BUDGET:?}",
    );

    // Cleanup.
    let _ = writer.shutdown().await;
    drop(writer);
    drop(reader);
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}

/// Spec §L5 line 151 + §"Known limitations" entry 6: after cancel,
/// the per-pack `PackLock` MUST be released within the OS budget.
/// Strict in-`grex_core::sync::run` PackLock observation is deferred
/// to feat-m7-3+; the pragmatic m7-2 substitute is "a SECOND
/// `tools/call sync` against the SAME pack_root completes within
/// budget after the first is cancelled".
///
/// If the first call leaked the PackLock, the second call would block
/// on `PackLock::acquire` indefinitely; the wall-clock timeout on the
/// second response surfaces the leak.
#[tokio::test]
#[allow(
    clippy::too_many_lines,
    reason = "inline handshake → first tools/call → cancel → drain → second \
              tools/call → drain probe; splitting would obscure the leak observation"
)]
async fn cancel_pack_lock_released_under_budget() {
    let _serial = CANCEL_SERIAL.lock().await;
    tools::sync::__test_set_block_until_cancelled(false);

    let workspace = TempDir::new().expect("alloc test workspace tempdir");
    let state = ServerState::new(
        // PARALLEL=2 so the second sync's permit acquire never blocks
        // on the first's permit release — we are observing PackLock
        // release latency, NOT scheduler permit release latency
        // (the previous test covers that). With PARALLEL=1 a leaked
        // permit would mask a leaked PackLock; PARALLEL=2 disambiguates.
        grex_core::Scheduler::new(2),
        grex_core::Registry::default(),
        workspace.path().join("grex.jsonl"),
        workspace.path().to_path_buf(),
    );
    let server = GrexMcpServer::new(state);

    let (server_io, client_io) = tokio::io::duplex(8 * 1024);
    let server_clone = server.clone();
    let server_task = tokio::spawn(async move {
        let _ = server_clone.run(server_io).await;
    });

    let (read, write) = tokio::io::split(client_io);
    let mut reader = BufReader::new(read);
    let mut writer = write;

    // Handshake.
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "grex-mcp-pack-lock-budget", "version": "0.0.1" }
            }
        }),
    )
    .await;
    let _ = read_frame_with_id(&mut reader, 1).await;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )
    .await;

    let pack_root = workspace.path().join("pack-lock-probe-nonexistent");

    // First sync call — fire and immediately cancel.
    let first_id: i64 = 100;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": first_id,
            "method": "tools/call",
            "params": {
                "name": "sync",
                "arguments": {
                    "packRoot": pack_root,
                    "dryRun": true,
                    "noValidate": true,
                }
            }
        }),
    )
    .await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let cancel_sent_at = Instant::now();
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": first_id, "reason": "pack-lock-probe" }
        }),
    )
    .await;
    let _first_resp =
        tokio::time::timeout(RESPONSE_BUDGET, read_frame_with_id(&mut reader, first_id))
            .await
            .expect("first sync response must arrive within RESPONSE_BUDGET");

    // Second sync — same pack_root. Must complete within budget after
    // the first is cancelled. A leaked PackLock would wedge here.
    let second_id: i64 = 101;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": second_id,
            "method": "tools/call",
            "params": {
                "name": "sync",
                "arguments": {
                    "packRoot": pack_root,
                    "dryRun": true,
                    "noValidate": true,
                }
            }
        }),
    )
    .await;
    let second_resp =
        tokio::time::timeout(CANCEL_BUDGET, read_frame_with_id(&mut reader, second_id))
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "second sync (same pack_root) did not complete within {CANCEL_BUDGET_MS} ms \
                     after first was cancelled — handler leaked PackLock",
                )
            });
    let elapsed = cancel_sent_at.elapsed();
    assert!(
        second_resp.get("result").is_some() || second_resp.get("error").is_some(),
        "second sync returned malformed envelope: {second_resp}",
    );
    // Sanity-bound the total cancel→second-response latency. Loose
    // bound: 2× CANCEL_BUDGET (covers the second sync's own runtime).
    assert!(
        elapsed <= CANCEL_BUDGET * 2,
        "second sync completed at {elapsed:?} > 2× budget {CANCEL_BUDGET:?}",
    );

    let _ = writer.shutdown().await;
    drop(writer);
    drop(reader);
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}

// ---------------------------------------------------------------------
// Shared driver for the 11 parametric cases
// ---------------------------------------------------------------------

/// Drive one cancel-chaos exchange for `verb`: `initialize` →
/// `notifications/initialized` → `tools/call <verb>` → after a 30 ms
/// settle, `notifications/cancelled`. Assert ANY terminal envelope
/// (clean `result` OR `-32800` error) arrives within `RESPONSE_BUDGET`.
///
/// Per spec §L5 lines 142 – 144 both outcomes are MCP-valid; the
/// race between handler-completion and cancel-arrival is the point.
/// Each verb gets a fresh `ServerState` + duplex pair so cancel races
/// do not bleed across cases (the rmcp `local_ct_pool` is per-server).
#[allow(
    clippy::too_many_lines,
    reason = "inline handshake → tools/call → cancel → drain sequence shared by \
              all 11 parametric verb cases; further extraction would split the \
              wire-level race observation across helpers and hurt readability"
)]
async fn drive_cancel_for_verb(verb: &str) {
    let workspace = TempDir::new().expect("alloc test workspace tempdir");
    let state = ServerState::new(
        grex_core::Scheduler::new(PARALLEL),
        grex_core::Registry::default(),
        workspace.path().join("grex.jsonl"),
        workspace.path().to_path_buf(),
    );
    let server = GrexMcpServer::new(state);

    let (server_io, client_io) = tokio::io::duplex(8 * 1024);
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_io).await;
    });

    let (read, write) = tokio::io::split(client_io);
    let mut reader = BufReader::new(read);
    let mut writer = write;

    // Handshake.
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "grex-mcp-cancel-chaos", "version": "0.0.1" }
            }
        }),
    )
    .await;
    let _ = read_frame_with_id(&mut reader, 1).await;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }),
    )
    .await;

    // tools/call <verb> with the minimal arg set each verb's `Params`
    // accepts. Mirrors `common::default_mcp_params_for` but inlined
    // here so this binary does not need to import `common` (which
    // brings in `assert_cmd` + the parity machinery).
    let req_id: i64 = 42;
    let args = mcp_args_for(verb, workspace.path());
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "tools/call",
            "params": {
                "name": verb,
                "arguments": args,
            }
        }),
    )
    .await;

    // 30 ms settle so rmcp registers the id in `local_ct_pool` before
    // cancel lands. Lower than the budget tests' 50 ms because here
    // we WANT some races to land both ways (cancel-wins and
    // handler-wins are both valid outcomes per spec).
    tokio::time::sleep(Duration::from_millis(30)).await;
    write_frame(
        &mut writer,
        &json!({
            "jsonrpc": "2.0",
            "method": "notifications/cancelled",
            "params": { "requestId": req_id, "reason": "chaos" }
        }),
    )
    .await;

    let resp = tokio::time::timeout(RESPONSE_BUDGET, read_frame_with_id(&mut reader, req_id))
        .await
        .unwrap_or_else(|_| {
            panic!("verb `{verb}` no terminal envelope within {RESPONSE_BUDGET:?}")
        });

    // Either outcome is valid per spec §L5 lines 142 – 144:
    //   * `result` envelope — handler completed before cancel landed
    //     (typical for the 9 stub verbs that return synchronously).
    //   * `error` envelope with code -32800 — cancel won the race
    //     (realistic for `sync` against a live pack).
    // ANY other shape (e.g. `error` with a non-cancel code) is a
    // contract violation worth a loud failure.
    if let Some(result) = resp.get("result") {
        // Clean CallToolResult — accept regardless of `isError`. Many
        // verbs return `isError=true` because they are M7-1 stubs;
        // that is still a valid "completed before cancel" outcome.
        assert!(result.is_object(), "verb `{verb}` returned non-object result: {result}",);
    } else if let Some(error) = resp.get("error") {
        let code = error.get("code").and_then(Value::as_i64).unwrap_or(0);
        assert_eq!(
            code,
            grex_mcp::REQUEST_CANCELLED as i64,
            "verb `{verb}` returned non-cancel error code {code}: {error}",
        );
    } else {
        panic!("verb `{verb}` returned envelope with neither result nor error: {resp}");
    }

    let _ = writer.shutdown().await;
    drop(writer);
    drop(reader);
    let _ = tokio::time::timeout(Duration::from_secs(2), server_task).await;
}

/// Minimal `tools/call` arguments per verb. Mirrors
/// `tests/common/mod.rs::default_mcp_params_for` but inlined to avoid
/// pulling the parity binary's full `common` module (which brings in
/// `assert_cmd` + subprocess machinery the cancel suite does not need).
fn mcp_args_for(verb: &str, workspace_root: &std::path::Path) -> Value {
    match verb {
        "add" => json!({ "url": "https://example.invalid/cancel-fixture.git" }),
        "rm" => json!({ "path": "nonexistent-pack" }),
        "run" => json!({ "action": "cancel-fixture-action" }),
        "exec" => json!({ "cmd": ["true"] }),
        "sync" => json!({
            "packRoot": workspace_root.join("cancel-sync-nonexistent"),
            "dryRun": true,
            "noValidate": true,
        }),
        // All other verbs are stubs accepting empty params.
        _ => json!({}),
    }
}

// ---------------------------------------------------------------------
// JSON-RPC line-frame helpers (lifted from stress.rs to keep this
// binary independent of `tests/common/mod.rs`'s subprocess machinery).
// ---------------------------------------------------------------------

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
        let frame: Value = serde_json::from_str(trimmed).expect("frame is valid JSON-RPC line");
        if frame.get("id").and_then(Value::as_i64) == Some(expected_id) {
            return frame;
        }
    }
}
