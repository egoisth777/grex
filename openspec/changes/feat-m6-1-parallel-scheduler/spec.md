# feat-m6-1 — Parallel scheduler (bounded semaphore)

**Status**: draft
**Milestone**: M6 (see [`../../../milestone.md`](../../../milestone.md) §M6)
**Depends on**: M5 (PRs #22 + #23, closed 2026-04-21); M4 `ActionPlugin` + `PackTypePlugin` registries.

## Motivation

Today `sync::run` walks the pack graph and executes packs sequentially. On large meta-trees and/or networked fetches this wastes wall-clock time. `.omne/cfg/concurrency.md` specifies a bounded `tokio::sync::Semaphore` as mechanism (3) of five; this change lands it as its own unit, ahead of the per-pack lock (feat-m6-2) and Lean4 proof (feat-m6-3).

Correctness is load-bearing — getting a parallel scheduler wrong corrupts the manifest and races `.grex-lock`. The design is constrained by `.omne/cfg/concurrency.md` lock-ordering pseudocode:

```
workspace-sync → semaphore → pack-lock → repo-backend → manifest-lock
```

This change implements the **semaphore** tier only. Per-pack lock lives in feat-m6-2.

## Goal

1. Add `--parallel N` flag to `grex sync`.
2. Introduce `crates/grex-core/src/scheduler.rs` housing a `Scheduler` struct wrapping `Arc<Semaphore>`.
3. Thread `Arc<Semaphore>` through `ExecCtx` so plugin dispatch (both `ActionPlugin` and `PackTypePlugin`) can acquire permits where needed.
4. 100 concurrent synthetic syncs on disjoint trees must race-free; `--parallel 1` must reproduce current serial wall-order.

## Design

### CLI flag

File: `crates/grex/src/cli.rs` (the `SyncArgs` struct).

```
--parallel N    # bound in-flight pack ops
```

Semantics:
- `N` unspecified → default `num_cpus::get()` (clamped `>= 1`).
- `N == 0` → unbounded (semaphore permits = `Semaphore::MAX_PERMITS`).
- `N == 1` → serial fast-path. Scheduler still runs but acquires 1 permit per op, preserving today's wall-order.
- `N >= 2` → bounded parallel.
- Negative / non-numeric → clap rejects at parse.

`GREX_PARALLEL` env var honored only when flag absent (parity with `.omne/cfg/concurrency.md` §Runtime).

### `Scheduler` struct

File: `crates/grex-core/src/scheduler.rs` (new module).

```rust
pub struct Scheduler {
    permits: Arc<tokio::sync::Semaphore>,
}

impl Scheduler {
    pub fn new(parallel: usize) -> Self;      // parallel == 0 → MAX_PERMITS
    pub fn permits(&self) -> Arc<Semaphore>;  // cloned handle for ExecCtx
    pub async fn acquire(&self) -> OwnedSemaphorePermit;
}
```

No `run<F>(fut)` helper in this change — feat-m6-2 adds the pack-lock + permit combinator. This change just owns the semaphore and the clone surface.

### `ExecCtx` extension

File: `crates/grex-core/src/execute/ctx.rs`.

Add field:

```rust
pub struct ExecCtx<'a> {
    // ...existing M4/M5 fields...
    pub scheduler: Option<&'a Arc<Semaphore>>,
}
```

`Option` preserves backward-compat with existing `ExecCtx` constructors (feat-m6-2 will flip the callers that need to acquire). M5 `#[non_exhaustive]` policy applied.

### Plugin-dispatch wiring

`sync::run_actions` constructs `Scheduler::new(opts.parallel)`, clones `scheduler.permits()` into the shared `Arc`, and passes the handle into every `ExecCtx` built for action + pack-type dispatch. Downstream plugins (notably `MetaPlugin` once it parallelizes in a future change) read `ctx.scheduler` to bound child fan-out.

**In this change the dispatch remains sequential at the walker level** — the scheduler handle is plumbed but nothing calls `acquire()` yet. Acquisition sites land in feat-m6-2 alongside per-pack locking.

## File / module targets

| Concrete path | Change |
|---|---|
| `crates/grex/src/cli.rs` | Add `--parallel N` to `SyncArgs`; parse + validate. |
| `crates/grex/src/cli/verbs/sync.rs` | Read flag; default `num_cpus::get()`; honour `GREX_PARALLEL`. |
| `crates/grex-core/src/scheduler.rs` | New module — `Scheduler` struct. |
| `crates/grex-core/src/lib.rs` | `pub mod scheduler;` + re-export. |
| `crates/grex-core/src/execute/ctx.rs` | Add `scheduler: Option<&Arc<Semaphore>>` field. |
| `crates/grex-core/src/sync/mod.rs` | Construct `Scheduler`; plumb permit handle into `ExecCtx`. |
| `crates/grex-core/Cargo.toml` | Add `num_cpus` dep (if not already present via workspace). |
| `.omne/cfg/concurrency.md` | No change — change implements the existing spec. |

## Test plan

### Unit

`crates/grex-core/src/scheduler.rs` `#[cfg(test)]`:
- `scheduler_new_zero_uses_max_permits`
- `scheduler_new_one_serializes` — 10 tasks, assert at-most-1 concurrent via an `AtomicUsize` witness.
- `scheduler_new_four_bounds_concurrency` — 100 tasks, `--parallel 4`, assert max-observed ≤ 4.
- `scheduler_permits_clone_is_shared` — clone, assert acquire from either handle blocks the other.

### Integration

`crates/grex/tests/sync_parallel.rs` (new):
- `parallel_one_preserves_serial_wall_order` — 3 disjoint fixture packs, `--parallel 1`, assert pack-completion tracing spans are totally ordered (no overlap).
- `parallel_four_races_100_synthetic_syncs_without_corruption` — 100 disjoint fixture packs (meta root + 100 declarative children), `--parallel 4`. Assert: all packs sync OK; lockfile entries == 100; no duplicate `actions_hash` writes; wall-time sub-linear vs `--parallel 1`.

### Stress

`crates/grex/tests/sync_stress.rs` (new, `#[ignore]` by default, CI runs on nightly job):
- `stress_parallel_max_100_packs` — `--parallel 0` (unbounded), 100 packs. Assert no panic, no leaked permits, lockfile consistent.

### CLI parse

`crates/grex/tests/cli_parse.rs`:
- `parallel_default_is_num_cpus`
- `parallel_zero_is_unbounded`
- `parallel_one_is_serial`
- `parallel_rejects_negative`
- `parallel_env_var_used_when_flag_absent`
- `parallel_flag_wins_over_env_var`

## Non-goals

- **No work-stealing**. tokio's multi-thread runtime already distributes tasks; we don't layer a custom scheduler on top.
- **No dynamic scaling**. Permit count fixed at `Scheduler::new`; no up/down-sizing during a sync.
- **No per-action-type caps** (e.g. "max 2 concurrent `exec` actions"). Deferred per `.omne/cfg/concurrency.md` §Operational tuning to v1.x.
- **No per-pack `.grex-lock`** — that is feat-m6-2.
- **No Lean4 proof** — that is feat-m6-3.
- **No `meta` parallelism**. `MetaPlugin` remains sequential LIFO per M5 R-M5-out-1. Scheduler is plumbed so a future change can parallelize without an API break.
- **No workspace-sync lock changes**. Already shipped in M3 review PR #16.

## Dependencies

- **Prior changes**: M4 `ActionPlugin` trait + `Registry`; M5 `PackTypePlugin` trait + `PackTypeRegistry`; M3-review PR #16 workspace fd-lock.
- **Next changes**:
  - feat-m6-2 (per-pack lock) acquires `scheduler.permits()` then the per-pack lock inside plugin-trait methods.
  - feat-m6-3 (Lean4 proof) models the scheduler state machine proven here.
- **Crate**: `tokio::sync::Semaphore` already in the dep tree; `num_cpus` possibly needs adding.

## Acceptance

1. `grex sync --parallel 4` runs a 100-pack tree without panic, without manifest corruption, with lockfile line-count == pack-count.
2. `grex sync --parallel 1` reproduces the tracing-span total-order from a sequential sync on the same fixture (byte-compare of redacted span log).
3. `scheduler_permits_clone_is_shared` unit test passes — proves the `Arc<Semaphore>` clone contract.
4. `cargo clippy --all-targets --workspace -- -D warnings` clean.
5. `cargo test --workspace` green.
6. No regressions on existing M5 tests (470 workspace / 382 with `plugin-inventory`).

## Source-of-truth links

- [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) — scheduler design, lock ordering, pseudocode (§Scheduler pseudocode).
- [`.omne/cfg/architecture.md`](../../../.omne/cfg/architecture.md) §concurrency — module layout `crates/grex-core/src/concurrency/{mod,scheduler,packlock}.rs` (this change places the new module at `scheduler.rs` top-level per current repo convention; align if a `concurrency/` submodule is preferred).
- [`.omne/cfg/test-plan.md`](../../../.omne/cfg/test-plan.md) §Integration — `sync_parallel.rs` 8-pack baseline test already slotted; this change upgrades it to 100 packs.
- [`milestone.md`](../../../milestone.md) §M6.
- [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §Success criteria #5 ("`meta` pack with nested children syncs the tree recursively in parallel under the `--parallel N` bound").
