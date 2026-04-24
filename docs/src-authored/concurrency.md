# concurrency

Tokio runtime, bounded semaphore, per-pack file lock, global manifest lock. One Lean4-verified invariant.

## Runtime

```rust
#[tokio::main(flavor = "multi_thread", worker_threads = ...)]
async fn main() -> anyhow::Result<()> { ... }
```

Worker threads default = `num_cpus::get()`, overridable via `--parallel N` or `GREX_PARALLEL` env.

## Five cooperating mechanisms

1. **Workspace sync lock** — `<workspace>/.grex.sync.lock` (fd-lock, non-blocking, fail-fast). Held for the full `grex sync` lifetime. Two concurrent `grex sync` invocations against the same workspace are a hard error, not a queue.
2. **Per-repo backend lock** — `<dest>.grex-backend.lock` (fd-lock, sibling file NOT inside `<dest>` so it survives `<dest>` wipe). Held across `clone` + `fetch` + `materialise_tree` for one repo path.
3. **Bounded semaphore** — caps in-flight pack ops across the process.
4. **Per-pack `.grex-lock`** — prevents two ops on the same pack path across processes and tasks.
5. **Global manifest RW lock** (`fd-lock`) — serializes manifest + lockfile mutations.

Lock acquisition order (fixed, deadlock-free): **workspace-sync → semaphore → pack-lock → repo-backend → manifest-lock**. Never reversed.

### TOCTOU closure

The sync pipeline revalidates the workspace dirty-check **twice**:

1. Before attempting to acquire the workspace sync lock (fast reject).
2. After acquiring the workspace sync lock AND immediately before calling `materialise_tree` (authoritative — any drift between steps 1 and 2 surfaces here).

Rationale: a concurrent non-sync writer (e.g. the user editing a file) could dirty the tree between our initial check and the moment we begin applying actions. The second check closes the window.

### Recovery scan

At sync startup, before acquiring the workspace lock, grex runs an **informational** recovery scan that:

- Lists stale `.grex.sync.lock` / `<dest>.grex-backend.lock` whose owning PID is gone.
- Lists incomplete event brackets in the manifest (`ActionStarted` with no matching `ActionCompleted` / `ActionHalted`).

The scan only logs — it never mutates. Auto-cleanup is deferred to `grex doctor` (M4+).

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
        let _plock  = PackLock::acquire(pack_path).await?;
        fut.await
    }
}
```

## Per-pack `PackLock`

File: `<pack_workdir>/.grex-lock`. Held exclusively via `fd-lock::RwLock::write`. Non-blocking try-first; on contention the task yields + retries with backoff.

```rust
pub struct PackLock {
    _guard: fd_lock::RwLockWriteGuard<'static, std::fs::File>,
}

impl PackLock {
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

Released on `Drop`. File NOT deleted on release (avoids TOCTOU race). `grex doctor` prunes stale `.grex-lock` files whose owning PID is gone.

## Global manifest RW lock

Any `grex.jsonl` or `grex.lock.jsonl` mutation takes exclusive `fd_lock::RwLock::write`. Readers take shared read. See [manifest.md](./manifest.md).

## Scheduler pseudocode

```text
schedule(packs, op):
    futures = []
    for pack in packs:
        fut = async {
            _sem_permit     = semaphore.acquire()            # bound parallelism
            _pack_lock      = PackLock::acquire(pack.path)   # per-pack exclusive
            result          = op.run_on(pack)
            _manifest_lock  = global_manifest.write_lock()   # innermost
            manifest.append(event_from(result))
            drop(_manifest_lock)                             # release innermost first
            result
        }
        futures.push(fut)
    return join_all(futures)
```

Key property: locks acquired outer-to-inner, released inner-to-outer. Manifest lock is the briefest; semaphore the longest.

## Lean4 invariant (v1 proof scope)

**Invariant I1**: For any two concurrent tasks `t1`, `t2` scheduled by `Scheduler`, if `t1.pack_path == t2.pack_path`, then their lock-holding windows do NOT overlap in time.

**Informal**: `PackLock::acquire` is exclusive per path; the later arrival awaits the earlier's drop.

**File**: `lean/Grex/Scheduler.lean`.

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

During M6, `pack_lock_exclusive` is promoted from `axiom` to `theorem` by modeling `PackLock::acquire` as a FIFO queue on `path` and proving mutual-exclusion from that model. Exact construction in the commit that lands M6.

CI job (`.github/workflows/lean.yml`):

```yaml
- uses: leanprover/lean-action@v1
- run: cd lean && lake build
```

Zero `sorry`; zero unresolved `axiom` outside the stated model-bridging ones.

## Deferred Lean4 proofs (v2)

- I2: manifest append serialization under fd-lock.
- I3: `.gitignore` managed-block idempotence.
- I4: compaction fold-equivalence.
- Commutativity of disjoint-path events.

## Operational tuning

- `--parallel` default = `num_cpus::get()`. Typical 4-16.
- Git fetch is IO-bound → higher parallelism helps until network saturates.
- Shell-out actions (`exec`) may be internally multi-threaded; consider a per-type cap in v1.x.

## Telemetry

Each scheduled task emits a `tracing` span: `pack_path`, `op`, `duration_ms`, `result`. `grex doctor` can read the last-N spans from an on-disk journal (v1.x feature) for retrospective diagnosis.
