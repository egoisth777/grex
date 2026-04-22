# feat-m7-1-mcp-server

**Status**: draft
**Milestone**: M7
**Artifacts**: [`spec.md`](./spec.md) · [`tasks.md`](./tasks.md)

## One-line

Embed an MCP 2025-06-18 stdio server inside `grex serve` (new crate `crates/grex-mcp/`) exposing 11 CLI verbs as `tools/call` targets, with cancellation threaded through M6's scheduler + pack-lock.

## Scope

- New crate `crates/grex-mcp/` built on `rmcp = "1.5"` (`#[tool]` + `Parameters<T: JsonSchema>`).
- 11 MCP tools = frozen verb set minus `serve` (which *is* the server) and minus `teardown` (plugin lifecycle, not a user verb): `init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `import`, `run`, `exec`.
- `grex serve` launches the server on stdio; one process = one MCP client session.
- `Arc<Scheduler>` shared across tool invocations; M6 5-tier lock order (workspace-sync → semaphore → pack-lock → backend → manifest) honored verbatim.
- `notifications/cancelled` threads `tokio_util::sync::CancellationToken` into two new primitives: `Scheduler::acquire_cancellable` and `PackLock::acquire_cancellable`.
- Agent-safety annotations (`readOnlyHint` + `destructiveHint`) on every tool. `exec --shell` removed from the MCP surface (CLI keeps it).
- Tracing goes to **stderr only**; stdout is the JSON-RPC wire.

## Non-goals

- No multi-client sessions (deferred).
- No HTTP / SSE transport; stdio only.
- No OAuth / auth; process-level fs permissions.
- No MCP resources / prompts APIs (tools-only).
- No `notifications/progress` emission (deferred).
- No `exec --shell` re-exposure via opt-in (deferred).
- No `-32002` code-split (disambiguation by `data.kind` for now).
- No Lean4 modelling of the MCP state machine (M6 proof covers the primitives; MCP is transport).

## Dependencies

- **Prior**: M5 plugin system (PRs #22 + #23, 2026-04-21); M6 scheduler (feat-m6-1); M6 pack-lock (feat-m6-2); M6 Lean proof (feat-m6-3).
- **SSOT**: [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md) (rewritten 2026-04-21, Path B); [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) (lock ordering); [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §Success criteria #2.

## Delivery plan (8 TDD stages)

See [`tasks.md`](./tasks.md) for the per-stage checklist.

1. Workspace deps (`rmcp 1.5`, `tokio-util 0.7`) + empty `crates/grex-mcp/` scaffold.
2. Thread `cancel: &CancellationToken` through all 11 verb `run()` signatures (mechanical refactor; CLI passes never-cancel sentinel).
3. `Scheduler::acquire_cancellable` — `tokio::select!` over permit acquire + token.
4. `PackLock::acquire_cancellable` — `spawn_blocking` + cancellation contract (documented OS-thread leak window).
5. Server skeleton: `initialize` / `tools/list` (empty) / `shutdown` + stderr-only tracing.
6. 11 tool impls with agent-safety annotations + error-code mapping (including `-32002` `data.kind` disambiguation).
7. `notifications/cancelled` handler wired to per-request `CancellationToken`.
8. `grex serve` CLI wiring + subprocess smoke test + `mcp-validator` CI job.

## Acceptance criteria (summary)

Full list in [`spec.md`](./spec.md) §Acceptance. Headline items:

- `grex serve` completes a 2025-06-18 handshake with `mcp-validator`.
- `tools/list` returns exactly 11 tools (compile-time asserted).
- Every tool carries both annotation hints; destructive set = `{rm, run, exec}`.
- `exec` tool has no `shell` field; sending one returns `-32602`.
- `notifications/cancelled` aborts an in-flight `sync` within 200 ms, returning `-32800`.
- Batch request arrays return `-32600`.
- stdout carries JSON-RPC only; tracing on stderr.
- `cargo test --workspace` + `cargo clippy -- -D warnings` green.
- No M5 / M6 regressions.

## Risks / open questions

- **Blocking `fd-lock` + cancellation**: Stage 4 accepts a one-OS-thread leak window per cancelled pack-lock wait. Documented inline + in the error module. Revisit if `fd-lock` gains async support.
- **`-32002` dual-use**: current design disambiguates via `data.kind`. If downstream clients rely strictly on code-only routing, we split into `-32002` (pack-op) + `-32006` (init-state) in a follow-up.
- **rmcp 1.5 API stability**: pinned exactly to `1.5`; a minor-version bump may require schema-macro adjustments. Tracked as a maintenance item post-M7.
