# feat-m6-2 — Per-pack `.grex-lock` + lock-ordering enforcement

**Status**: draft
**Milestone**: M6 (see [`../../../milestone.md`](../../../milestone.md) §M6)
**Depends on**: feat-m6-1 (`Scheduler` + `--parallel N` + `ExecCtx.scheduler`); M5 pack-type plugin system.

## Motivation

feat-m6-1 lands the semaphore but leaves pack-level exclusion unsolved. Two parallel tasks on the same `<pack_path>` would race on `.gitignore`, on action side-effects, and on the backend clone/fetch. `.omne/cfg/concurrency.md` §2 specifies `<pack_workdir>/.grex-lock` via `fd-lock::RwLock::write` as the defence; this change lands it plus the enforced acquisition ordering so the full 5-tier chain cannot deadlock.

## Goal

1. Introduce `PackLock` type wrapping `fd-lock::RwLock` on `<pack_path>/.grex-lock`.
2. Integrate acquisition inside each `PackTypePlugin` method (`install` / `update` / `sync` / `teardown`) — the plugin owns the acquire, not the caller.
3. Enforce the fixed 5-tier lock order from `.omne/cfg/concurrency.md`:
   1. workspace-sync lock (already shipped — M3 review PR #16)
   2. semaphore slot (feat-m6-1)
   3. per-pack `.grex-lock` (**this change**)
   4. backend lock `<dest>.grex-backend.lock` (already shipped — M3 review PR #16)
   5. manifest RW lock (already shipped — M2)
4. Add `.grex-lock` to the default managed-gitignore block so `git status` stays clean.
5. Acceptance stress test: overlapping pack trees across parallel sync invocations must show zero simultaneous writes to the same pack and zero deadlocks.

## Design

### `PackLock` type

File: `crates/grex-core/src/concurrency/packlock.rs` (new module; see `.omne/cfg/architecture.md` layout).

```rust
pub struct PackLock {
    _guard: fd_lock::RwLockWriteGuard<'static, std::fs::File>,
    path: PathBuf,
}

impl PackLock {
    pub async fn acquire(pack_path: &Path) -> Result<Self, PackLockError>;
    pub fn path(&self) -> &Path;
}
```

Semantics (from `.omne/cfg/concurrency.md`):
- File at `<pack_path>/.grex-lock`; created on demand (`OpenOptions::create(true).read(true).write(true)`).
- Exclusive via `RwLock::write`.
- Non-blocking `try_write` first; on `WouldBlock` yield with exponential backoff (start 1 ms, cap 100 ms) + jitter.
- Released on `Drop`. File **not deleted** on release (TOCTOU avoidance; `grex doctor` M4+ prunes stale files by PID).
- Windows + Unix: `fd-lock` abstraction — same call shape both OSes.

### Error type

```rust
#[non_exhaustive]
pub enum PackLockError {
    Io { path: PathBuf, source: io::Error },
    Contention { path: PathBuf, waited_ms: u64 },  // backoff budget exhausted
}
```

Plumbed into `ExecError` via a new `ExecError::PackLockAcquire(PackLockError)` variant (`#[non_exhaustive]` already on the enum).

### Plugin-method integration

**Contract**: the per-pack lock is acquired **inside** each `PackTypePlugin` method, not by the caller. Rationale:
- Plugins own their own mutual exclusion policy.
- Tests that invoke a plugin directly (without the scheduler) still get correctness.
- Matches M5 decision that dispatch is plugin-owned.

Implementation skeleton (applied to `MetaPlugin`, `DeclarativePlugin`, `ScriptedPlugin`):

```rust
async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError> {
    let _plock = PackLock::acquire(ctx.pack_root).await.map_err(ExecError::PackLockAcquire)?;
    // existing M5 body
}
```

Same prologue in `update`, `sync`, `teardown`.

**`MetaPlugin` nuance**: `MetaPlugin` acquires a lock on the *meta* pack root. Recursion into children happens via the tree walker which re-enters the plugin for each child — each child acquires its own `.grex-lock` on its own path. Cycle detection (M5-2) still prevents re-entry on the same path; the lock is the belt to that suspender. Children must not recursively reacquire the parent meta's lock (the walker passes a new `ctx` per child with `ctx.pack_root` updated).

### Acquisition-order enforcement

Pseudocode (straight from `.omne/cfg/concurrency.md` §Scheduler pseudocode, adapted to v1):

```
sync_run(workspace):
    _ws = acquire_workspace_sync_lock(workspace)   # tier 1 — already shipped
    scheduler = Scheduler::new(opts.parallel)      # tier 2 permit source — feat-m6-1
    for pack in walk(workspace):
        _permit = scheduler.acquire().await        # tier 2 — held across pack op
        dispatch(pack_type_plugin, pack, ctx):
            _plock = PackLock::acquire(pack.path)   # tier 3 — this change
            op = run_on(pack)                       # may acquire:
              # tier 4: backend_lock (inside fetcher)
              # tier 5: manifest_lock (inside manifest::append)
            drop(_plock)
        drop(_permit)
    drop(_ws)
```

Property: **outer-to-inner acquire, inner-to-outer release**. The manifest lock is always the briefest; workspace-sync the longest. No loop in the partial-order graph ⇒ no deadlock. feat-m6-3 mechanizes this proof in Lean4.

### Gitignore managed block

M5-2 shipped per-pack gitignore managed blocks. This change extends the **default block body** emitted by every pack's `GitignoreWriter` to include:

```
.grex-lock
```

Implementation: `crates/grex-core/src/gitignore/` — add `.grex-lock` to the default static pattern list that every managed block starts with, ahead of any pack-declared entries. Users who delete the line from inside the block must expect a re-add on the next sync (managed block semantics — users are warned to edit outside markers per M5 contract).

## File / module targets

| Concrete path | Change |
|---|---|
| `crates/grex-core/src/concurrency/mod.rs` | New parent module (if not present from feat-m6-1). |
| `crates/grex-core/src/concurrency/packlock.rs` | New — `PackLock` + `PackLockError`. |
| `crates/grex-core/src/scheduler.rs` | May move under `concurrency/scheduler.rs` for consistency with `architecture.md`. |
| `crates/grex-core/src/execute/error.rs` | Add `ExecError::PackLockAcquire` variant. |
| `crates/grex-core/src/plugin/pack_types/meta.rs` | Acquire `PackLock` in each of 4 methods. |
| `crates/grex-core/src/plugin/pack_types/declarative.rs` | Same. |
| `crates/grex-core/src/plugin/pack_types/scripted.rs` | Same. |
| `crates/grex-core/src/gitignore/mod.rs` | Default block body includes `.grex-lock`. |
| `crates/grex-core/Cargo.toml` | `fd-lock` already present (used in M2 manifest); confirm feature-gates. |
| `.omne/cfg/concurrency.md` | No change — this change implements the existing spec. |

## Test plan

### Unit

`packlock.rs` `#[cfg(test)]`:
- `packlock_acquire_creates_file`
- `packlock_release_keeps_file_on_disk` — TOCTOU invariant.
- `packlock_second_acquire_blocks_then_succeeds` — spawn task B that waits for task A's drop.
- `packlock_backoff_bounded` — mock contention for > cap period, assert `Contention` error.
- `packlock_different_paths_do_not_contend` — A on `/p1`, B on `/p2`, both acquire concurrently.

### Integration

`crates/grex/tests/sync_pack_lock.rs` (new):
- `two_parallel_syncs_on_same_pack_serialize` — two `tokio::spawn(sync)` against the same fixture meta pack; assert `ActionStarted` events for the second sync come strictly after `ActionCompleted` of the first on every shared pack path.
- `parallel_syncs_on_disjoint_packs_run_concurrently` — control; assert pack A and pack B overlap in time (negative regression for feat-m6-1).
- `pack_lock_file_auto_added_to_gitignore_managed_block` — sync a fixture pack; assert the managed block contains `.grex-lock`.

### Stress

`crates/grex/tests/sync_stress.rs` (extend from feat-m6-1):
- `stress_overlapping_trees_no_deadlock` — 5 pack trees, each 20 packs deep, with 3 shared children across trees. Run `--parallel 8` × 10 iterations. Assert every run terminates within 60 s wall-clock; assert no two `ActionStarted` on the same pack path overlap any `ActionCompleted` on the same path; assert zero `PackLockError::Contention` at the default backoff budget.
- `stress_100_concurrent_pack_ops_overlapping_roots` — 100 `grex sync` invocations in-process (separate workspaces but overlapping fixture pack URLs → shared *repo-backend* targets). Validates tier 3 (pack lock) and tier 4 (backend lock) interact correctly; no deadlock.

### Process-level

`crates/grex/tests/sync_cross_process.rs` (new, `#[ignore]`):
- `two_grex_sync_processes_on_same_pack_one_fails_workspace_lock` — one sync gets the workspace lock, second fails fast with `WorkspaceLocked` (existing M3-review behaviour; regression cover).
- `two_grex_sync_processes_on_different_workspaces_sharing_pack_serialize_on_packlock` — each has its own `.grex.sync.lock`, but the shared pack path's `.grex-lock` enforces mutex; assert completion-order invariant.

### Property (if time)

`proptest` on a model `Vec<(pack_path, timestamp_start, timestamp_end)>` — random schedules, assert no two records with equal `pack_path` overlap. Mirrors the Lean4 `no_double_lock` theorem statement; acts as executable oracle for feat-m6-3.

## Non-goals

- **No `grex doctor` auto-prune** of stale `.grex-lock` (PID-based). Carry-forward per `.omne/cfg/concurrency.md` §Per-pack `PackLock`.
- **No lock contention telemetry beyond `tracing` spans**. Per-pack wait-time histogram deferred to v1.x.
- **No retry policy exposed to users**. Backoff constants (1 ms start, 100 ms cap, 5 s budget) are internal.
- **No cross-process fairness guarantees**. `fd-lock` is FIFO-ish on Linux, not guaranteed on Windows. Proof (feat-m6-3) models FIFO-per-path; deviation documented.
- **No lock on `.grex/` (the contract dir)** — lock is on `<pack_path>/.grex-lock`, not inside `.grex/`.

## Dependencies

- **Prior**: feat-m6-1 (scheduler + flag + `ExecCtx.scheduler`); M2 `fd-lock` usage; M3-review PR #16 workspace + backend locks; M5-2 gitignore managed-block writer.
- **Next**: feat-m6-3 proves the ordering rule is deadlock-free.

## Acceptance

1. `sync_pack_lock.rs` tests green — same-pack serialisation + disjoint-pack parallelism.
2. `stress_overlapping_trees_no_deadlock` passes under `cargo test -- --ignored` on Linux + Windows CI.
3. `pack_lock_file_auto_added_to_gitignore_managed_block` passes — `.grex-lock` appears exactly once per managed block.
4. No regressions on M5 workspace baseline (470 / 382 with feature).
5. `cargo clippy --all-targets --workspace -- -D warnings` clean.
6. Manual check: run `grex sync` twice in two terminals against the same workspace; one errors fast (workspace lock), does not hang.

## Source-of-truth links

- [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) §Per-pack `PackLock` + §Lock acquisition order — primary spec.
- [`.omne/cfg/architecture.md`](../../../.omne/cfg/architecture.md) §Workspace — module layout places `packlock.rs` under `src/concurrency/`.
- [`.omne/cfg/test-plan.md`](../../../.omne/cfg/test-plan.md) §Concurrency tests — baseline.
- [`milestone.md`](../../../milestone.md) §M6 — "Per-pack `<path>/.grex-lock` file (fd-lock) prevents same-pack double-exec."
- [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §M5 R-M5-08 (gitignore managed block) — extended here.
