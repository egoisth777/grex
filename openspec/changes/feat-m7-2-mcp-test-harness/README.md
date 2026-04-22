# feat-m7-2-mcp-test-harness

**Status**: draft

5-layer test coverage for the `grex-mcp` server (feat-m7-1). Covers layers **L2 – L5** — E2E handshake, CLI ↔ MCP parity, concurrent stress, cancellation chaos. **L1** (inline unit) belongs to feat-m7-1; **L6 – L8** (Inspector, mcp-protocol-validator, fuzz) move to feat-m7-3.

Tests nest under `crates/grex-mcp/tests/` — **no separate `grex-mcp-tests` crate**. That would violate the "sub-crates avoided" rule in `.omne/cfg/architecture.md` §Workspace.

## Layer-to-file map

| Layer | File | Harness |
|---|---|---|
| L1 (unit) | `crates/grex-mcp/src/**` inline `#[cfg(test)]` | owned by feat-m7-1 |
| L2 (E2E handshake) | `crates/grex-mcp/tests/handshake.rs` | `tokio::io::duplex(4096)` — zero subprocess |
| L2 (real-pipe) | `tests/real_pipe_linux.rs` + `tests/real_pipe_windows.rs` | one `assert_cmd`-spawned `grex serve` per OS |
| L3 (parity) | `crates/grex-mcp/tests/parity.rs` | CLI subprocess vs in-process MCP, 11 verbs |
| L4 (stress) | `crates/grex-mcp/tests/stress.rs` | N = 100 × 11 + `tokio::sync::Barrier` |
| L5 (cancel) | `crates/grex-mcp/tests/cancel.rs` | `tools/call` + immediate `notifications/cancelled` |

Shared fixtures + `normalize()` live in `crates/grex-mcp/tests/common/mod.rs`.

## Key design calls

- **Duplex-first.** L2 uses `tokio::io::duplex(4096)` paired halves so the bulk of E2E coverage runs in-process. Real OS pipes get exactly one guard test per OS — enough to catch kernel-buffer / stderr-closure regressions `duplex` cannot reproduce.
- **11 tools, not 12.** `serve` is the server, not a tool; `teardown` is a plugin lifecycle hook of `rm`, not a user-invokable verb. `VERBS_EXPOSED` is the single const that drives every `len()` assertion.
- **Inequality, not equality, for `tools/list.len()`.** `>= VERBS_EXPOSED.len()` so future MCP-only tools don't retrip the check.
- **Helper function, not a macro, for parity.** An earlier draft floated `parity_test!`; review rejected it — opaque error spans, harder IDE nav, over-abstract for 11 cases.
- **Normaliser surface: 2 tokens only.** `<TS>` + `<PATH>`. `<ID>`, `<PID>`, `<SHA>` are opt-in, each requires a linked failing test to justify. Minimum surface keeps false-positives low.
- **Barrier in the handler body.** L4 saturation is proven at the semaphore, not at spawn — a test-only `#[cfg(test)]` hook lets the barrier fire after permit acquire.
- **`high_water` asserts split.** Separate `>= PARALLEL` (saturation) and `<= PARALLEL` (no over-subscribe) assertions. Exact-equality is flaky under slow CI; splitting disambiguates failures.
- **OS-specific cancel budgets.** 250 ms on Linux / macOS, 500 ms on Windows. Windows file-lock cancellation latency is OS-driven (MED-5), not grex-driven.
- **CI repeat policy = 3×.** Stress tests run 3× back-to-back in one job; a single failure across the 3 fails the job.

## Artifacts

- [`spec.md`](./spec.md) — full design, cases, acceptance.
- [`tasks.md`](./tasks.md) — 8-stage TDD plan.

## Dependencies

- **Prior**: feat-m7-1 (server + cancellable tool API + `VERBS_EXPOSED`); M6 feat-m6-1 (Scheduler), feat-m6-2 (PackLock), feat-m6-3 (lock-order proof); M5 pack-type plugin system.
- **Next**: feat-m7-3 (L6 Inspector, L7 mcp-protocol-validator, L8 fuzz).

## Source-of-truth links

- [`.omne/cfg/mcp.md`](../../../.omne/cfg/mcp.md) — tool catalog, cancellation semantics, stdio discipline.
- [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) — 5-tier lock order shared across CLI + MCP.
- [`.omne/cfg/architecture.md`](../../../.omne/cfg/architecture.md) §Workspace — sub-crate prohibition.
- [`.omne/cfg/test-plan.md`](../../../.omne/cfg/test-plan.md) §MCP coverage — L1 – L8 layering baseline.
