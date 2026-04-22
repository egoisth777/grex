//! `sync` tool — drive the M3 Stage B end-to-end pipeline.
//!
//! Only verb (alongside `teardown`, which is NOT exposed as an MCP tool)
//! that has a real core implementation today. Bridges
//! [`grex_core::sync::run`] (synchronous, blocking) onto the async
//! rmcp dispatch via [`tokio::task::spawn_blocking`]. Cancellation token
//! comes from the per-request `RequestContext::ct` plumbed in Stage 7
//! and threaded through `tool_router`'s `&self` shim.

use crate::error::{packop_error, CancelledExt};
use grex_core::sync::{self, SyncOptions};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::{CallToolResult, Content},
    ErrorData as McpError,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;

/// Args for `sync`. Mirrors the CLI's `--json` shape — JSON-only fields,
/// no positional args. `pack_root` is required at the MCP edge because
/// the legacy "no-arg stub print" branch makes no sense for an agent.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct SyncParams {
    /// Pack root: directory holding `.grex/pack.yaml` or the YAML file itself.
    pub pack_root: PathBuf,
    /// Workspace directory for cloned children.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
    /// Plan actions without touching the filesystem.
    #[serde(default)]
    pub dry_run: bool,
    /// Skip plan-phase validators. Debug-only escape hatch.
    #[serde(default)]
    pub no_validate: bool,
    /// Override the default ref for every pack.
    #[serde(default, rename = "ref")]
    pub ref_override: Option<String>,
    /// Restrict sync to packs whose path matches one of the globs.
    #[serde(default)]
    pub only: Vec<String>,
    /// Re-execute every pack even when its `actions_hash` is unchanged.
    #[serde(default)]
    pub force: bool,
    /// Max parallel pack ops. `None` → core default; `0` → unbounded; `1` → serial.
    #[serde(default)]
    pub parallel: Option<u32>,
}

pub(crate) async fn handle(
    state: &crate::ServerState,
    p: Parameters<SyncParams>,
    cancel: CancellationToken,
) -> Result<CallToolResult, McpError> {
    run_with_cancel(state, p.0, cancel).await
}

async fn run_with_cancel(
    state: &crate::ServerState,
    p: SyncParams,
    cancel: CancellationToken,
) -> Result<CallToolResult, McpError> {
    // Test-only block-until-cancelled hook — gated behind the `test-hooks`
    // cargo feature so it compiles out of release `grex serve` binaries
    // entirely (no exposed test surface, no runtime atomic load). The
    // cancellation integration test
    // (`crates/grex-mcp/tests/cancellation.rs::notifications_cancelled_aborts_inflight_sync`)
    // enables `test-hooks` via the dev-dep self-edge in `Cargo.toml`, then
    // flips the toggle on so the in-flight handler awaits its per-request
    // `CancellationToken` instead of running the (microseconds-fast)
    // `sync::run` and losing the race against `notifications/cancelled`.
    #[cfg(any(test, feature = "test-hooks"))]
    if test_hooks::block_until_cancelled() {
        cancel.cancelled().await;
        return Err(McpError::from(CancelledExt));
    }

    // feat-m7-2 Stage 7 — bound the MCP edge by the shared
    // `Scheduler` so concurrent `tools/call sync` invocations never
    // over-subscribe past `--parallel N`. This is the FIRST production
    // consumer of `Scheduler::acquire_cancellable` (m7-1 Stage 3 added
    // the method; m7-1 Stage 5 wired the scheduler into `ServerState`).
    // Holding the permit through the full handler — including the
    // `spawn_blocking(sync::run)` body — means the bound is observable
    // from the outside as the in-flight `tools/call` count, not just
    // the queued-into-spawn_blocking count. Permit drops at end-of-
    // function so the next queued caller can proceed. Cancellation
    // before a permit is granted maps to `-32800 RequestCancelled` via
    // the existing `CancelledExt` envelope.
    let _permit = state
        .scheduler
        .acquire_cancellable(&cancel)
        .await
        .map_err(|_| McpError::from(CancelledExt))?;

    // Test-only stress-barrier hook (feat-m7-2 Stage 6). When a
    // `tokio::sync::Barrier` has been installed via
    // `__test_set_stress_barrier`, every handler invocation increments
    // a shared in-flight counter, awaits the barrier (which releases
    // simultaneously across all parked handlers + the test thread),
    // then decrements the counter on its way out. The L4 stress harness
    // (`crates/grex-mcp/tests/stress.rs`) uses this to pin the in-flight
    // population at exactly PARALLEL handlers and assert the scheduler
    // never over-subscribes. Same `cfg(any(test, feature = "test-hooks"))`
    // gate as the cancellation hook above — zero footprint in release
    // `grex serve`. Stage 7: now sits AFTER the permit-acquire so only
    // PARALLEL handlers ever park here; the rest queue at the
    // semaphore.
    #[cfg(any(test, feature = "test-hooks"))]
    let _stress_guard = test_hooks::stress_barrier_enter().await;

    let opts = build_opts(&p);
    let pack_root = p.pack_root.clone();

    // `sync::run` is sync and may block on filesystem / git. Push it onto a
    // blocking thread so the rmcp dispatcher's reactor stays responsive. The
    // join handle is `select!`'d against `cancel.cancelled()` so the request
    // can return -32800 promptly (the OS thread continues briefly per the
    // documented leak window in `pack_lock::acquire_cancellable`).
    let cancel_clone = cancel.clone();
    let handle = tokio::task::spawn_blocking(move || sync::run(&pack_root, &opts, &cancel_clone));

    let outcome = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Err(McpError::from(CancelledExt)),
        joined = handle => joined,
    };

    match outcome {
        Ok(Ok(report)) => Ok(success_envelope(&report)),
        Ok(Err(err)) => Ok(packop_error(&format!("{err}"))),
        Err(join_err) => Ok(packop_error(&format!("internal: blocking task failed: {join_err}"))),
    }
}

/// Test-only knobs. `block_until_cancelled` is consulted by
/// `run_with_cancel` at the top of every call when compiled with the
/// `test-hooks` cargo feature; flipping it on turns the handler into
/// "await cancel; return -32800". This is the deterministic substitute
/// for a slow git fetch that the Stage 7 task description anticipated.
/// The whole module is gated behind `cfg(any(test, feature = "test-hooks"))`
/// so neither the atomic nor the setter ships in default-feature release
/// builds.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
mod test_hooks {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::sync::Barrier;

    static BLOCK: AtomicBool = AtomicBool::new(false);

    pub fn block_until_cancelled() -> bool {
        BLOCK.load(Ordering::SeqCst)
    }

    pub fn set_block_until_cancelled(v: bool) {
        BLOCK.store(v, Ordering::SeqCst);
    }

    // ---- Stress barrier hook (feat-m7-2 Stage 6) ----
    //
    // Holds an optional `Arc<Barrier>` plus an `AtomicUsize` recording
    // the high-water in-flight count observed across all handler
    // invocations. The test installs the barrier (sized N+1 so the test
    // thread is the +1 releaser), then drives N concurrent `tools/call
    // sync` requests; each handler hits `stress_barrier_enter`, bumps
    // the in-flight counter, awaits the barrier, and on guard-drop
    // decrements. When the post-`Barrier::wait()` snapshot equals
    // PARALLEL exactly, the contract holds.

    static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
    static HIGH_WATER: AtomicUsize = AtomicUsize::new(0);

    fn barrier_slot() -> &'static Mutex<Option<Arc<Barrier>>> {
        static SLOT: Mutex<Option<Arc<Barrier>>> = Mutex::new(None);
        &SLOT
    }

    pub fn set_stress_barrier(b: Option<Arc<Barrier>>) {
        *barrier_slot().lock().expect("stress barrier slot poisoned") = b;
    }

    pub fn reset_stress_metrics() {
        IN_FLIGHT.store(0, Ordering::SeqCst);
        HIGH_WATER.store(0, Ordering::SeqCst);
    }

    pub fn stress_high_water() -> usize {
        HIGH_WATER.load(Ordering::SeqCst)
    }

    /// RAII guard returned by [`stress_barrier_enter`]. Decrements
    /// `IN_FLIGHT` when dropped so the counter reflects live handlers
    /// only. The high-water mark is monotone — never decremented.
    pub struct StressGuard {
        _private: (),
    }

    impl Drop for StressGuard {
        fn drop(&mut self) {
            IN_FLIGHT.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Increment the in-flight counter, refresh the high-water mark,
    /// and (if a barrier is installed) await it. Returns a guard whose
    /// `Drop` decrements the in-flight counter. Cheap no-op when no
    /// barrier is installed (the common case — only the L4 stress
    /// harness installs one).
    pub async fn stress_barrier_enter() -> StressGuard {
        let prev = IN_FLIGHT.fetch_add(1, Ordering::SeqCst);
        let now = prev + 1;
        // Atomic max via CAS loop. `fetch_max` is stable on AtomicUsize
        // since 1.45 so a single call suffices.
        HIGH_WATER.fetch_max(now, Ordering::SeqCst);

        let barrier = barrier_slot().lock().expect("stress barrier slot poisoned").clone();
        if let Some(b) = barrier {
            b.wait().await;
        }
        StressGuard { _private: () }
    }
}

/// Test-only setter for the block-until-cancelled hook. See
/// [`test_hooks`] for rationale. Hidden from rustdoc and compiled out
/// unless the `test-hooks` cargo feature is enabled.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn __test_set_block_until_cancelled(v: bool) {
    test_hooks::set_block_until_cancelled(v);
}

/// Test-only setter for the L4 stress barrier (feat-m7-2 Stage 6).
/// Pass `Some(barrier)` to install; pass `None` to clear after the
/// stress test releases. Sized at `Barrier::new(PARALLEL + 1)` — N
/// handlers + the test thread.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn __test_set_stress_barrier(b: Option<std::sync::Arc<tokio::sync::Barrier>>) {
    test_hooks::set_stress_barrier(b);
}

/// Test-only reset for the stress in-flight + high-water counters.
/// Call once at the top of every stress case so a previous run's
/// state does not bleed into the next.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn __test_reset_stress_metrics() {
    test_hooks::reset_stress_metrics();
}

/// Test-only accessor for the high-water in-flight count observed by
/// the stress barrier. Monotone — never decremented across calls.
#[cfg(any(test, feature = "test-hooks"))]
#[doc(hidden)]
pub fn __test_stress_high_water() -> usize {
    test_hooks::stress_high_water()
}

fn build_opts(p: &SyncParams) -> SyncOptions {
    let only = if p.only.is_empty() { None } else { Some(p.only.clone()) };
    SyncOptions::new()
        .with_dry_run(p.dry_run)
        .with_validate(!p.no_validate)
        .with_workspace(p.workspace.clone())
        .with_ref_override(p.ref_override.clone())
        .with_only_patterns(only)
        .with_force(p.force)
}

fn success_envelope(report: &grex_core::sync::SyncReport) -> CallToolResult {
    let body =
        format!("sync ok: {} step(s); halted={}", report.steps.len(), report.halted.is_some());
    CallToolResult::success(vec![Content::text(body)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::handler::server::tool::schema_for_type;

    #[test]
    fn sync_params_schema_resolves() {
        let _ = schema_for_type::<SyncParams>();
    }

    /// Happy-path: against an empty / non-existent pack root the core
    /// returns a `SyncError`; we map to a `pack_op` envelope. The test's
    /// goal is "handler runs and returns a structured envelope" — the
    /// exact error text is core's domain.
    #[tokio::test]
    async fn sync_happy_path_returns_envelope() {
        let s = crate::ServerState::for_tests();
        let p = SyncParams {
            pack_root: std::env::temp_dir().join("grex-mcp-nonexistent-pack"),
            workspace: None,
            dry_run: true,
            no_validate: true,
            ref_override: None,
            only: Vec::new(),
            force: false,
            parallel: None,
        };
        let r = handle(&s, Parameters(p), CancellationToken::new()).await.unwrap();
        // We expect failure — pack root does not exist. Either way the
        // tool MUST return Ok(envelope), not a JSON-RPC -32xxx.
        assert!(r.is_error.is_some(), "must set isError flag");
    }
}
