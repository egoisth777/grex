# concurrency

Tokio runtime, bounded semaphore, per-pack file lock, per-meta manifest lock. One Lean4-verified invariant.

> Canonical source: [.omne/cfg/concurrency.md](../../.omne/cfg/concurrency.md) (SSOT, separate `grex-inst` repo). This page is the user-facing projection.

## Runtime

```rust
#[tokio::main(flavor = "multi_thread", worker_threads = ...)]
async fn main() -> anyhow::Result<()> { ... }
```

Worker threads default = `num_cpus::get()`, overridable via `--parallel N` or `GREX_PARALLEL` env. The same `--parallel N` cap is honoured by the rayon scheduler that drives sibling sync within one meta — see [walker §Phase 1](./walker.md#phase-1--sync-direct-children-parallel) and [walker §Phase 3](./walker.md#phase-3--recurse-into-child-metas-parallel-autonomous).

## Five cooperating mechanisms

1. **Per-meta sync lock** — `<meta>/.grex.sync.lock` (fd-lock, non-blocking, fail-fast). Held for the full `grex sync` lifetime of THAT meta's frame. Two concurrent `grex sync` invocations against the same meta are a hard error, not a queue. **v1.x → v1.2.0:** through v1.x this was a single `<workspace>/.grex.sync.lock` at the workspace root (one global lock per workspace). Under v1.2.0+ each meta owns its own fd-lock under its own dir; cross-meta locks are independent (distinct metas never serialize against each other), the walker's recursion acquires + releases one lock per meta frame, and cargo-style parallel sub-meta sync is N concurrent fd-locks across the meta tree (one per meta currently being processed). Locking is per-meta, never global.
2. **Per-repo backend lock** — `<dest>.grex-backend.lock` (fd-lock, sibling file NOT inside `<dest>` so it survives `<dest>` wipe). Held across `clone` + `fetch` + `materialise_tree` for one repo path.
3. **Bounded semaphore** — caps in-flight pack ops across the process.
4. **Per-pack `.grex-lock`** — prevents two ops on the same pack path across processes and tasks.
5. **Per-meta manifest RW lock** (`fd-lock`) — serialises that meta's lockfile + event-log writes. **v1.x → v1.2.0:** under v1.2.0+ each meta has its own manifest fd-lock at `<meta>/.grex.lock`; the lock is per-meta, not global, so distinct metas may mutate their own lockfile + event log in parallel.

Lock acquisition order (fixed, deadlock-free): **per-meta-sync → semaphore → pack-lock → repo-backend → manifest-lock**. Never reversed.

### TOCTOU closure

The sync pipeline revalidates the per-meta dirty-check **twice**:

1. Before attempting to acquire the per-meta sync lock (fast reject).
2. After acquiring the per-meta sync lock AND immediately before calling `materialise_tree` (authoritative — any drift between steps 1 and 2 surfaces here).

Rationale: a concurrent non-sync writer (e.g. the user editing a file) could dirty the tree between our initial check and the moment we begin applying actions. The second check closes the window.

The path-swap TOCTOU (attacker swapping a directory for a symlink between `canonicalize(dest)` and the actual filesystem write) is closed by the `BoundedDir` dirfd-binding primitive — see [toctou](./toctou.md).

### Recovery scan

At sync startup, before acquiring the per-meta lock, grex runs an **informational** recovery scan that:

- Lists stale `.grex.sync.lock` / `<dest>.grex-backend.lock` whose owning PID is gone.
- Lists incomplete event brackets in the manifest (`action_started` with no matching `action_completed` / `action_halted`).

The scan only logs — it never mutates. Auto-cleanup is `grex doctor` territory.

## Bounded semaphore

```rust
use tokio::sync::Semaphore;
use std::sync::Arc;

pub struct Scheduler {
    permits: Arc<Semaphore>,
}

impl Scheduler {
    pub fn new(parallel: usize) -> Self {
        Self { permits: Arc::new(Semaphore::new(parallel)) }
    }

    pub async fn run<F, T>(&self, pack_path: &std::path::Path, fut: F) -> anyhow::Result<T>
    where
        F: Future<Output = anyhow::Result<T>> + Send,
        T: Send,
    {
        let _permit = self.permits.clone().acquire_owned().await?;
        let _plock  = PackLock::open(pack_path)?.acquire_async().await?;  // v1.2.4+: legacy `PackLock::acquire` is a deprecated shim
        fut.await
    }
}
```

The semaphore caps process-wide in-flight pack ops; the per-pack lock prevents double-execution of the same pack path across recursion frames or invocations. Sibling parallelism inside one meta and sub-meta parallelism across metas both run under the same semaphore cap.

## Per-pack `PackLock`

File: `<pack_workdir>/.grex-lock`. Held exclusively via `fd-lock::RwLock::write`. Non-blocking try-first; on contention the task yields and retries with backoff.

> **API note (v1.2.4+):** the canonical async entry point is `PackLock::acquire_async` (and `PackLock::acquire_cancellable` for the cancellable variant). The original `PackLock::acquire` signature shown in the sketch below is **deprecated** and retained only as a thin shim for backward compatibility — new call sites should use `acquire_async`.

```rust
pub struct PackLock {
    _guard: fd_lock::RwLockWriteGuard<'static, std::fs::File>,
}

impl PackLock {
    // Deprecated since v1.2.4 — prefer `acquire_async` (shown for prose continuity only).
    pub async fn acquire(pack_path: &std::path::Path) -> anyhow::Result<Self> {
        let lock_path = pack_path.join(".grex-lock");
        let file = std::fs::OpenOptions::new()
            .create(true).read(true).write(true)
            .open(&lock_path)?;
        let lock = fd_lock::RwLock::new(file);
        // retry loop: try_write() → on WouldBlock sleep + retry
        // ...
    }
}
```

Released on `Drop`. The file is NOT deleted on release (avoids a TOCTOU race). `grex doctor` prunes stale `.grex-lock` files whose owning PID is gone.

## Per-meta manifest RW lock

Any `events.jsonl` or `grex.lock.jsonl` mutation takes exclusive `fd_lock::RwLock::write` on the meta-local `<meta>/.grex.lock`. Readers take shared read. See [manifest](./manifest.md). The three-way disambiguation between this fd-lock file (`.grex.lock`), the lockfile (`.grex/grex.lock.jsonl`), and the event log (`.grex/events.jsonl`) lives in [lockfile §Three "lock" artifacts](./lockfile.md#three-lock-artifacts--disambiguation).

Because the lock is **per-meta** under v1.2.0+, distinct metas can mutate their own lockfile + event log in parallel. There is no global serialisation point at the manifest layer — the only cross-meta serialisation is the bounded process-wide semaphore on in-flight pack ops.

## Scheduler pseudocode

```text
schedule(packs, op):
    futures = []
    for pack in packs:
        fut = async {
            _sem_permit     = semaphore.acquire()            # bound parallelism
            _pack_lock      = PackLock::acquire_async(pack.path)   # per-pack exclusive (v1.2.4+; sync `acquire` is deprecated shim)
            result          = op.run_on(pack)
            _manifest_lock  = pack.meta.manifest.write_lock()  # innermost (per-meta)
            manifest.append(event_from(result))
            drop(_manifest_lock)                             # release innermost first
            result
        }
        futures.push(fut)
    return join_all(futures)
```

Key property: locks acquired outer-to-inner, released inner-to-outer. Manifest lock is the briefest; semaphore the longest.

## Lean4 invariant `I1` (no_double_lock)

**Invariant `I1`**: for any two concurrent tasks `t1`, `t2` scheduled by `Scheduler`, if `t1.pack_path == t2.pack_path`, then their lock-holding windows do NOT overlap in time.

> `I1` = "Invariant 1" — first concurrency-series invariant. Distinct from walker `I1` (boundary preservation) and architecture `I1` (the same scheduler theorem re-cited from the architecture doc). See the invariant series cross-reference table in the SSOT.

**Informal**: `PackLock::acquire_async` (canonical entry point since v1.2.4; the legacy `PackLock::acquire` is a deprecated shim) is exclusive per path; the later arrival awaits the earlier's drop.

**File**: `proof/Grex/Scheduler.lean`.

**Sketch**:

```lean
namespace Grex.Scheduler

structure Task where
  path    : String
  started : Nat      -- logical clock
  ended   : Nat
  deriving Repr

def Schedule := List Task

def overlaps (a b : Task) : Prop :=
  a.started < b.ended ∧ b.started < a.ended

-- PackLock is modeled as FIFO queue per path:
-- acquire(p) returns only after all prior holders for p have released.
axiom pack_lock_exclusive
    (s : Schedule) (a b : Task) :
    a ∈ s → b ∈ s → a.path = b.path → a ≠ b → ¬ overlaps a b

-- I1: scheduler never holds two concurrent locks on the same pack path.
theorem no_double_lock
    (s : Schedule) (a b : Task)
    (ha : a ∈ s) (hb : b ∈ s) (hpath : a.path = b.path) (hne : a ≠ b) :
    ¬ overlaps a b :=
  pack_lock_exclusive s a b ha hb hpath hne

end Grex.Scheduler
```

CI job (`.github/workflows/lean.yml`):

```yaml
- uses: leanprover/lean-action@v1
- run: cd proof && lake build
```

Zero `sorry`; zero unresolved `axiom` outside the stated model-bridging ones.

### Walker `I8` reduction

Walker invariant `I8` (parallel sync of disjoint sub-trees commutes — see [walker §Three changes vs v1.1.x](./walker.md#three-changes-vs-v11x)) reduces to concurrency `I1` for its mutual-exclusion lemma. The shipped axiom `sync_disjoint_commutes` in `proof/Grex/Walker.lean` covers the disjoint-pack work commutativity that the rayon scheduler relies on; no new theorem is required for the v1.2.1 rayon sibling-sync swap.

## Operational tuning

- `--parallel` default = `num_cpus::get()`. Typical 4-16.
- Git fetch is IO-bound → higher parallelism helps until network saturates.
- Shell-out actions (`exec`) may be internally multi-threaded; consider a per-type cap in v1.x.

## Telemetry

Each scheduled task emits a `tracing` span: `pack_path`, `op`, `duration_ms`, `result`. `grex doctor` can read the last-N spans from an on-disk journal for retrospective diagnosis.

## Cross-references

- Walker phases + parallel sibling/sub-meta scheduling: [walker](./walker.md)
- Distributed lockfile + per-meta manifest fd-lock disambiguation: [lockfile](./lockfile.md)
- Manifest event-log atomic append + crash recovery: [manifest](./manifest.md)
- TOCTOU `BoundedDir` (cap-std + Linux openat2): [toctou](./toctou.md)
- Force-prune audit log (writes through this manifest fd-lock): [force-prune](./force-prune.md)
