import Grex.Types

/-!
# `Grex.Bridge` — model-bridge axioms for grex's mechanised proofs

This module is the canonical home for **all** model-bridge axioms used by
grex's Lean proofs. A bridge axiom encodes a contract that Lean cannot
prove directly because it depends on Rust-runtime, kernel, or hardware
semantics (rayon scheduling, fd-lock FIFO, cap-std capability handles,
filesystem rename atomicity, …). Every axiom here corresponds to a
specific identifiable line in the Rust impl whose faithful preservation
is the engineer's responsibility.

This file currently consolidates the **six** bridge axioms previously
inline in `Grex.Walker` (4) and `Grex.Scheduler` (2):

  Walker bridges (depend on the walker model in `Grex.Types`):
  * `sync_disjoint_commutes`  — rayon scheduler runs disjoint subtrees in
                                parallel without interference
  * `sync_no_untracked`       — Rust walker's `untracked` accumulator
                                semantics (no declared `.git/` without
                                a matching pack)
  * `sync_local_writes`       — cap-std/openat2 capability-confined writes
                                (no escapes via symlink traversal)
  * `sync_idempotent`         — stable-input idempotency of the recursive
                                walker

  Scheduler bridges (depend on the scheduler model in `Grex.Types`):
  * `runtime_respects_ordering` — Rust scheduler obeys the fixed 5-tier
                                  lock-acquisition order
  * `pack_lock_exclusive`       — fd-lock FIFO mutex semantics for the
                                  per-pack lock

Future v1.2.0 axioms (`git_in_progress_decidable`,
`consent_walk_reflects_fs_state`) land here in Commit C.

## Dependency graph

```
                  Grex.Types
                 /     |    \
                /      |     \
          Walker   Scheduler  Bridge
                \      |     /
                 \     |    /
                   Grex (root)
```

`Grex.Bridge` imports only `Grex.Types` (the shared model). `Grex.Walker`
and `Grex.Scheduler` import `Grex.Types` and `Grex.Bridge` so their
theorems can discharge against the axioms. The root `Grex.lean` re-exports
all four.

Source-of-truth links:
* `.omne/cfg/walker.md` — primary spec for v1.2.0 walker contracts
* `.omne/cfg/concurrency.md` §Lean4 invariant — scheduler contracts
* `.omne/cfg/architecture.md` §Walker invariants / §Runtime invariants
-/

namespace Grex

/-! ## Walker bridges

Open `Walker` to make the walker-model identifiers (Path, ChildRef,
Manifest, World, sync, descends) visible without `Grex.Walker.` prefix.
-/

namespace Walker

/-- **Bridge 1.** Disjoint sub-trees commute under `sync`. This is the
    semantic content of v1.2.0's `cargo-parallel` over rayon: two
    children whose paths share no common descendant can be synchronised
    in either order with the same final `World`. The Rust impl satisfies
    this by virtue of independent working directories + per-pack
    `fd-lock`s; modelling it from first principles would require
    importing the Scheduler proof's lock model into the walker.

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk_recursive`
    (the rayon-driven sibling fan-out).

    **Note on `mkdir_p` ancestor sharing.** `mkdir_p(folder/a)` and
    `mkdir_p(folder/b)` share the `folder/` ancestor — so they are NOT
    strictly FS-op-disjoint. This is fine: Rust's `std::fs::create_dir_all`
    is order-insensitive (idempotent), so the shared-ancestor `mkdir_p`
    sequence converges regardless of interleaving. The "disjoint"
    precondition here is **path-suffix-disjoint** (neither sub-tree
    descends from the other), NOT all-FS-ops-disjoint. The shared
    ancestor `mkdir_p` is captured by the idempotency property of the
    Rust stdlib, not by this axiom. -/
axiom sync_disjoint_commutes
    (p₁ p₂ : Path) (w : World) :
    (¬ descends p₁ p₂) → (¬ descends p₂ p₁) →
    sync p₁ (sync p₂ w) = sync p₂ (sync p₁ w)

/-- **Bridge 2.** A successful `sync` leaves no `.git/` directory at a
    **declared** child path without a recognised `.grex/pack.yaml`.
    Crucially this is restricted to declared paths — the Rust algorithm
    iterates `manifest.children` only and never scans for untracked
    `.git/` at non-declared paths (per `walker.md:41-53`). The Rust
    impl enforces this by failing fast when it discovers a `.git/` at
    a declared dest with no `.grex/pack.yaml` (the `untracked`
    accumulator in Phase 1).

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk_recursive`
    (specifically the `dest_has_git_repo` / `synthesize_plain_git_manifest`
    branch which guarantees every declared dest with a `.git/` is paired
    with a recognised pack — synthetic or real).

    **Quantification scope.** The `child` parameter is constrained by
    `child ∈ m.children` so the axiom does NOT make claims about
    arbitrary made-up children (a vacuous-strong universal would be
    unsound — `tracked = true` for paths the manifest never mentioned
    contradicts `sync_local_writes` if the path lies outside `parent`). -/
axiom sync_no_untracked
    (parent : Path) (w : World) (m : Manifest) (child : ChildRef) :
    child ∈ m.children →
    (sync parent w).hasGit (parent.join child.segments) = true →
    (sync parent w).tracked (parent.join child.segments) = true

/-- **Bridge 3.** `sync` writes only inside its argument subtree. Every
    other path's `tracked`, `lock`, and `hasGit` are unchanged. This is
    the model-level statement of v1.2.0's parent-relative discipline.

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk`
    (the recursive descent that this axiom bridges to). The contract
    relies on **capability-based filesystem ops** — i.e. `cap-std` (or
    equivalent) confining all `open`/`create`/`mkdir` to the
    parent-rooted capability handle. Without this, a malicious symlink
    inside the subtree could cause `sync` to clobber `w.hasGit q` for
    a `q` that does not descend from `parent`, falsifying this axiom.

    The Rust impl's `validate_children_paths` gate (rejects `..` and
    absolute segments) is necessary but NOT sufficient on its own; the
    capability-handle invariant is what closes the symlink-traversal
    escape window. Future-proofing note: any change to the Rust impl
    that swaps `cap-std` for raw `std::fs` MUST re-prove this axiom
    (or bridge it via an explicit "no-symlink-escape" lemma). -/
axiom sync_local_writes
    (parent : Path) (w : World) (q : Path) :
    ¬ descends q parent →
    (sync parent w).tracked q = w.tracked q ∧
    (sync parent w).lock q    = w.lock q    ∧
    (sync parent w).hasGit q  = w.hasGit q

/-- **Bridge 4.** Repeated `sync` is a no-op when nothing in the world's
    underlying tree, lockfile, or filesystem state changed between
    calls. Equivalent to: `sync` is idempotent on its own fixed points.

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk`
    post-condition — "If pre-state of FS + upstream + manifest unchanged,
    post-state equals invocation pre-state."

    **Why the unguarded form is sound here.** `walker.md` §95 conditions
    idempotency on "manifest, FS, and upstream are stable." Our `World`
    model has *no* upstream (no remote-fetch effect), *no* FS-mutation
    primitive outside `sync` itself, and the manifest is captured by
    `w.tree` which `sync` does not mutate. Therefore the conditioned
    form ("if those three are stable") is **vacuously equivalent** to
    the unguarded form on this model: the only mutator IS `sync`, and
    its second invocation re-reads the same `w.tree` from the post-state
    of the first. Any future model that adds an upstream-fetch step
    MUST re-state this axiom with the explicit stability guard. -/
axiom sync_idempotent
    (parent : Path) (w : World) :
    sync parent (sync parent w) = sync parent w

end Walker

/-! ## Scheduler bridges -/

namespace Scheduler

/-- **Axiom 1 / model bridge.** The Rust runtime acquires every lock via
    `Scheduler::run`, which enforces the fixed 5-tier order via its
    outer-to-inner await structure. This axiom asserts the corresponding
    invariant on the Lean model: every acquisition in a reachable state
    satisfies `legalAcquire`.

    Promotion to theorem would require modelling tokio's await semantics
    in Lean — deferred to v2 per `.omne/cfg/concurrency.md` §Deferred. -/
axiom runtime_respects_ordering :
    ∀ (t : Task) (ℓ : Lock), legalAcquire t ℓ

/-- **Axiom 2 / model bridge.** Two distinct tasks that both hold
    `Lock.pack p` for the same path `p` have non-overlapping time
    windows. This encodes the FIFO mutual-exclusion semantics of
    `fd_lock::RwLock::write` as used by `PackLock::acquire` in
    `.omne/cfg/concurrency.md` §Per-pack `PackLock`.

    Promotion to theorem requires modelling `fd-lock`'s kernel-level FIFO
    queue in Lean — deferred to v2 per spec §Non-goals. -/
axiom pack_lock_exclusive
    (a b : Task) (p : String) (wa wb : TimeWindow) :
    Lock.pack p ∈ a.held → Lock.pack p ∈ b.held → a.id ≠ b.id →
    ¬ overlaps wa wb

end Scheduler

end Grex
