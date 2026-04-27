# mcp — embedded MCP server

`grex serve` launches an embedded stdio server speaking **MCP 2025-06-18** natively. Every CLI verb except `serve` is exposed as an MCP tool invoked via `tools/call`. No custom JSON-RPC dialect, no `grex.*` methods, no batching.

## Goal

Agent-native control surface. MCP tool handlers call the same library entrypoints the CLI dispatcher calls — **no subprocess wrapper**. Single-process observability, shared tokio runtime, manifest cache persists across requests, scheduler + pack-lock primitives shared verbatim.

## Transport

- **Wire**: stdio, newline-delimited JSON per MCP 2025-06-18 (one JSON-RPC message per line, LF-terminated). `rmcp` `transport-io` default framer.
- **Encoding**: UTF-8.
- **Protocol version**: `2025-06-18` — returned from `initialize`, asserted by clients and mcp-protocol-validator.
- **Batching**: **NOT supported.** MCP 2025-06-18 rejects JSON-RPC batch arrays. Server MUST return `-32600 Invalid Request` if `[req, req, …]` arrives.
- **Stdout discipline**: stdout is reserved exclusively for the JSON-RPC wire. Tracing, logs, and diagnostics go to **stderr only**. Any accidental stdout write is a server bug.

## Protocol lifecycle

Only MCP-standard methods are accepted.

| Method / notification | Direction | Purpose |
|---|---|---|
| `initialize` (req) | client → server | Capability negotiation, protocol-version agreement. |
| `notifications/initialized` | client → server | Client ready to send requests. |
| `tools/list` (req) | client → server | Return the 11 tools with JSON-Schema. |
| `tools/call` (req) | client → server | Invoke a tool by `name`. |
| `notifications/cancelled` | client → server | Cancel an in-flight `tools/call` by `requestId`. |
| `notifications/progress` | server → client | Optional per-operation progress (deferred). |
| `shutdown` (req) | client → server | Drain in-flight tasks then exit. |

### Handshake

```json
→ {"jsonrpc":"2.0","id":1,"method":"initialize",
   "params":{"protocolVersion":"2025-06-18",
             "clientInfo":{"name":"claude-code","version":"x"},
             "capabilities":{}}}
← {"jsonrpc":"2.0","id":1,
   "result":{"protocolVersion":"2025-06-18",
             "serverInfo":{"name":"grex","version":"<workspace-version>"},
             "capabilities":{"tools":{"listChanged":false}}}}
→ {"jsonrpc":"2.0","method":"notifications/initialized"}
```

### `tools/call` example

```json
→ {"jsonrpc":"2.0","id":42,"method":"tools/call",
   "params":{"name":"sync","arguments":{"recursive":true,"parallel":8}}}
← {"jsonrpc":"2.0","id":42,
   "result":{"content":[{"type":"text","text":"<json-result>"}],"isError":false}}
```

## Tool catalog (11 tools)

Frozen CLI verb set: `init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `serve`, `import`, `run`, `exec` (12 verbs).

**Exposed as MCP tools: 11.** `serve` is the server itself → not a tool. `teardown` is a plugin lifecycle hook of `rm`, **not** a user-invokable verb → not a tool. The constant `VERBS_11_EXPOSED_AS_TOOLS` is defined in `grex-mcp` and drives every `len()` assertion.

| Tool name | Description (for `tools/list`) | `readOnlyHint` | `destructiveHint` |
|---|---|---|---|
| `init` | Initialise a grex workspace. | false | false |
| `add` | Register and clone a pack. | false | false |
| `rm` | Unregister a pack (runs teardown unless `--skip-teardown`). | false | **true** |
| `ls` | List registered packs. | true | false |
| `status` | Report drift + installed state. | true | false |
| `sync` | Sync all packs recursively. | false | false |
| `update` | Update one or more packs (re-resolve refs, reinstall). | false | false |
| `doctor` | Check manifest + gitignore + on-disk drift. | true | false |
| `import` | Import packs from a `REPOS.json` meta-repo index. | false | false |
| `run` | Run a declared action across matching packs. | false | **true** |
| `exec` | Execute a command across matching packs. | false | **true** |

Param and result shapes mirror the `--json` output of each CLI verb field-for-field. Every `*Params` struct derives `JsonSchema`; rmcp auto-publishes schemas in `tools/list`.

**`exec --shell` is removed from the MCP surface.** Arbitrary shell interpolation is a dangerous capability for an agent. The flag remains on the CLI but is absent from the `exec` tool's param schema. Reintroduction requires an explicit per-session capability opt-in (deferred).

## Cancellation

MCP-standard `notifications/cancelled` with `requestId`. No custom `grex.cancel` method.

```json
→ {"jsonrpc":"2.0","method":"notifications/cancelled",
   "params":{"requestId":42,"reason":"user aborted"}}
```

Server signals the matching request's `tokio_util::sync::CancellationToken`. Every tool handler propagates the token through:

- `Scheduler::acquire_cancellable(&CancellationToken)` — `tokio::select!` between `semaphore.acquire_owned()` and `cancel.cancelled()`.
- `PackLock::acquire_cancellable(path, &CancellationToken)` — same pattern; breaks the backoff loop on cancel.
- Inner action / pack-type dispatch loop — checks `cancel.is_cancelled()` between steps.

Cancelled request returns `-32800 request cancelled` (MCP-standard reserved code).

## Progress

`notifications/progress` is **optional and deferred**. v1 tool calls return only a final `CallToolResult`. Progress wiring from `sync` / `update` / `run` / `exec` handlers (tracing span → progress bridge) lands in a later milestone.

## Error codes

Standard JSON-RPC 2.0 codes + MCP-standard `-32800` + grex-reserved `-32001..-32005` for pack-op failures.

| Code | Source | Meaning |
|---|---|---|
| `-32600` | JSON-RPC | Invalid Request (malformed envelope; batch array) |
| `-32601` | JSON-RPC | Method / tool not found |
| `-32602` | JSON-RPC | Invalid params (deserialization failure; disallowed flag) |
| `-32603` | JSON-RPC | Internal error (catch-all) |
| `-32800` | MCP | Request cancelled |
| `-32001` | grex | Manifest integrity failure |
| `-32002` | grex | Pack op failed **or** initialization-state error (see note) |
| `-32003` | grex | Lock contention |
| `-32004` | grex | Drift detected |
| `-32005` | grex | Unknown action / pack-type (plugin missing) |

**Dual use of `-32002`**: same code surfaces (a) a user-level pack-op failure returned inside a completed `tools/call`, and (b) an initialization-state protocol error ("not initialized" / "already initialized") returned from the envelope. Disambiguation is by `data.kind`: `"pack_op"` vs `"init_state"`. Splitting into two codes is a future item.

## Agent-safety annotations

Every tool in `tools/list` declares both `annotations.readOnlyHint` and `annotations.destructiveHint`. See the catalog table above.

- Read-only tools (`ls`, `status`, `doctor`) are safe for unattended agent use.
- Destructive tools (`rm`, `run`, `exec`) carry `destructiveHint: true` so policy layers (claude-code, IDE clients) can prompt the user or gate them behind approval.
- The annotations are advisory hints, not enforcement — enforcement is the client's responsibility.

## Session model

**One `grex serve` process = one MCP client session.** Concurrent multi-client sessions over a single server are a future milestone. Rationale:

- stdio transport is inherently single-peer.
- Manifest cache, scheduler permit pool, and pack-lock table are scoped to the process — a second client would need explicit session partitioning.
- Agent-harness pattern (Claude Code, Cursor, etc.) spawns one server per workspace anyway.

## Concurrency integration

MCP tool handlers share one `Arc<Scheduler>` for the server lifetime — concurrent `tools/call` invocations respect `--parallel` exactly like local CLI invocations. Manifest cache is reused across requests. `ExecCtx` is built fresh per call, borrowing the shared scheduler + registry handles.

**5-tier lock ordering invariant (M6).** Tool handlers MUST acquire concurrency primitives in the fixed order documented in `.omne/cfg/concurrency.md`:

1. workspace-sync lock
2. scheduler semaphore permit
3. pack-lock (per pack)
4. backend (git) lock
5. manifest lock

No handler may invert this order. Enforced at runtime by acquisition helpers and statically by M6's Lean4 proof (`feat-m6-3`).

## Launch

`grex serve` — no `--mcp` flag; the command **is** the MCP server. Flags:

- `--manifest <path>` — override manifest path (captured at launch; clients cannot override mid-session).
- Inherits global `--parallel N` from the grex CLI root.

Security posture:

- stdio only. No network listener.
- Filesystem ops confined to the workspace root.
- Session inherits process file permissions; no privilege escalation.

## Implementation stack

- **Server framework**: `rmcp = "1.5"` (official Rust MCP SDK). Provides transport framing, `initialize` negotiation, `tools/list` schema publication, and `notifications/cancelled` plumbing out of the box.
- **Schema generation**: `schemars` — every tool's `*Params` struct derives `JsonSchema`.
- **Cancellation**: `tokio_util::sync::CancellationToken` threaded through `Scheduler` and `PackLock`.
- **Crate layout**: `crates/grex-mcp/` (server + tool handlers) + `crates/grex/src/cli/verbs/serve.rs` (thin launch shim).

Testing:

- `crates/grex-mcp/src/**` — inline `#[cfg(test)]` unit tests (routing, schema gen, error mapping).
- `crates/grex-mcp/tests/**` — integration tests via `tokio::io::duplex`.
- `.github/workflows/ci.yml` — `mcp-validator` job runs `mcp-protocol-validator` against a release build of `grex serve`.

## Out-of-scope / future

- Multi-client sessions over a single server process.
- `notifications/progress` emission from long-running tool handlers.
- `exec --shell` re-exposure via per-session capability opt-in.
- Splitting `-32002` into distinct pack-op vs init-state codes.
- Remote transports (HTTP/SSE); stdio is the only v1 transport.
