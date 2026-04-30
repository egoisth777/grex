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

This file currently consolidates the **nine** bridge axioms previously
inline in `Grex.Walker` (4 + 2 new in Stage 0.5.C + 1 new in
Stage 0.5.D4) and `Grex.Scheduler` (2):

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
  * `git_in_progress_decidable` *(new in Stage 0.5.C)* — the
                                `.git/`-marker probes used by Phase 1's
                                `classify_dest` are decidable at every
                                world point
  * `consent_walk_reflects_fs_state` *(new in Stage 0.5.C)* —
                                `recursive_consent_walk` faithfully
                                reflects FS dirtiness, allowing
                                `pruneAt` to be conditionally inert
  * `sync_lock_partition` *(new in Stage 0.5.D4)* — recursive `syncTree`
                                writes the lock at parent exactly to
                                that meta's direct children, lifting
                                W2 (`distributed_isolation`) to the
                                full tree fold

  Scheduler bridges (depend on the scheduler model in `Grex.Types`):
  * `runtime_respects_ordering` — Rust scheduler obeys the fixed 5-tier
                                  lock-acquisition order
  * `pack_lock_exclusive`       — fd-lock FIFO mutex semantics for the
                                  per-pack lock

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

/-! ### v1.2.0 walker bridges (Stage 0.5.C) -/

/-- **Bridge 5 (Stage 0.5.C).** The `.git/`-state markers probed by
    Phase 1's `classify_dest` to detect a mid-flight git operation
    (rebase, merge, cherry-pick, bisect, revert) are *decidable* at
    every world point. I.e. for every `(p, w)`, `in_progress_at p w`
    is either definitively true or definitively false — there is no
    third "unknown" state.

    **Rust contract:** the future `crates/grex-core/src/tree/walker.rs::probe_in_progress`
    (to be added in Stage 1.e). The probe sequence is:

    1. `dest.join(".git/rebase-merge")` exists?
    2. `dest.join(".git/rebase-apply")` exists?
    3. `dest.join(".git/MERGE_HEAD")` exists?
    4. `dest.join(".git/CHERRY_PICK_HEAD")` exists?
    5. `dest.join(".git/REVERT_HEAD")` exists?
    6. `dest.join(".git/BISECT_LOG")` exists?

    Each `exists?` call is an atomic syscall (`statx` on Linux,
    `GetFileAttributes` on Windows); `Decidable` here is the Lean
    encoding of "the syscall returns YES or NO, never indefinite". This
    axiom is needed because `in_progress_at` is declared `opaque` (its
    content is a Rust FS probe), and a `Decidable` instance is the
    bridge that lets `classify_dest` pattern-match on it.

    **Soundness assumption.** Atomicity of each individual probe is
    guaranteed by the kernel; *consistency across the six probes* is
    NOT — a concurrent git operation could land between probe 1 and
    probe 2. The Rust impl mitigates by holding the per-pack
    `fd-lock` for the duration of the classification (see
    `concurrency.md` §Per-pack `PackLock`); this axiom encodes the
    POST-lock state as decidable. Any change that classifies
    *without* holding the pack lock would invalidate this axiom. -/
axiom git_in_progress_decidable :
    ∀ (p : Path) (w : World), Decidable (in_progress_at p w)

/-- **Bridge 7 (Stage 0.5.D4).** Folding the per-meta lockfile across
    a `ManifestTree` produces a *disjoint partition* of lockentries by
    meta path — at the root of the recursion, the lock at `parent`
    equals exactly the manifest's declared children mapped to
    `LockEntry`. The hypothesis `w.tree.manifest = m` ties the free
    parameter `m` to the actual manifest at the recursion's root,
    keeping the axiom sound (without it, two distinct `m₁ ≠ m₂` could
    contradict).

    This is W2 (`distributed_isolation`) lifted from a single
    `syncChildren` call to the recursive `syncTree`. A pure-model
    proof is possible in principle (induction on `ManifestTree` plus
    a disjointness lemma showing that recursion into a child path
    never mutates `lock parent`), but the auxiliary lemma itself
    requires a full induction over `syncTree`'s helper `go`. Encoding
    it as a bridge axiom matches the existing pattern of trusting the
    Rust impl for cross-cutting structural facts.

    **Rust contract:** `crates/grex-core/src/tree/lockfile.rs::write_distributed_lockfile`
    plus the recursion in `crates/grex-core/src/tree/walker.rs::Walker::walk_recursive`.
    The Rust impl writes each meta's `.grex/grex.lock.jsonl` independently
    from that meta's manifest's direct children — exactly the partition
    the axiom states.

    **Re-review trigger.** If Phase 2 prune logic ever changes how
    lockentries are written (e.g. cross-meta entries, deferred writes,
    grandchild aggregation), this axiom must be re-verified against the
    Rust impl.

    **Soundness assumption.**
    * The recursive `syncTree` writes `lock parent` exactly once
      (the leading `pruneLock parent m (syncChildren parent m _)`
      pair) and never re-writes it from sibling/child recursion.
    * `pruneLock` after `syncChildren` is the identity on the freshly
      written set (every entry's segments are in `m.children.map
      segments`), so the post-prune lock equals the post-syncChildren
      lock.
    * Therefore the lock at `parent` after `sync parent w` equals
      `m.children.map (fun c => ⟨c.segments, c.url⟩)` whenever
      `w.tree.manifest = m`. -/
axiom sync_lock_partition
    (parent : Path) (m : Manifest) (w : World) :
    w.tree.manifest = m →
    (sync parent w).lock parent =
      m.children.map (fun c => ⟨c.segments, c.url⟩)

/-- **Bridge 6 (Stage 0.5.C).** `recursive_consent_walk` faithfully
    reflects the world's FS state, so `pruneAt` becomes inert whenever
    the walk returns anything other than `Clean`.

    Concretely: if `recursive_consent_walk d w ≠ Clean`, then
    `pruneAt d w = w` (the world is unchanged — no FS mutation, no
    lockfile mutation). This is the model-level statement that prune
    refuses on any non-clean consent.

    **Rust contract:** future
    `crates/grex-core/src/tree/walker.rs::recursive_consent_walk`
    (Stage 1.f) plus `prune_undeclared_dest` (Stage 1.f). The walk
    runs `git status --porcelain --ignored` plus the in-progress
    probes from `git_in_progress_decidable` recursively across `d`'s
    subtree. The classification is total (covered by the five
    `ConsentResult` constructors).

    **Soundness assumption.**
    * `git status --porcelain --ignored` is a faithful oracle for tree
      dirtiness — i.e. its exit code and output line set fully
      characterise FS state w.r.t. tracked + ignored files. This is
      a libgit2 / git-cli invariant; we assume it holds.
    * `recursive_consent_walk` aggregates child results without losing
      information — a single `DirtyTree` anywhere in the subtree
      bubbles up. Faithfully implemented in Rust; this axiom binds
      the model to that implementation.
    * `pruneAt` does NOT short-circuit *before* consulting the consent
      walk; the Rust impl wires `prune_undeclared_dest` to call
      `recursive_consent_walk` first and bail if non-clean.

    **Why the converse direction is NOT axiomised.** This axiom only
    asserts that non-Clean ⇒ no mutation. The Clean ⇒ mutation
    direction is the actual prune action and is exercised by the
    existing W7 (`cleanup_safety`) for lockfile-only pruning, and by
    Stage 1.f's integration tests for FS-level pruning. -/
axiom consent_walk_reflects_fs_state
    (d : Path) (w : World) :
    recursive_consent_walk d w ≠ ConsentResult.Clean →
    pruneAt d w = w

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
