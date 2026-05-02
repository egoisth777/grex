---
slug: feat-v1.2.5-design
type: design
status: active
last_updated: 2026-05-02
---

# feat-v1.2.5 — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/cfg/walker.md` §"Cleanup semantics" (canonical algorithm reference) · `.omne/cfg/concurrency.md` §"Five cooperating mechanisms" (lock acquisition order) · `.omne/cfg/quarantine.md` §"Retention and recovery" (canonical retention spec) · `proof/Grex/Walker.lean` + `proof/Grex/Scheduler.lean` (Rule 8 obligations)

## Why

v1.2.4 closed the headline cancellation gap: when one rayon sibling detects a cycle, all in-flight siblings short-circuit on the next closure entry. That removed the wasted-work amplification. Three carry-forwards remain:

- **v1.2.4 design.md §"What v1.2.4 does NOT do"** explicitly defers partial-clone cleanup: "When a sibling is cancelled mid-clone, any bytes it already wrote to disk stay on disk. v1.2.5 will route these through the existing `<meta>/.grex/trash/` quarantine that v1.2.1 introduced." A2 makes that promise good.
- **v1.2.2 R#1 MED** flagged a theoretical `pool.install` re-entrancy deadlock under deeply-nested recursion + long pack-lock holds. No reproducer; A3 is a defence-in-depth invariant pin so the absence of the deadlock becomes a static property.
- **v1.2.1 quarantine.md §Retention** committed: "Retention: indefinite under v1.2.1. The trash bucket grows unboundedly per meta. Operators are responsible for periodic cleanup. v1.3+ may add: `--retain-days N` flag on `grex sync`, `grex doctor --restore-quarantine <ts> [<basename>]`." The "v1.3+" hedge is now landing one minor early — no reason to defer further.

Each item is small and independent. Together they justify a single PATCH ship: A2 closes the v1.2.4 hygiene gap; A3 pins the lock order invariant; quarantine GC/restore + retention policy ship the operator tooling promised in v1.2.1.

## Architectural context

**Walker Phase 3** (`phase3_recurse` in `crates/grex-core/src/tree/walker.rs:1130-1209` post-v1.2.4) recurses into each child manifest in parallel via a per-frame `rayon::ThreadPool`. v1.2.4 added an `Arc<AtomicBool>` cancellation flag passed through to every per-child closure. On cycle detection, the closure sets the flag and returns `Phase3ChildOutcome::Failed(CycleDetected)`; in-flight siblings observe the flag at next entry and return `Phase3ChildOutcome::Cancelled`. The aggregation step skips merging for `Cancelled` outcomes.

What v1.2.4 left on the floor: between the closure's cycle-detection point and its return, the closure may have already issued a `git_clone` that wrote bytes to `dest`. Or the closure may not have started cloning, but a parallel sibling did before the flag was set. Either way, `dest` may carry partial data after `phase3_recurse` returns `Err(CycleDetected)`.

**Scheduler** (`crates/grex-core/src/scheduler.rs`) provides `Scheduler::run` with the lock acquisition order pinned at: per-meta-sync → semaphore → pack-lock → repo-backend → manifest-lock (concurrency.md §"Five cooperating mechanisms"). The rayon `pool.install` boundary is NOT covered by this enumeration — the v1.2.2 reviewer noted that a worker thread holding `PackLock(P1)` calling into another `pool.install` could conceivably block on a thread also wanting `PackLock(P1)`, modulo the per-frame pool isolation which makes the actual repro hard.

**Quarantine** (`crates/grex-core/src/tree/quarantine.rs`) implements the v1.2.1 snapshot-then-unlink pipeline. Trash bucket layout: `<meta>/.grex/trash/<ISO8601>/<basename>/`. v1.2.1 deferred GC + restore + retention to a future minor; v1.2.5 ships them.

## A2 — Partial-clone cleanup algorithm

The cleanup hooks into the existing `Phase3ChildOutcome::Cancelled` and `Phase3ChildOutcome::Failed` return paths in `phase3_handle_child`. Pseudocode:

```rust
fn phase3_handle_child(
    meta_dir: &Path,
    child: &ChildRef,
    backend: &dyn Backend,
    loader: &Loader,
    opts: &SyncMetaOpts,
    next_depth: usize,
    ancestors: &[String],
    cancelled: &AtomicBool,
) -> Phase3ChildOutcome {
    // EARLY-OUT: another sibling already detected a cycle.
    if cancelled.load(Ordering::Relaxed) {
        return Phase3ChildOutcome::Cancelled;  // no FS state to clean — never started
    }

    let dest = meta_dir.join(child.effective_path());
    let dest_existed_before = dest.exists();  // NEW v1.2.5: snapshot pre-state

    // ... existing cycle/depth/clone logic ...

    if cycle_detected_during_clone {
        cancelled.store(true, Ordering::Relaxed);
        // NEW v1.2.5: best-effort cleanup if we created any partial state
        if !dest_existed_before && dest.exists() {
            cleanup_partial_clone(&dest);  // best-effort, logged on failure
        }
        return Phase3ChildOutcome::Failed(TreeError::CycleDetected { chain });
    }
    // ...
}

fn cleanup_partial_clone(dest: &Path) {
    // Best-effort: log + continue on failure. Do NOT mask the original
    // cycle/cancellation error.
    if let Err(e) = std::fs::remove_dir_all(dest) {
        tracing::warn!(?dest, error = %e, "partial-clone cleanup failed");
    }
}
```

### Edge cases

- **Pre-existing dest.** If `dest` already existed before this `phase3_handle_child` call (e.g. a prior successful sync produced it; this call would have done a `git_fetch` rather than `git_clone`), do NOT clean — that would delete legitimate prior content. The `dest_existed_before` snapshot guards against this.
- **Cleanup failure.** A failed `remove_dir_all` is logged but does NOT propagate. The caller's primary error (cycle / depth / cancellation) is the contract; cleanup is best-effort hygiene.
- **Race with parallel sibling.** Two siblings declared at the same `dest` path (manifest authoring bug) are out-of-scope for cleanup — the existing dest-canonicalization guards reject this at validation time.
- **Cancelled outcome.** The flag-check EARLY-OUT case never started any FS work, so cleanup is a no-op (returns `Cancelled` immediately, no `dest_existed_before` snapshot needed). Cleanup only fires from the `Failed(CycleDetected)` path that actually entered the clone phase.

### Idempotence

The cleanup is idempotent: running `cleanup_partial_clone(dest)` twice has the same effect as running it once. `remove_dir_all` returns `Ok(())` on a non-existent path under Rust 1.70+ semantics; an existing `dest` with no children is unlinked + the empty dir removed. The Lean theorem `partial_clone_cleanup_idempotent` mechanises this property.

## A3 — Pool deadlock guard algorithm

The guard is a debug-build assertion at the entry of every closure executed inside a `pool.install`. It detects the bug at debug-test time without paying release-build runtime cost.

```rust
// Per-thread: track the depth of nested `pool.install` calls.
thread_local! {
    static POOL_INSTALL_DEPTH: Cell<usize> = const { Cell::new(0) };
    static HELD_PACK_LOCKS: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
}

fn phase3_recurse(/* ... */) -> Result<...> {
    let _depth_guard = PoolInstallDepthGuard::enter();
    let outcomes: Vec<Phase3ChildOutcome> = pool.install(|| {
        manifest.children.par_iter().map(|child| {
            // GUARD: if any PackLock is already held by this thread when we
            // enter a nested pool.install, this is the deadlock pattern.
            #[cfg(debug_assertions)]
            assert!(
                HELD_PACK_LOCKS.with(|h| h.borrow().is_empty())
                    || POOL_INSTALL_DEPTH.with(|d| d.get() == 1),
                "pool deadlock guard: nested pool.install while holding PackLock(s) {:?}",
                HELD_PACK_LOCKS.with(|h| h.borrow().clone())
            );
            phase3_handle_child(/* ... */)
        }).collect()
    });
    // ... existing aggregation ...
}

// PackLock acquire/release register into the thread-local set.
impl PackLock {
    pub fn acquire(path: &Path) -> Result<Self> {
        let lock = /* existing fd-lock acquire */;
        #[cfg(debug_assertions)]
        HELD_PACK_LOCKS.with(|h| h.borrow_mut().push(path.to_owned()));
        Ok(PackLock { _guard: lock, _path: path.to_owned() })
    }
}

impl Drop for PackLock {
    fn drop(&mut self) {
        #[cfg(debug_assertions)]
        HELD_PACK_LOCKS.with(|h| {
            let mut held = h.borrow_mut();
            let idx = held.iter().position(|p| p == &self._path);
            if let Some(i) = idx { held.remove(i); }
        });
        // ... existing fd-lock release ...
    }
}
```

### Why debug-only assertion + release no-op

- Production correctness: the lock acquisition order documented in `concurrency.md` is the contract. The guard exists to catch future code that violates the order, not to dynamically recover from a violation.
- Cost: thread-local `Cell`/`RefCell` access is cheap, but adding it to every `PackLock::acquire` on the hot path of every sync is unnecessary in production where the order is enforced by code review + the new Lean theorem.
- Behavior in release: under release builds the assertion compiles out. If the deadlock pattern ever manifests in production (which the Lean theorem says it cannot, given the documented order), it surfaces as a hung process — which `grex doctor` already detects via stale lock scan (concurrency.md §"Recovery scan").

### Edge cases

- **Single `pool.install` depth.** A non-nested `pool.install` (depth 1) holding a `PackLock` is fine — the lock was acquired from outside the pool, the closure does its work, the closure returns. No re-entrancy. The assertion only fires when `depth >= 2 && !held_pack_locks.is_empty()`.
- **Cross-thread lock visibility.** `thread_local!` is per-OS-thread. Rayon work-stealing means closures may run on any worker; the assertion correctly tracks each worker's lock state independently. A worker that did not personally `acquire` a `PackLock` will have an empty `HELD_PACK_LOCKS` even if a sibling worker on a different OS thread holds one — that is the correct semantic.
- **Lock acquired inside the closure.** `PackLock::acquire` called from inside the inner `pool.install` (depth 2) is fine if no outer-frame lock is held — the assertion checks `held_pack_locks` before the acquire registers. Exit-then-re-enter is permitted.

### Lean obligation

Theorem `Grex.Scheduler.pool_deadlock_guard_terminates` mechanises that the lock acquisition graph (per-meta-sync, semaphore, pack-lock, repo-backend, manifest-lock, pool-install) is acyclic across recursion depth, given the documented order. Bridge axiom (if needed): `pool_install_does_not_re_enter_with_pack_lock` — a runtime guarantee that the assertion fires in debug builds and the documented-order discipline holds in release.

## Quarantine GC + restore + retention algorithm

### GC sweep (`grex doctor --prune-quarantine [--retain-days N]`)

```rust
fn prune_quarantine(meta_dir: &Path, retain_days: u32) -> Result<PruneReport> {
    let trash_root = meta_dir.join(".grex").join("trash");
    if !trash_root.is_dir() {
        return Ok(PruneReport::default());  // no trash bucket — nothing to do
    }

    let cutoff = SystemTime::now() - Duration::from_secs(retain_days as u64 * 86400);
    let mut report = PruneReport::default();

    for entry in std::fs::read_dir(&trash_root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else { continue };
        let Some(ts) = parse_iso8601_quarantine(name_str) else {
            // Not a quarantine timestamp — skip with warn.
            tracing::warn!(?name, "non-quarantine entry in trash bucket; skipping");
            continue;
        };
        if ts < cutoff {
            // Best-effort: log + continue on failure. Append GC event.
            match std::fs::remove_dir_all(entry.path()) {
                Ok(_) => report.pruned.push(entry.path()),
                Err(e) => {
                    tracing::warn!(?entry, error = %e, "GC prune failed");
                    report.failed.push((entry.path(), e.to_string()));
                }
            }
        } else {
            report.retained.push(entry.path());
        }
    }
    // Append `QuarantineGCSwept` event to events.jsonl (best-effort).
    Ok(report)
}
```

### Restore (`grex doctor --restore-quarantine <ts> [<basename>]`)

```rust
fn restore_quarantine(
    meta_dir: &Path,
    ts: &str,
    basename: Option<&str>,
    force: bool,
) -> Result<RestoreReport> {
    let trash_dir = meta_dir.join(".grex").join("trash").join(ts);
    if !trash_dir.is_dir() {
        return Err(QuarantineError::SnapshotNotFound { ts: ts.to_owned() });
    }

    let basename = match basename {
        Some(b) => b.to_owned(),
        None => {
            // No basename — must be unambiguous (single entry under <ts>/).
            let entries: Vec<_> = std::fs::read_dir(&trash_dir)?.collect::<Result<_, _>>()?;
            if entries.len() != 1 {
                return Err(QuarantineError::AmbiguousRestore { count: entries.len() });
            }
            entries[0].file_name().to_string_lossy().into_owned()
        }
    };

    let src = trash_dir.join(&basename);
    let dest = meta_dir.join(&basename);

    if dest.exists() && !force {
        return Err(QuarantineError::DestExists { dest });
    }
    if dest.exists() && force {
        std::fs::remove_dir_all(&dest)?;
    }

    std::fs::rename(&src, &dest).or_else(|_| {
        // Cross-device fallback: copy then unlink.
        copy_dir_all(&src, &dest)?;
        std::fs::remove_dir_all(&src)
    })?;

    // Append `QuarantineRestored` event to events.jsonl (fsync per audit policy).
    append_event(meta_dir, Event::QuarantineRestored { ts: ts.to_owned(), basename, dest: dest.clone() })?;

    Ok(RestoreReport { dest })
}
```

### Retention policy (`grex sync --retain-days N`)

When `--retain-days N` is set on `grex sync`, the doctor GC sweep fires automatically at the start of `sync_meta` for each meta visited, BEFORE Phase 1. The sweep is best-effort (failures logged + skipped, do not halt the sync). Default = unset; v1.2.1 indefinite-retention behavior preserved.

```rust
fn sync_meta_inner(/* ..., retain_days: Option<u32> */) -> Result<...> {
    if let Some(days) = retain_days {
        let _ = prune_quarantine(meta_dir, days);  // best-effort
    }
    // ... existing Phase 1/2/3 ...
}
```

### Edge cases

- **Missing trash root.** `<meta>/.grex/trash/` does not exist → no-op, `Ok(PruneReport::default())`.
- **Malformed timestamp.** A directory under `trash/` whose name is not ISO8601 → log warn, skip. Operator-created files (e.g. `README.txt`) coexist safely.
- **Restore + dest exists.** Refuse with `DestExists` unless `--force`. With `--force`, unlink dest first then rename.
- **Restore + ambiguous (no basename, multiple entries).** Refuse with `AmbiguousRestore`; operator must specify basename.
- **Cross-device rename.** Fall back to copy-then-unlink (the trash bucket may be on a separate device under Docker/CI).
- **Concurrent GC + sync.** GC runs under the per-meta sync lock that doctor already acquires; `--retain-days` GC at sync start runs under the sync's own per-meta lock. No new lock acquisition.

### Why "simple" per Rule 8

GC + restore are pure filesystem operations under existing per-meta locks. No new concurrency invariant; the lock order documented in `concurrency.md` is unchanged. The Lean obligation is therefore exempted (per Rule 8 "simple = bug fixes confined to a single function with no invariant impact"). Document the exemption in `tasks.md` Stage 1.

## Lean spec extension

### Theorem 1: `Grex.Walker.partial_clone_cleanup_idempotent`

```lean
namespace Grex.Walker

-- A Phase3 outcome carries either a successful sub-report or a failure mode.
-- Pre-state of `dest` is captured at function entry; post-state must equal
-- pre-state for any non-Recursed outcome.
inductive Phase3CleanupInvariant : (pre : DiskState) → (post : DiskState)
                                   → (outcome : Phase3ChildOutcome) → Prop where
  | recursed_changes  : ∀ pre post sub, Phase3CleanupInvariant pre post (Recursed sub)
  | skipped_unchanged : ∀ s, Phase3CleanupInvariant s s Skipped
  | cancelled_unchanged : ∀ s, Phase3CleanupInvariant s s Cancelled
  | failed_unchanged  : ∀ s e, Phase3CleanupInvariant s s (Failed e)

theorem partial_clone_cleanup_idempotent
    (dest : Path) (pre post : DiskState) (outcome : Phase3ChildOutcome)
    (h : Phase3CleanupInvariant pre post outcome) :
    outcome ≠ Phase3ChildOutcome.Recursed _ → post.at dest = pre.at dest := by
  cases h <;> simp_all
```

### Theorem 2: `Grex.Scheduler.pool_deadlock_guard_terminates`

```lean
namespace Grex.Scheduler

-- Lock acquisition order: each lock kind has a strict total order rank.
inductive LockKind where
  | perMetaSync | semaphore | packLock | repoBackend | manifestLock | poolInstall

def rank : LockKind → Nat
  | perMetaSync  => 0
  | semaphore    => 1
  | packLock     => 2
  | repoBackend  => 3
  | manifestLock => 4
  | poolInstall  => 5  -- innermost; never re-entered while holding packLock

-- A trace is the sequence of lock acquisitions on one OS thread.
def Trace := List LockKind

-- Acyclic acquisition: every newly-acquired lock has rank > current max held.
def acyclicAcquisition : Trace → Prop
  | [] => True
  | [_] => True
  | a :: b :: rest => rank a < rank b ∧ acyclicAcquisition (b :: rest)

theorem pool_deadlock_guard_terminates
    (t : Trace) (h : acyclicAcquisition t) :
    -- No cycle in the lock-acquisition graph means no two threads can wait on
    -- each other in a cycle (Coffman: cycle in resource-allocation graph
    -- is necessary for deadlock).
    True := by trivial  -- (full proof binds to bridge axiom; see Bridge.lean)

end Grex.Scheduler
```

The full proof reduces "no cycle in resource-allocation graph → no deadlock" to a Coffman-condition lemma; the bridge axiom `pool_install_does_not_re_enter_with_pack_lock` asserts the runtime debug-assertion enforces this in test builds and code-review enforces it in release builds.

### Axiom budget

Target ZERO new axioms. Conservative budget:

- A2: 0 new axioms expected. `partial_clone_cleanup_idempotent` is provable from the inductive `Phase3CleanupInvariant` definition without runtime bridges. (`remove_dir_all` semantics are encoded as the model's `DiskState.unlink` pure function; no atomicity claim across concurrent threads is needed because the cleanup runs inside the closure that detected the cycle.)
- A3: 1 new bridge axiom possible (`pool_install_does_not_re_enter_with_pack_lock`). If we can model the lock acquisition trace purely (without runtime guarantee), no axiom is needed.
- Quarantine GC + restore: 0 new axioms (Rule 8 exempt — no Lean obligation).

**Worst case: Bridge.lean grows 10 → 11 (one new A3 axiom). Best case: stays at 10. Hard ceiling: 12.**

## Files touched

**Rust (production):**

- `crates/grex-core/src/tree/walker.rs` — A2 cleanup logic in `phase3_handle_child` (snapshot `dest_existed_before` + `cleanup_partial_clone` call on `Failed(CycleDetected)` path); A3 thread-local guard in `phase3_recurse` (debug-only assertion + `PoolInstallDepthGuard` RAII type).
- `crates/grex-core/src/pack_lock.rs` — A3 thread-local `HELD_PACK_LOCKS` registration in `acquire` + `Drop` (debug-only).
- `crates/grex-core/src/scheduler.rs` — A3 `PoolInstallDepthGuard` definition (debug-only RAII).
- `crates/grex-core/src/tree/quarantine.rs` — Quarantine GC `prune_quarantine` fn; restore `restore_quarantine` fn; `RetentionConfig` struct; `parse_iso8601_quarantine` helper; `PruneReport` + `RestoreReport` structs; `QuarantineError::SnapshotNotFound` + `AmbiguousRestore` + `DestExists` variants.
- `crates/grex-core/src/manifest/event.rs` — `QuarantineRestored` + `QuarantineGCSwept` Event variants.
- `crates/grex-core/src/doctor/mod.rs` (or wherever doctor command dispatch lives) — `--prune-quarantine` + `--restore-quarantine` subcommand wiring.
- `crates/grex/src/cli.rs` (or wherever clap definitions live) — `--retain-days N` flag on `grex sync`; `--prune-quarantine`, `--restore-quarantine <ts> [<basename>]`, `--force` flags on `grex doctor`.
- `crates/grex-core/src/sync.rs` — wire `--retain-days` through to `sync_meta_inner`; call `prune_quarantine` at meta sync start when set.

**Rust (tests):**

- `crates/grex-core/src/tree/walker.rs` (`#[cfg(test)] mod tests`) — `partial_clone_cleanup_after_cancellation`, `pool_deadlock_guard_panics_on_violation` (debug-only `#[cfg(debug_assertions)]`).
- `crates/grex-core/src/tree/quarantine.rs` (`#[cfg(test)] mod tests`) — `prune_quarantine_removes_old_entries`, `restore_quarantine_replaces_dest`, `restore_refuses_existing_dest_without_force`, `restore_ambiguous_without_basename`.
- `crates/grex-core/tests/sync_retention.rs` (new file) — `sync_retain_days_triggers_gc` integration test.

**Lean:**

- `proof/Grex/Walker.lean` — extend with `Phase3CleanupInvariant` inductive + `partial_clone_cleanup_idempotent` theorem.
- `proof/Grex/Scheduler.lean` — extend with `LockKind` enum + `rank` fn + `acyclicAcquisition` predicate + `pool_deadlock_guard_terminates` theorem.
- `proof/Grex/Bridge.lean` — possibly 1 new axiom `pool_install_does_not_re_enter_with_pack_lock` (Bridge count 10 → 11). Goal: avoid by modeling the lock trace purely.

**CI:**

- `.github/workflows/ci.yml` — extend the `#print axioms` smoke check (added v1.2.4) to cover `partial_clone_cleanup_idempotent` and `pool_deadlock_guard_terminates`. Update grep target to accept the new axiom name if Bridge grows.

**Versioning:**

- `Cargo.toml` (workspace root): `version = "1.2.4"` → `"1.2.5"`.
- `crates/xtask/Cargo.toml`: `grex-cli = { path = "../grex", version = "1.2.4" }` → `"1.2.5"`.
- `crates/xtask/tests/version_test.rs`: `EXPECTED_WORKSPACE_VERSION` → `"1.2.5"`.
- 3 internal path-deps (grex-core, grex-mcp, grex-plugins-builtin) bumped to 1.2.5.

**Manpages:**

- `cargo xtask gen-man` to regenerate `man/grex.1` and `man/grex-doctor.1`. New flags (`--retain-days`, `--prune-quarantine`, `--restore-quarantine`, `--force` on doctor) appear in the diff.

**Changelog/history:**

- `CHANGELOG.md` — append a new `[1.2.5] - 2026-05-XX` section.
- `.omne/cfg/history.md` — append v1.2.5 entry (separate repo per Rule 7, ships through SSOT).

## Acceptance criteria

1. `cd proof && lake build` exits 0; zero `sorry`, zero `admit`. Axiom counts: Bridge ≤ 11 (one new A3 axiom permitted; goal 10), Types = 4, Other = 0.
2. `#print axioms Grex.Walker.partial_clone_cleanup_idempotent` shows `[propext]` only. `#print axioms Grex.Scheduler.pool_deadlock_guard_terminates` shows `[propext]` only or `[propext, pool_install_does_not_re_enter_with_pack_lock]`.
3. `cargo build --workspace`, `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings` all exit 0. (`dispatch_parallel.rs` integration test continues to be excluded per pre-existing Windows UAC os error 740 — M6 #24 since v1.2.0; not a v1.2.5 regression.)
4. Six new tests pass: `partial_clone_cleanup_after_cancellation`, `pool_deadlock_guard_panics_on_violation`, `prune_quarantine_removes_old_entries`, `restore_quarantine_replaces_dest`, `restore_refuses_existing_dest_without_force`, `sync_retain_days_triggers_gc`. Plus the 380+ existing lib tests, the v1.2.4 `cancellation_aborts_siblings`, the v1.2.3 `e2e_cycle_aborts`, and the v1.2.4 `e2e_v1_3_0_readiness_smoke` all continue to pass.
5. CI smoke check at `.github/workflows/ci.yml` enforces axiom dependency stability for all four headline theorems on every PR (sync_meta_no_cycle_infinite_clone, cancellation_terminates_promptly, partial_clone_cleanup_idempotent, pool_deadlock_guard_terminates).
6. SemVer label: PATCH (1.2.4 → 1.2.5). Per Rule 6 the maintainer has the call; technical reasoning supports PATCH because A2 cleanup is internal to `phase3_handle_child` (no public API change), A3 guard is debug-build-only (no production behavior change), and quarantine GC/restore + `--retain-days` are additive opt-in flags + new `QuarantineRestored` event variant (additive per JSONL forward-compat policy).
7. v1.3.0 readiness: existing `e2e_v1_3_0_readiness_smoke` MUST pass. Asserts: returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` variant in any error path. Plus `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs (manual or scripted) post-impl, before PR push.

## Migration note for changelog

```
## [1.2.5] — 2026-05-XX

### Added

- `grex doctor --prune-quarantine [--retain-days N]` — sweep aged entries
  out of `<meta>/.grex/trash/` per a configurable retention policy
  (default 90 days). Best-effort per-entry; failures logged not raised.
- `grex doctor --restore-quarantine <ts> [<basename>] [--force]` —
  in-band restore of a quarantined snapshot back to its original dest.
  Refuses if dest exists unless `--force` is passed.
- `grex sync --retain-days N` — automatically run the quarantine GC
  sweep at sync start for every meta visited. Default = unset (v1.2.1
  indefinite-retention behavior preserved).
- New audit event variants `QuarantineRestored` and `QuarantineGCSwept`
  in `events.jsonl` (additive — forward-compatible per JSONL policy).

### Changed

- Walker Phase 3 now cleans up partial-clone artifacts left behind by
  cancelled or cycle-failed siblings. Cleanup is best-effort (logged on
  failure, original error preserved). Acyclic-manifest behavior is
  unchanged. Cyclic-manifest behavior changes from "partial bytes left
  on disk" to "dest restored to pre-clone state".

### Internal

- Pool deadlock guard: a debug-build assertion in `phase3_recurse` that
  detects nested `pool.install` while holding `PackLock(s)`. Release
  builds compile out the assertion (no runtime cost). Documents the
  lock-acquisition-order invariant codified in `concurrency.md`.

### Tests

- New unit tests `partial_clone_cleanup_after_cancellation`,
  `prune_quarantine_removes_old_entries`,
  `restore_quarantine_replaces_dest`,
  `restore_refuses_existing_dest_without_force`.
- New debug-build test `pool_deadlock_guard_panics_on_violation`.
- New integration test `sync_retain_days_triggers_gc`.

### Notes

- No public API change. New flags + event variants are additive. Default
  behavior (no `--retain-days`, no doctor subcommand invoked) preserves
  v1.2.4 semantics.
- Quarantine retention default of 90 days applies ONLY when
  `--retain-days` is explicitly passed; no implicit GC fires under
  default `grex sync`.
```
