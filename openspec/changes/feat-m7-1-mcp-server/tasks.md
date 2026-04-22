# feat-m7-1 — tasks (TDD)

**Status**: draft
**Spec**: [`spec.md`](./spec.md)
**SSOT**: [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md), [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md)

Tests-first per stage. A stage is "done" only when its listed tests exist, run red before code lands, and run green after. No stage may skip ahead; cross-stage refactors are explicit sub-items.

---

## Stage 1 — Workspace deps + crate scaffold

Land the empty crate and the deps so subsequent stages compile in isolation.

- [ ] 1.0 **Verify rmcp 1.5.0 surface before pinning.** Run `cargo doc -p rmcp` (or `cargo add rmcp@1.5.0 --dry-run && cargo doc`) and confirm: (a) `#[tool]` proc-macro is published, (b) `Parameters<T: JsonSchema>` handler surface exists, (c) client `peer().send_request().cancel(reason)` helper is available (else fall back to raw stdio `notifications/cancelled` per spec §Cancellation). If any of (a)/(b) is missing, pin a different version OR narrow this change's scope; do NOT proceed with a broken pin.
- [ ] 1.1 Add to root `Cargo.toml` `[workspace.dependencies]`:
  - [ ] `rmcp = "1.5.0"`
  - [ ] `tokio-util = { version = "0.7", features = ["rt"] }`
- [ ] 1.2 Create `crates/grex-mcp/Cargo.toml` — name `grex-mcp`, `workspace = true` on all deps (`rmcp`, `tokio`, `tokio-util`, `schemars`, `serde`, `tracing`, `grex-core`).
- [ ] 1.3 Create `crates/grex-mcp/src/lib.rs` with a single `pub fn _placeholder() {}` (to be replaced in Stage 5).
- [ ] 1.4 Add `crates/grex-mcp` to root `Cargo.toml` `[workspace].members`.
- [ ] 1.5 Add `tokio-util` to `crates/grex-core/Cargo.toml` (workspace pin).

**Tests**:
- [ ] 1.T1 `cargo build -p grex-mcp` succeeds.
- [ ] 1.T2 `cargo metadata` shows `rmcp 1.5.*` and `tokio-util 0.7.*` exactly once in the resolved graph (no dup-version warnings).
- [ ] 1.T3 `cargo test --workspace` remains green (no behavioural code yet).

**Verify**: `cargo clippy -p grex-mcp -- -D warnings` clean.

---

## Stage 2 — `CancellationToken` threading through verb `run()` signatures

Mechanical refactor. No behaviour change for CLI users. Unblocks Stages 3-7.

- [ ] 2.1 Add `cancel: &CancellationToken` as the final parameter to each core-verb `run()`:
  - [ ] `init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `import`, `run`, `exec` (11).
  - [ ] Plus `teardown` (plugin hook, for parity — invoked from `rm`).
- [ ] 2.2 Update every CLI caller in `crates/grex/src/cli/verbs/*.rs` to pass `&CancellationToken::new()` (never-cancelled sentinel).
- [ ] 2.3 Update existing core tests that call `run()` directly to pass `&CancellationToken::new()`.
- [ ] 2.4 No `cancel.is_cancelled()` checks inside verb bodies yet — those land as the acquire-cancellable call sites are wired in Stages 3-4.

**Tests**:
- [ ] 2.T1 `cargo test --workspace` green — proves the refactor is signature-only.
- [ ] 2.T2 New test `crates/grex-core/tests/cancel_plumbing.rs::cli_sentinel_never_cancels` — construct a `CancellationToken::new()`, assert `.is_cancelled() == false` after a no-op `sync` over an empty workspace.
- [ ] 2.T3 `cargo clippy --workspace -- -D warnings` clean.

---

## Stage 3 — `Scheduler::acquire_cancellable` + tests

Tests first (red), then implementation.

- [ ] 3.1 Write unit tests in `crates/grex-core/src/scheduler.rs #[cfg(test)]`:
  - [ ] 3.T1 `acquire_cancellable_returns_permit_when_not_cancelled`
  - [ ] 3.T2 `acquire_cancellable_returns_cancelled_if_token_fires_before_permit` — 0-permit scheduler, cancel after 10 ms, assert `Err(Cancelled)` within 30 ms.
  - [ ] 3.T3 `acquire_cancellable_dropped_future_does_not_leak_permit` — 4 permits, spawn 100 waiters each with its own token, cancel all; assert `available_permits() == 4` after join.
  - [ ] 3.T4 `acquire_cancellable_cancel_after_success_is_no_op` — acquire succeeds, then cancel; permit still valid until drop.
- [ ] 3.2 Run: all four tests must compile and fail (method missing).
- [ ] 3.3 Implement:

  ```rust
  pub struct Cancelled;
  impl Scheduler {
      pub async fn acquire_cancellable(
          &self,
          cancel: &CancellationToken,
      ) -> Result<OwnedSemaphorePermit, Cancelled> {
          tokio::select! {
              biased;
              _ = cancel.cancelled() => Err(Cancelled),
              permit = self.permits.clone().acquire_owned() => {
                  permit.map_err(|_| Cancelled)
              }
          }
      }
  }
  ```

- [ ] 3.4 Re-run tests: all green.
- [ ] 3.5 Verify with `cargo clippy -p grex-core -- -D warnings`.

---

## Stage 4 — `PackLock::acquire_cancellable` + tests

Tests first (red), then implementation. Covers the `spawn_blocking` leak-window contract explicitly.

- [ ] 4.1 Write unit tests in `crates/grex-core/src/pack_lock.rs #[cfg(test)]`:
  - [ ] 4.T1 `acquire_cancellable_happy_path` — uncontended path, returns `Ok(PackLockHold)`.
  - [ ] 4.T2 `acquire_cancellable_cancel_during_blocking_fd_lock_returns_cancelled` — task A holds the lock, task B calls `acquire_cancellable`, cancel B's token after 10 ms, assert `Err(Cancelled)` within 50 ms.
  - [ ] 4.T3 `acquire_cancellable_spawn_blocking_thread_releases_guard_when_it_finally_unblocks` — regression for the documented OS-thread leak window; release A's lock after cancelling B, assert the blocking thread completes and drops its guard (verified via a `Weak<File>` sentinel).
- [ ] 4.2 Run: all three tests must compile and fail.
- [ ] 4.3 Implement:
  - [ ] `pub enum PackLockErrorOrCancelled { Cancelled, Lock(PackLockError) }`
  - [ ] `acquire_cancellable(self, cancel: &CancellationToken) -> Result<PackLockHold, PackLockErrorOrCancelled>` — consumes `self` to match existing `acquire_async(self)` at `pack_lock.rs:179`; reuses the boxed-fd + `transmute` lifetime dance. Uses `tokio::task::spawn_blocking` wrapping the existing `fd_lock::write()` call; outer `tokio::select!` between `cancel.cancelled()` and the `JoinHandle`.
  - [ ] Inline doc-comment describing the "OS thread held past cancel until syscall returns" contract, linking to `.omne/cfg/mcp.md` §Cancellation.
- [ ] 4.4 Re-run tests: all green.
- [ ] 4.5 Verify `cargo clippy -p grex-core -- -D warnings`.

---

## Stage 5 — Server skeleton + `initialize` / `tools/list` / `shutdown` + stderr-only tracing

Wire the rmcp framework, land the handshake and empty tools/list, enforce stdout discipline.

- [ ] 5.1 Replace `crates/grex-mcp/src/lib.rs` placeholder with:
  - [ ] `pub struct ServerState { scheduler, registry, manifest, workspace }`
  - [ ] `pub struct GrexMcpServer { state: Arc<ServerState> }`
  - [ ] `impl GrexMcpServer { pub fn new(state) -> Self; pub async fn run(self, transport) -> Result<(), rmcp::Error>; }`
- [ ] 5.2 Register `initialize`, `tools/list` (returns empty array for now), `shutdown` via rmcp's builder.
- [ ] 5.3 Set up `tracing_subscriber::fmt().with_writer(std::io::stderr).init()` inside `GrexMcpServer::run` (idempotent guard for test reuse).
- [ ] 5.4 Add `crates/grex-mcp/src/error.rs` skeleton with `From<Cancelled>` → `ErrorData { code: -32800, .. }`.
- [ ] 5.5 Add `crates/grex-mcp/src/tools/mod.rs` with `pub const VERBS_11_EXPOSED_AS_TOOLS: &[&str] = &[/* 11 names */];` and `const _: () = assert!(VERBS_11_EXPOSED_AS_TOOLS.len() == 11);`.

**Tests**:
- [ ] 5.T1 `crates/grex-mcp/tests/handshake.rs::initialize_handshake_accepts_2025_06_18` — duplex-stream round-trip, assert server's `protocolVersion == "2025-06-18"`.
- [ ] 5.T2 `handshake.rs::batch_request_array_is_rejected_with_minus_32600` — send `[req,req]`, expect `-32600`.
- [ ] 5.T3 `handshake.rs::shutdown_returns_then_closes` — send shutdown, assert clean close.
- [ ] 5.T4 `crates/grex-mcp/src/lib.rs::stdout_has_no_tracing_leaks` — spawn server against a `duplex`, emit `tracing::info!` from a mock tool, capture server stdout, assert zero non-JSON-RPC bytes.
- [ ] 5.T5 CI lint: `grep -rn 'println!\|print!' crates/grex-mcp/src/` returns 0 matches (Make or xtask target).
- [ ] 5.T6 `tools_list_returns_empty_in_stage_5` — sanity check; replaced in Stage 6.

---

## Stage 6 — 11 tool impls with agent-safety annotations

Each tool is its own file under `crates/grex-mcp/src/tools/`. Tests first per tool: schema assertion + happy-path call.

- [ ] 6.1 For each verb `v` in `{init, add, rm, ls, status, sync, update, doctor, import, run, exec}`:
  - [ ] 6.1.`v`.a Define `<V>Params` struct in `tools/<v>.rs` mirroring the CLI `--json` input shape; derive `JsonSchema + Deserialize`.
  - [ ] 6.1.`v`.b Define `#[tool(name = "<v>", description = "...", annotations(read_only_hint = ..., destructive_hint = ...))]` async handler.
  - [ ] 6.1.`v`.c Handler builds a fresh `ExecCtx` borrowing `ServerState`, calls the core `v::run(ctx, opts, cancel)`, maps result to `CallToolResult { content, isError }`.
  - [ ] 6.1.`v`.d `exec` `<V>Params` has NO `shell` field.
- [ ] 6.2 Populate `tools/mod.rs` with the 11 `#[tool]` registrations; `tools/list` now returns all 11.
- [ ] 6.3 Flesh out `error.rs`:
  - [ ] Pack-op failures → `ErrorData { code: -32002, data: { kind: "pack_op", .. } }` (inside `CallToolResult.isError = true`).
  - [ ] Init-state failures → `ErrorData { code: -32002, data: { kind: "init_state", .. } }` (envelope).
  - [ ] Manifest / lock / drift / plugin-missing → `-32001` / `-32003` / `-32004` / `-32005`.

**Tests**:
- [ ] 6.T1 `tools/mod.rs::tools_list_advertises_exactly_11`.
- [ ] 6.T2 `tools/mod.rs::every_tool_has_both_annotation_hints`.
- [ ] 6.T3 `tools/mod.rs::destructive_tools_are_rm_run_exec_only`.
- [ ] 6.T4 `tools/exec.rs::exec_tool_schema_has_no_shell_field`.
- [ ] 6.T5 `error.rs::packop_failure_maps_to_minus_32002_with_kind_pack_op`.
- [ ] 6.T6 `error.rs::init_state_failure_maps_to_minus_32002_with_kind_init_state`.
- [ ] 6.T7 `error.rs::cancelled_maps_to_minus_32800`.
- [ ] 6.T8 `crates/grex-mcp/tests/agent_safety.rs::exec_tool_rejects_shell_param_with_minus_32602`.
- [ ] 6.T9 `crates/grex-mcp/tests/lock_ordering.rs::concurrent_tool_calls_share_arc_scheduler` — `--parallel 2` at boot, two `sync` calls, max-observed in-flight ≤ 2.
- [ ] 6.T10 `lock_ordering.rs::pack_lock_acquired_after_permit_not_before` — tracing-span ordering.

---

## Stage 7 — `notifications/cancelled` handler wired to `CancellationToken`

- [ ] 7.1 In `GrexMcpServer`, maintain a `DashMap<RequestId, CancellationToken>` of in-flight tool calls.
- [ ] 7.2 On each `tools/call` entry: insert token; on exit: remove.
- [ ] 7.3 `notifications/cancelled` handler: look up `requestId`, call `token.cancel()`.
- [ ] 7.4 Tool bodies already receive `cancel: &CancellationToken` (Stage 2); confirm every acquire site in core uses `acquire_cancellable` for the handlers that reach scheduler / pack-lock (primarily `sync`, `update`, `run`, `exec`).

**Tests**:
- [ ] 7.T1 `crates/grex-mcp/tests/cancellation.rs::notifications_cancelled_aborts_inflight_sync` — mock-fetch fixture that blocks; send cancel after 50 ms; assert `-32800` within 200 ms.
- [ ] 7.T2 `cancellation.rs::cancel_after_result_is_ignored` — tool completes, cancel arrives; no crash, no spurious error.
- [ ] 7.T3 `cancellation.rs::cancel_unknown_request_id_is_ignored` — protocol MAY-silent on unknown id.

---

## Stage 8 — `grex serve` CLI wiring + smoke test

- [ ] 8.1 Replace stub body in `crates/grex/src/cli/verbs/serve.rs`:
  - [ ] Build `ServerState` from the global `ExecCtx` (scheduler from `--parallel`, registry, manifest path from `--manifest`).
  - [ ] Construct `rmcp::transport::stdio()`.
  - [ ] `GrexMcpServer::new(state).run(transport).await`.
- [ ] 8.2 `ServeOpts` accepts `--manifest <path>` override (captured at launch; not mutable mid-session).
- [ ] 8.3 Inherit global `--parallel N`.

**Tests**:
- [ ] 8.T1 `crates/grex/tests/serve_smoke.rs::grex_serve_subprocess_responds_to_tools_list` — spawn `grex serve` as child, pipe initialize + tools/list, assert 11-tool response, stdout is valid JSON-RPC only.
- [ ] 8.T2 `serve_smoke.rs::grex_serve_shutdown_exits_cleanly` — send shutdown, assert exit code 0 within 500 ms.
- [ ] 8.T3 `serve_smoke.rs::grex_serve_stderr_carries_tracing` — assert stderr contains `tracing` lines, stdout contains none.
- [ ] 8.T4 CI: add `mcp-validator` job in `.github/workflows/ci.yml` running `mcp-validator` against a release build; blocks merge.

---

## Cross-stage exit gates

- [ ] G1 `cargo test --workspace` green across all stages.
- [ ] G2 `cargo clippy --all-targets --workspace -- -D warnings` clean.
- [ ] G3 `mcp-validator` green in CI.
- [ ] G4 No regressions on M5 (plugin) + M6 (scheduler, pack-lock) suites.
- [ ] G5 Spec acceptance criteria 1-13 all demonstrably met (cross-link each to its test in the PR description).
