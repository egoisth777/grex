# feat-m7-1 — MCP stdio server (`grex serve`)

**Status**: draft
**Milestone**: M7 (see [`../../../milestone.md`](../../../milestone.md) §M7)
**Depends on**: M5 plugin system (PRs #22 + #23, closed 2026-04-21); M6 scheduler + per-pack lock (feat-m6-1 / feat-m6-2); `.omne/cfg/mcp.md` (rewritten 2026-04-21, Path B MCP-native).

## Motivation

Grex has no agent-facing control surface today — every verb is CLI-only. Agents that want programmatic drive must shell-out and re-parse human text, losing typed results, cancellation, and the scheduler state that a single long-lived process would preserve.

`.omne/cfg/mcp.md` now pins the design to **Path B**: embed an MCP 2025-06-18 stdio server inside `grex serve`, speak the wire natively (no custom JSON-RPC dialect), and reuse the library entrypoints the CLI dispatcher already calls. This change lands that server.

The server is load-bearing for `openspec/feat-grex/spec.md` success criterion #2 ("agent-driven control via MCP tools"). M6's lock-ordering invariant must survive the crossing — MCP handlers acquire the same `Scheduler` + `PackLock` primitives the CLI uses, in the same fixed order. Cancellation is new in M7 and drives one additional API pair (`acquire_cancellable`) on both primitives.

## Goal

1. New crate `crates/grex-mcp/` hosting the server + 11 tool handlers.
2. `grex serve` launches the server on stdio; no `--mcp` flag (the command *is* the server).
3. All 11 user-facing verbs (`init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `import`, `run`, `exec`) reachable via `tools/call`. `serve` is the server itself, not a tool; `teardown` is a plugin lifecycle hook of `rm`, not a verb.
4. MCP tool handlers share one `Arc<Scheduler>` and obey the M6 5-tier lock ordering verbatim: workspace-sync → semaphore → pack-lock → backend → manifest.
5. `notifications/cancelled` threads a `tokio_util::sync::CancellationToken` into the acquire path of both `Scheduler` and `PackLock`; in-flight blocking `fd-lock` writes are cancelled by dropping their `spawn_blocking` join handle. Client-side cancellation is emitted via rmcp's MCP-native API (`peer().send_request().cancel(reason)` on `rmcp 1.5.0`); see §Cancellation for the verified-at-Stage-1 fallback path if that helper is not published.
6. `exec --shell` is **absent** from the MCP param schema. CLI keeps it; agent surface refuses it.

## Design

### Crate layout

```
crates/grex-mcp/
├── Cargo.toml                        # workspace = true pinning
└── src/
    ├── lib.rs                        # GrexMcpServer + ServerState
    ├── error.rs                      # Rust err → rmcp::ErrorData mapping
    └── tools/
        ├── mod.rs                    # #[tool] dispatcher + VERBS_11_EXPOSED_AS_TOOLS
        ├── init.rs   add.rs   rm.rs
        ├── ls.rs     status.rs  sync.rs
        ├── update.rs doctor.rs  import.rs
        ├── run.rs    exec.rs
```

### Workspace deps

Root `Cargo.toml`:

```toml
[workspace.dependencies]
rmcp       = "1.5"                                        # official MCP SDK
tokio-util = { version = "0.7", features = ["rt"] }       # CancellationToken
schemars   = "0.8"                                        # already present; reconfirm
```

`crates/grex-mcp/Cargo.toml` consumes all three via `workspace = true`.

### Server skeleton

File: `crates/grex-mcp/src/lib.rs`.

```rust
pub struct ServerState {
    pub scheduler: Arc<Scheduler>,
    pub registry:  Arc<Registry>,         // ActionPlugin + PackTypePlugin
    pub manifest:  Arc<ManifestCache>,
    pub workspace: PathBuf,
}

pub struct GrexMcpServer {
    state: Arc<ServerState>,
}

impl GrexMcpServer {
    pub fn new(state: ServerState) -> Self { ... }

    /// Drive the server on the given stdio handle. Returns when the client
    /// sends `shutdown` or the transport closes.
    pub async fn run(self, transport: impl Transport) -> Result<(), rmcp::Error>;
}
```

rmcp 1.5's `#[tool]` macro + `Parameters<T: JsonSchema>` handlers auto-publish JSON-Schema in `tools/list`. No hand-rolled schema.

### Agent-safety annotations

Every `#[tool]` declaration sets both `annotations.readOnlyHint` and `annotations.destructiveHint` per the table in `.omne/cfg/mcp.md`. `exec` is advertised **without** the `--shell` field in its `*Params` struct; reintroduction is a future per-session capability opt-in.

### Cancellable API additions

Two new methods, one per M6 primitive.

File: `crates/grex-core/src/scheduler.rs`.

```rust
use tokio_util::sync::CancellationToken;

impl Scheduler {
    /// `acquire()` + cancel-awareness. Returns `Cancelled` if the token fires
    /// before a permit is available; the pending acquire is dropped cleanly
    /// (no permit leak — tokio drops the `Acquire` future).
    pub async fn acquire_cancellable(
        &self,
        cancel: &CancellationToken,
    ) -> Result<OwnedSemaphorePermit, Cancelled>;
}

pub struct Cancelled;  // lightweight marker; maps to -32800 at MCP edge
```

Body sketch:

```rust
tokio::select! {
    permit = self.permits.clone().acquire_owned() => Ok(permit?),
    _      = cancel.cancelled()                    => Err(Cancelled),
}
```

File: `crates/grex-core/src/pack_lock.rs`.

```rust
impl PackLock {
    pub async fn acquire_cancellable(
        self,
        cancel: &CancellationToken,
    ) -> Result<PackLockHold, PackLockErrorOrCancelled>;
}

pub enum PackLockErrorOrCancelled {
    Cancelled,
    Lock(PackLockError),
}
```

Ownership model matches the existing `acquire_async(self)` in `pack_lock.rs:179` — `acquire_cancellable` consumes the `PackLock`, reusing its internal boxed-fd + `transmute` lifetime dance. No second fd-open path is introduced. Implementation uses `tokio::task::spawn_blocking` to run `fd_lock::RwLock::write()` (which can block at the OS level on contended NFS / Windows handles). The outer future is a `tokio::select!` between `cancel.cancelled()` and the join handle. On cancel we **drop** the join handle — the OS thread continues to completion because `fd_lock::write()` cannot be interrupted mid-syscall; the acquired guard is dropped immediately on the blocking thread's return, releasing the fd-lock. This is the documented leak: **one OS worker thread is briefly held past cancel**; acceptable per rmcp semantics ("cancelled requests may still complete server-side"). Documented inline + in the error module.

### Cancellation — client emit path + server receive path

**Client emit (MCP-native).** `rmcp 1.5.0` exposes cancellation on the `Client` peer as `peer().send_request().cancel(reason)` — this is the preferred surface for integration tests that drive `notifications/cancelled` from the Rust side. **Fallback**: if Stage 1 `cargo doc -p rmcp` inspection shows that helper is absent on the 1.5.0 client builder, tests construct the `notifications/cancelled` envelope as raw JSON and write it directly to the stdio transport. Either path emits the same wire frame; the spec does not depend on which is used.

**Server receive.** Tool handlers reached via rmcp's `#[tool]` macro receive a `RequestContext` whose `ct: tokio_util::sync::CancellationToken` field is the request's cancellation token. Forward that token into every core verb's `run(..., cancel: &CancellationToken)` entry; from there it reaches `Scheduler::acquire_cancellable` and `PackLock::acquire_cancellable`.

### CancellationToken threading

All 11 verb `run()` signatures (plus `teardown` for plugin-lifecycle parity) gain `cancel: &CancellationToken` as their final parameter. Examples:

```rust
// crates/grex-core/src/sync/mod.rs
pub async fn run(
    ctx: &ExecCtx<'_>,
    opts: SyncOpts,
    cancel: &CancellationToken,   // new
) -> Result<SyncReport, SyncError>;
```

CLI callers pass `CancellationToken::new()` (never cancelled — preserves today's behaviour under Ctrl-C, which tokio handles separately). MCP tool handlers forward the request's token from rmcp's dispatch context. One stage of this change is a pure mechanical refactor with no behaviour change for CLI users.

### Session & concurrency model

- One `grex serve` process = one MCP client session. Multi-client deferred.
- `Arc<Scheduler>` created once at server boot, shared across every `tools/call`.
- Lock ordering invariant (workspace-sync → semaphore → pack-lock → backend → manifest) is enforced by the same acquisition helpers the CLI uses — no MCP-specific shortcut paths.
- Manifest cache persists for the server lifetime; each `tools/call` re-borrows it.

### Tracing discipline

stdout is reserved for the JSON-RPC wire. `tracing_subscriber::fmt().with_writer(std::io::stderr)` at boot. Any stdout write from a tool body is a bug. CI lint: grep for `println!` / `print!` under `crates/grex-mcp/src/` must return 0 matches (enforced in Stage 5 tests).

### `grex serve` wiring

File: `crates/grex/src/cli/verbs/serve.rs` currently a stub. After this change:

```rust
pub async fn run(ctx: ExecCtx<'_>, opts: ServeOpts) -> Result<(), ServeError> {
    let state = ServerState::from_ctx(&ctx)?;
    let transport = rmcp::transport::stdio();      // framed stdio
    GrexMcpServer::new(state).run(transport).await?;
    Ok(())
}
```

**ServeArgs MUST NOT re-declare `--parallel`.** The flag is inherited from the global `GlobalArgs` scope (shared across all verbs, landed in M6). A verb-local re-declaration is a clap parse-time conflict (`ArgConflict`) and will fail startup. `ServeArgs` contains only verb-local fields (e.g. `--manifest <path>` override); parallelism reads from `ctx.parallel` populated by the global parser.

### Error-code overload (`-32002`)

Per `.omne/cfg/mcp.md` §Error codes, `-32002` is dual-use:

1. Envelope-level initialize-state error ("not initialized" / "already initialized") raised by rmcp's state machine — `data.kind = "init_state"`.
2. Grex pack-op failure raised inside a completed `tools/call` with `isError: true` — `data.kind = "pack_op"`.

`crates/grex-mcp/src/error.rs` centralises the mapping and stamps `data.kind` on every `-32002` path. Unit tests assert disambiguation. Splitting into two codes is a documented future item; not in this change.

## File / module targets

| Concrete path | Change |
|---|---|
| `Cargo.toml` (root) | Add `rmcp = "1.5"`, `tokio-util = "0.7"` to `[workspace.dependencies]`. |
| `crates/grex-mcp/Cargo.toml` | New — consumes workspace deps. |
| `crates/grex-mcp/src/lib.rs` | New — `GrexMcpServer`, `ServerState`, `run()`. |
| `crates/grex-mcp/src/error.rs` | New — Rust err → `rmcp::ErrorData`; `-32002` disambiguation. |
| `crates/grex-mcp/src/tools/mod.rs` | New — `#[tool]` dispatcher, `VERBS_11_EXPOSED_AS_TOOLS`. |
| `crates/grex-mcp/src/tools/{init,add,rm,ls,status,sync,update,doctor,import,run,exec}.rs` | New — one file per tool. |
| `crates/grex-mcp/tests/` | New — integration via `tokio::io::duplex`. |
| `crates/grex-core/src/scheduler.rs` | Add `acquire_cancellable` + `Cancelled`. |
| `crates/grex-core/src/pack_lock.rs` | Add `acquire_cancellable` + `PackLockErrorOrCancelled`. |
| `crates/grex-core/src/sync/mod.rs` | Thread `cancel: &CancellationToken` into `run()`. |
| `crates/grex-core/src/{init,add,rm,ls,status,update,doctor,import,run,exec}.rs` (whichever modules host each verb) | Thread `cancel: &CancellationToken`. |
| `crates/grex-core/Cargo.toml` | Add `tokio-util` workspace dep. |
| `crates/grex/src/cli/verbs/serve.rs` | Replace stub with `GrexMcpServer::run(stdio)`. |
| `crates/grex/src/cli/verbs/{init,add,rm,ls,status,sync,update,doctor,import,run,exec}.rs` | Pass `&CancellationToken::new()` to core. |
| `.omne/cfg/mcp.md` | Referenced, NOT modified. |

## Test plan

### Unit

`crates/grex-core/src/scheduler.rs` `#[cfg(test)]`:
- `acquire_cancellable_returns_permit_when_not_cancelled`
- `acquire_cancellable_returns_cancelled_if_token_fires_before_permit`
- `acquire_cancellable_dropped_future_does_not_leak_permit` — 4 permits, cancel 100 waiters, assert `available_permits() == 4` after.
- `acquire_cancellable_cancel_after_success_is_no_op`

`crates/grex-core/src/pack_lock.rs` `#[cfg(test)]`:
- `acquire_cancellable_happy_path`
- `acquire_cancellable_cancel_during_blocking_fd_lock_returns_cancelled` — hold the lock in task A, call `acquire_cancellable` in task B, cancel B's token, assert `Cancelled` within 50 ms.
- `acquire_cancellable_spawn_blocking_thread_releases_guard_when_it_finally_unblocks` — regression for the documented OS-thread leak window.

`crates/grex-mcp/src/tools/mod.rs` `#[cfg(test)]`:
- `tools_list_advertises_exactly_11` — `VERBS_11_EXPOSED_AS_TOOLS.len() == 11`.
- `exec_tool_schema_has_no_shell_field` — serde introspect.
- `every_tool_has_both_annotation_hints` — `readOnlyHint` + `destructiveHint` both present.
- `destructive_tools_are_rm_run_exec_only`.

`crates/grex-mcp/src/error.rs` `#[cfg(test)]`:
- `packop_failure_maps_to_minus_32002_with_kind_pack_op`
- `init_state_failure_maps_to_minus_32002_with_kind_init_state`
- `cancelled_maps_to_minus_32800`.

### Integration

`crates/grex-mcp/tests/handshake.rs`:
- `initialize_handshake_accepts_2025_06_18` — round-trip via `tokio::io::duplex`.
- `batch_request_array_is_rejected_with_minus_32600`.
- `tools_list_returns_11_tools_with_schemas`.
- `shutdown_drains_in_flight`.

`crates/grex-mcp/tests/cancellation.rs`:
- `notifications_cancelled_aborts_inflight_sync` — launch `sync` against a fixture that blocks on a mock fetch, send `notifications/cancelled`, assert `-32800` within 200 ms.
- `cancel_after_result_is_ignored`.

`crates/grex-mcp/tests/lock_ordering.rs`:
- `concurrent_tool_calls_share_arc_scheduler` — two concurrent `sync` tool calls on disjoint packs with `--parallel 2` at server boot; assert both complete, max-observed in-flight ≤ 2.
- `pack_lock_acquired_after_permit_not_before` — tracing-span inspection across one `sync` tool call.

`crates/grex-mcp/tests/agent_safety.rs`:
- `exec_tool_rejects_shell_param_with_minus_32602` — client sends `{"shell": "bash -c ..."}` in args, server returns Invalid Params.

`crates/grex/tests/serve_smoke.rs` (new):
- `grex_serve_subprocess_responds_to_tools_list` — spawn `grex serve` as subprocess, pipe initialize + tools/list over stdio, assert 11-tool response, no stdout pollution outside wire.

### Protocol-validator CI

`.github/workflows/ci.yml` gains a `mcp-validator` job running `mcp-validator` against a release build of `grex serve`. Blocks merge on any MCP 2025-06-18 non-conformance.

## Non-goals

- **No multi-client sessions.** One server process = one client. Deferred.
- **No HTTP / SSE transport.** stdio only for v1.
- **No OAuth / auth layer.** Process-level fs permissions only.
- **No MCP resources / prompts APIs.** Tools-only surface.
- **No `notifications/progress` emission.** Tool calls return only a final `CallToolResult` in v1; progress wiring deferred.
- **No `exec --shell` on the MCP surface.** CLI keeps the flag.
- **No `teardown` tool.** Lifecycle hook of `rm`, not a user verb.
- **No splitting `-32002` into distinct pack-op vs init-state codes.** Disambiguation by `data.kind` for now.
- **No Lean4 modelling of the MCP state machine.** M6's proof covers the primitives; MCP is a pure transport wrapper over them.

## Dependencies

- **Prior changes**:
  - [`feat-m6-1-parallel-scheduler`](../feat-m6-1-parallel-scheduler/spec.md) — `Scheduler` struct + `Arc<Semaphore>` handle.
  - [`feat-m6-2`](../feat-m6-2-pack-lock/spec.md) — `PackLock::acquire` + per-pack exclusion.
  - [`feat-m6-3`](../feat-m6-3-lean-proof/spec.md) — `no_double_lock` invariant over the primitives this change re-uses.
  - M5 PRs #22 + #23 — `ActionPlugin` + `PackTypePlugin` registries consumed by tool handlers via `Arc<Registry>`.
- **SSOT docs**:
  - [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md) — wire, tool catalog, error codes, agent-safety annotations.
  - [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) — 5-tier lock ordering.
  - [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §Success criteria #2.
- **Crate additions**: `rmcp 1.5`, `tokio-util 0.7` (feature `rt`).

## Acceptance

1. `grex serve` launches on stdio and completes a 2025-06-18 handshake with `mcp-validator`.
2. `tools/list` returns exactly 11 tools; `VERBS_11_EXPOSED_AS_TOOLS.len() == 11` at compile time.
3. Every tool in the catalog carries both `readOnlyHint` and `destructiveHint`; destructive set = `{rm, run, exec}`.
4. `exec` tool schema contains no `shell` field; sending one yields `-32602`.
5. Concurrent `tools/call` invocations share one `Arc<Scheduler>` and honor `--parallel N` from server boot.
6. 5-tier lock order (workspace-sync → semaphore → pack-lock → backend → manifest) is observed in MCP-driven invocations — verified by tracing-span ordering test.
7. `notifications/cancelled` returns `-32800` within 200 ms for a `sync` in-flight on a blocked fetch.
8. Batch request arrays are rejected with `-32600`.
9. stdout is wire-only; `grex serve` subprocess smoke test detects zero non-JSON-RPC bytes on stdout.
10. `cargo clippy --all-targets --workspace -- -D warnings` clean.
11. `cargo test --workspace` green including the new crate's unit + integration tests.
12. `mcp-validator` CI job green.
13. No regressions on M5 + M6 test suites.

## Source-of-truth links

- [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md) — Path B spec (wire, tools, cancellation, errors, session).
- [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) — lock ordering invariant.
- [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §Success criteria #2 — MCP agent control.
- [`openspec/changes/feat-m6-1-parallel-scheduler/spec.md`](../feat-m6-1-parallel-scheduler/spec.md) — scheduler API shape, voice reference.
- [`milestone.md`](../../../milestone.md) §M7.
