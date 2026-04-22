# feat-m7-2 — `grex-mcp` test harness (L2 – L5)

**Status**: draft
**Milestone**: M7 (see [`../../../milestone.md`](../../../milestone.md) §M7)
**Depends on**: feat-m7-1 (`grex-mcp` server + cancellable tool API); M6 (scheduler + per-pack `PackLock`); M5 pack-type plugin system.

## Motivation

feat-m7-1 seeds the `grex-mcp` server with unit-level (L1) coverage of routing, schema derivation, and error mapping. That's enough to prove the handlers exist; it is not enough to prove the server **behaves** correctly on the wire, matches the CLI field-for-field, stays correct under parallel load, or releases its M6 primitives when a client cancels mid-call.

`.omne/cfg/mcp.md` fixes the contract — MCP 2025-06-18 stdio, 11 exposed tools, standard `notifications/cancelled`, 5-tier lock ordering shared verbatim with the CLI. `.omne/cfg/test-plan.md` splits MCP coverage into eight layers (L1 – L8). This change lands layers **L2 – L5** — the ones that can be built with in-process transports plus a single per-OS real-pipe guard. Layers L6 (Inspector), L7 (mcp-validator), and L8 (fuzz) are deferred to feat-m7-3; they need external binaries and long-running jobs and must not block merge of the core harness.

## Goal

1. Add E2E handshake coverage over `tokio::io::duplex(4096)` — `initialize` → `initialized` → `tools/list` → `shutdown`, plus rejection of requests-before-init and rejection of double-init.
2. Add **one** real-pipe subprocess test per OS (Linux + Windows + macOS) to catch regressions specific to OS pipe buffering, >64 KiB responses, and closed-stderr behaviour that `duplex` cannot exercise.
3. Add CLI ↔ MCP parity — for each of the 11 exposed verbs, assert the normalised `--json` CLI output equals the normalised MCP `tools/call` result structurally.
4. Add a concurrent-stress harness — N = 100 clients × 11 verbs against a shared server instance, with a `tokio::sync::Barrier` pinning saturation so the assertions are deterministic.
5. Add cancellation-chaos coverage — every tool must honour `notifications/cancelled` and release its scheduler permit + pack-lock within a bounded wall-clock budget.
6. Tests live **inside** `crates/grex-mcp/tests/` — no separate `grex-mcp-tests` crate (violates the sub-crates-avoided rule in `.omne/cfg/architecture.md`).

## Design

### Directory shape (no new crate)

```
crates/grex-mcp/
├── src/                          # L1 unit tests live inline under #[cfg(test)] — owned by feat-m7-1
└── tests/
    ├── common/
    │   └── mod.rs                # fixtures + normalize() helper
    ├── handshake.rs              # L2 — duplex E2E lifecycle
    ├── real_pipe_linux.rs        # L2 — cfg(target_os="linux") subprocess guard
    ├── real_pipe_macos.rs        # L2 — cfg(target_os="macos") subprocess guard
    ├── real_pipe_windows.rs      # L2 — cfg(target_os="windows") subprocess guard
    ├── parity.rs                 # L3 — CLI vs MCP per-verb parity
    ├── stress.rs                 # L4 — concurrent saturation
    └── cancel.rs                 # L5 — cancellation chaos
```

No top-level crate additions. `crates/grex-mcp/Cargo.toml` `[dev-dependencies]` gains `tokio` (features `full`), `serde_json`, `tempfile`, `assert_cmd`, `anyhow`, and whatever `rmcp` test-utility surface is published.

### L1 — owned by feat-m7-1

L1 inline unit tests (routing, schema derivation, error mapping) belong to feat-m7-1 alongside the code they cover. **This change adds no L1.** Listed here for completeness only; L1 closes when m7-1 lands.

### L2 — E2E handshake (`handshake.rs`)

Harness: one paired `tokio::io::duplex(4096)` split into `(client_rx, client_tx)` + `(server_rx, server_tx)`. Server wired via the same framer production uses (`rmcp` `transport-io`). No subprocess, no filesystem, no network.

Cases (each an independent `#[tokio::test]`):

| Name | Flow | Assertion |
|---|---|---|
| `handshake_ok` | `initialize` → `notifications/initialized` → `tools/list` → `shutdown` | `tools/list` returns `>= VERBS_EXPOSED.len()` tools; `shutdown` returns a clean result. |
| `request_before_init_rejected` | send `tools/list` before `initialize` | error `-32002` with `data.kind = "init_state"`. |
| `double_init_rejected` | `initialize` twice | second errors `-32002` with `data.kind = "init_state"`. |
| `graceful_shutdown_drains` | spawn a long tool (test-only `sleep` sentinel) → `shutdown` | shutdown blocks until the in-flight tool returns; no panics on the drop path. |
| `protocol_version_echoed` | `initialize` with `2025-06-18` | `result.protocolVersion == "2025-06-18"`. |

### L2 — Real-pipe guard (`real_pipe_*.rs`)

One test per OS, cfg-gated (`#[cfg(target_os = "linux")]` / `#[cfg(target_os = "macos")]` / `#[cfg(target_os = "windows")]`). Spawns a release build of `grex serve` via `assert_cmd`, feeds bytes through real OS pipes, and verifies:

- A single `tools/call` result > 64 KiB is delivered intact (crosses the kernel pipe buffer boundary that `duplex(4096)` hides).
- Closing stderr on the client side does not crash the server (stdout-discipline invariant from `.omne/cfg/mcp.md`).

Rationale: `duplex` is in-memory and cannot reproduce kernel-pipe backpressure or stderr closure. Two focused subprocess tests close that gap without paying the subprocess tax on every L2 case.

### L3 — CLI ↔ MCP parity (`parity.rs`)

**Tool enumeration**: the 11 MCP tools are enumerated by a single const exported from `grex-mcp`:

```rust
pub const VERBS_EXPOSED: &[&str] = &[
    "init", "add", "rm", "ls", "status", "sync",
    "update", "doctor", "import", "run", "exec",
];
```

Parity assertion uses `>=`, not `==`:

```rust
assert!(tools_list.tools.len() >= VERBS_EXPOSED.len(),
    "tools/list must expose at least {} tools, got {}",
    VERBS_EXPOSED.len(), tools_list.tools.len());
```

The inequality lets future changes add MCP-only tools (e.g. `workspace/subscribe` in v1.x) without retripping this check.

**Per-verb test shape** — a **helper function**, not a macro. An earlier draft proposed `parity_test!`; review rejected it (opaque error spans, harder IDE nav, over-abstract for 11 cases).

```rust
async fn assert_parity(verb: &str, args: &[&str], mcp_params: Value) {
    let fixture = TestFixture::new();
    let cli_json = run_cli_json(&fixture, verb, args).await;
    let mcp_json = run_mcp_tool(&fixture, verb, mcp_params).await;
    assert_eq!(normalize(cli_json), normalize(mcp_json));
}
```

**Normaliser** — two placeholder tokens only:

- `<TS>` — any RFC3339 / `u64` timestamp field.
- `<PATH>` — any absolute path prefix matching the fixture workspace root is rewritten to a relative-from-fixture-root form.

No `<ID>`, `<PID>`, or `<SHA>` tokens unless a concrete failing test proves it is needed. The minimum surface is deliberate — every extra placeholder is a potential false-positive hider.

### L4 — Concurrent stress (`stress.rs`)

N = 100 virtual clients × 11 verbs = 1 100 in-flight `tools/call` invocations against one shared `grex-mcp` server. Each client gets its own `duplex` pair; all clients share one `Arc<Server>`.

Determinism via explicit barrier:

```rust
let barrier = Arc::new(Barrier::new(PARALLEL + 1));
for _ in 0..PARALLEL {
    let b = barrier.clone();
    tokio::spawn(async move {
        b.wait().await;            // release simultaneously
        tool_handler.run().await;
    });
}
barrier.wait().await;              // main releases the herd
```

The barrier wait is injected into the **handler body** (via a test-only hook behind `#[cfg(test)]`), not into the top of the test — that way saturation is provable at the semaphore, not just at spawn.

Assertions:

- `scheduler.high_water() >= PARALLEL` — prove we actually saturated. Exact equality is flaky under slow CI; inequality is the contract.
- `scheduler.high_water() <= PARALLEL` — prove we never over-subscribed. Asserted separately so failures disambiguate.
- Same-pack requests serialise through the M6 `PackLock` — a dedicated sub-test sends 8 concurrent `tools/call{name:"sync", arguments:{pack:"p1"}}` and asserts `ActionStarted(p1, i+1)` strictly follows `ActionCompleted(p1, i)`.
- Wall-clock budget — initial placeholder **5 s**, recalibrated from first CI run's p99 × 1.5 and committed. Treat this as tuneable, not sacred.

CI repeat policy: each stress test runs **3× consecutively** in one job. A single failure across the 3 runs fails the job. Rationale: parallelism bugs often escape single-shot runs; repeating triples detection sensitivity at marginal cost.

### L5 — Cancellation chaos (`cancel.rs`)

For each of the 11 tools:

1. Send `tools/call` with `requestId = N`.
2. Immediately send `notifications/cancelled { "requestId": N }` (no artificial sleep — the race is the point).
3. Assert the response is **either** `-32800 RequestCancelled` **or** a clean `CallToolResult` — per `.omne/cfg/mcp.md`, both are MCP-valid outcomes depending on whether cancellation fires before or after the handler's terminal `await`.
4. Assert the scheduler permit **and** the pack-lock are released within a bounded wall-clock budget:
   - Linux / macOS: **250 ms**.
   - Windows: **500 ms** (per MED-5 — Windows file-lock cancellation latency is OS-driven, not grex-driven).

Permit-release probe: after cancel, acquire `PARALLEL` permits with a 1 s timeout — if the leak existed, `acquire` would block indefinitely.

Pack-lock-release probe: after cancel, acquire `PackLock::acquire(same_path)` with the same budget; must succeed.

## File / module targets

| Concrete path | Change |
|---|---|
| `crates/grex-mcp/tests/common/mod.rs` | New — `TestFixture`, `run_cli_json`, `run_mcp_tool`, `normalize`. |
| `crates/grex-mcp/tests/handshake.rs` | New — L2 duplex lifecycle cases. |
| `crates/grex-mcp/tests/real_pipe_linux.rs` | New — `#[cfg(target_os = "linux")]` subprocess guard. |
| `crates/grex-mcp/tests/real_pipe_macos.rs` | New — `#[cfg(target_os = "macos")]` subprocess guard. |
| `crates/grex-mcp/tests/real_pipe_windows.rs` | New — `#[cfg(target_os = "windows")]` subprocess guard. |
| `crates/grex-mcp/tests/parity.rs` | New — L3 per-verb parity loop. |
| `crates/grex-mcp/tests/stress.rs` | New — L4 concurrent saturation. |
| `crates/grex-mcp/tests/cancel.rs` | New — L5 cancellation chaos. |
| `crates/grex-mcp/src/lib.rs` | Export `pub const VERBS_EXPOSED: &[&str]` for tests + downstream asserts (added here if feat-m7-1 did not land it). |
| `crates/grex-mcp/src/handlers/mod.rs` | Add `#[cfg(test)]` barrier-wait hook (L4 determinism). |
| `crates/grex-mcp/Cargo.toml` | `[dev-dependencies]`: `tokio` (features = full), `serde_json`, `tempfile`, `assert_cmd`, `anyhow`. |

## Test plan

### L2 unit envelope

`handshake.rs` cases enumerated in §Design. 5 cases, all `#[tokio::test]`.

### L2 real-pipe

`real_pipe_linux.rs` + `real_pipe_macos.rs` + `real_pipe_windows.rs` — 2 cases per file, 6 total:

- `large_response_crosses_pipe_buffer` — send `tools/call{name:"ls"}` against a fixture with > 1 024 packs; assert deserialisable result.
- `client_stderr_close_does_not_panic_server` — close client stderr after `initialize`; send `tools/list`; assert server still responds.

### L3 parity

One `#[tokio::test]` **per verb** (11 tests), parametric over `VERBS_EXPOSED`. Each asserts `normalize(cli_json) == normalize(mcp_json)`. Fixture is a tempdir workspace seeded once per test; destructive verbs (`rm`, `run`, `exec`) get isolated workspaces.

### L4 stress

`stress.rs`:

- `stress_100x11_no_oversubscription` — barrier + saturation asserts.
- `stress_same_pack_serialises` — 8 concurrent same-pack `sync` calls; interleave-free invariant.
- `stress_no_deadlock_across_3_iterations` — run the above 3× back-to-back in one `#[tokio::test]`; any iteration failure fails the test.

### L5 cancel

`cancel.rs` — 11 parametric cases (one per exposed tool) plus:

- `cancel_permit_released_under_budget` — explicit post-cancel permit-acquire probe.
- `cancel_pack_lock_released_under_budget` — explicit post-cancel `PackLock::acquire` probe.

## Non-goals

- **No L6 Inspector harness.** Browser/HTTP sidecar, moved to feat-m7-3.
- **No L7 mcp-validator binary runs.** External tool, moved to feat-m7-3.
- **No L8 fuzz (`cargo fuzz`).** Long-running, deferred further in m7-3.
- **No HTTP/SSE transport tests.** Stdio is the only v1 transport per `.omne/cfg/mcp.md` §Launch.
- **No multi-client-per-server tests.** One `grex serve` = one session per `.omne/cfg/mcp.md` §Session model.
- **No progress-notification coverage.** `notifications/progress` is deferred per `.omne/cfg/mcp.md` §Progress.
- **No `<ID> / <PID> / <SHA>` placeholders in the normaliser** unless a test proves they are required. Start minimal; add only with a failing-test justification.
- **No new top-level crate.** Tests nest under `crates/grex-mcp/tests/` per `.omne/cfg/architecture.md` §Workspace.

## Dependencies

- **Prior**: feat-m7-1 (server + cancellable tool-handler API + `VERBS_EXPOSED` const); M6 feat-m6-1 (`Scheduler`), feat-m6-2 (`PackLock`), feat-m6-3 (lock-order proof); M5 pack-type plugin system.
- **Next**: feat-m7-3 (L6 Inspector, L7 mcp-validator, L8 fuzz).

## Acceptance

1. All L2 tests pass on Linux + macOS + Windows CI — both `duplex` cases and the one real-pipe case per OS.
2. L3 parity passes for all 11 verbs — `normalize(cli_json) == normalize(mcp_json)` byte-equal.
3. L4 stress passes 3× consecutive on Linux + Windows; both `high_water` assertions hold; same-pack interleave-free invariant holds.
4. L5 cancel passes for all 11 tools; post-cancel permit-acquire and pack-lock-acquire probes succeed within the OS-specific budget.
5. No new top-level crate introduced — tests live under `crates/grex-mcp/tests/` only.
6. `cargo clippy --all-targets --workspace -- -D warnings` clean.
7. Wall-clock budget recalibrated from first CI run p99 × 1.5 and committed; flakiness triaged to root cause, not retried.

## Known limitations

Discovered during Stage 2 implementation; affect L2 envelope-layer assertions.

1. **L2.2 `request_before_init_rejected`** asserts transport-close (server EOF), NOT a `-32002 init_state` envelope. rmcp 1.5.0's `ServerInitializeError::ExpectedInitializeRequest` gate (see `serve_directly_with_ct` ~L170-203) closes the transport rather than returning a structured error. The test still proves the safety contract (no method dispatch happens pre-init), and EOF is a strictly stronger signal than `-32002`. Wiring `init_state_error()` (defined at `crates/grex-mcp/src/error.rs:93`, currently unused at the dispatch layer) is tracked as an m7-1 follow-up.

2. **L2.3 `double_init_rejected`** asserts only protocol-version invariance across a second `initialize` call, NOT rejection. rmcp 1.5.0 dispatches the second `initialize` through the regular request handler returning a fresh `InitializeResult` (no "already initialized" gate). Materially weaker than spec line 57. Wire `init_state_error()` when m7-1 adds a layered request-router; tracked as an m7-1 follow-up.

3. **L2 real-pipe `large_response_crosses_pipe_buffer`** substitutes a 32-frame `tools/list` burst (cumulative >64 KiB) for the spec's single >64 KiB `tools/call{name:"ls"}` response. Reason: m7-1 ships `ls` as `not_implemented_result()` (-32601), so single-frame >64 KiB is unreachable today. The contract under test (rmcp framer drains under pipe back-pressure without dropping or reordering bytes) is preserved by the burst form. Switch to single-frame when m7-3 lands the real `ls` impl. Inline TODO in all 3 real-pipe files points at this.

4. **L2 real-pipe `client_stderr_close_does_not_panic_server`** implements "close client stderr" as `Stdio::null()` at child-spawn time, NOT a mid-session `CloseHandle`/`dup2`. Reason: closing stderr after the child starts races the `tracing_subscriber::fmt().with_writer(stderr).init()` call inside `grex serve`, producing flaky tests. Both forms exercise the same invariant from `.omne/cfg/mcp.md` §Stdio discipline ("server tolerates an unreadable stderr").

5. **L3 parity** asserts shape-equal `ParitySignal` (`Unimplemented` | `PackOpError`) rather than the spec's byte-equal `normalize(cli_json) == normalize(mcp_json)`. Reason: m7-1 ships `--json` parsed on `GlobalFlags` (`crates/grex/src/cli/args.rs:16`) but unwired into any verb (`crates/grex/src/cli/verbs/sync.rs:30` ignores `global.json`; 9 stub verbs print `"grex <verb>: unimplemented (M1 scaffold)"`). The current contract proves both surfaces agree on outcome class for every verb in `VERBS_EXPOSED` (10 stubs → `Unimplemented`, `sync` against a missing pack root → `PackOpError`). Flip to byte-equal once CLI `--json` wiring lands (m7-4 scope alongside real verb impls); call sites in `crates/grex-mcp/tests/parity.rs` stay unchanged.

## Source-of-truth links

- [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md) — tool catalog, cancellation, session model, stdio discipline.
- [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) — 5-tier lock ordering shared across CLI + MCP.
- [`.omne/cfg/architecture.md`](../../../.omne/cfg/architecture.md) §Workspace — "sub-crates avoided" rule; tests nest under `crates/grex-mcp/tests/`.
- [`.omne/cfg/test-plan.md`](../../../.omne/cfg/test-plan.md) §MCP coverage — L1 – L8 layering; this change covers L2 – L5.
- [`milestone.md`](../../../milestone.md) §M7 — MCP server plus test harness plus proof.
- [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §M7 — MCP v1 requirements.
