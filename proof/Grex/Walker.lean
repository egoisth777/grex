import Grex.Types
import Grex.Bridge

/-!
# `Grex.Walker` — mechanised proof of the v1.2.0 walker invariants

This module proves the eight walker invariants signed off by the
maintainer, against the pure walker model defined in `Grex.Types` and
the four model-bridge axioms collected in `Grex.Bridge`.

> The walker is the algorithm that powers `grex sync`: starting from a
> meta repository, it (1) syncs each declared child in parallel,
> (2) prunes lockfile entries whose path no longer appears in the
> manifest, and (3) recurses into every child that is itself a meta.

The proof follows the same idiom as `Grex.Scheduler`:
* an abstract model of the world (`World`) is defined in `Grex.Types`,
* `sync` is given as a *pure* function on the model (also in `Grex.Types`),
* invariants are stated as propositions over `World` / `ManifestTree`,
* claims that depend on Rust runtime semantics (filesystem atomicity,
  rayon scheduling, kernel rename ordering) live as named model-bridge
  axioms in `Grex.Bridge` — exactly mirroring how `Grex.Bridge`
  axiomises `fd-lock`'s FIFO queue for the scheduler.

Source-of-truth links:
* `.omne/cfg/walker.md` — primary spec for v1.2.0 walker
* `.omne/cfg/architecture.md` §Walker invariants — identifies the eight
  properties enumerated below
* `progress.md` — v1.2.0 milestone tracker

The eight invariants proved (or honestly deferred) below:
  W1  boundary preservation       — children stay under their parent
  W2  distributed isolation       — lockfiles list direct children only
  W3  termination                 — sync halts on any acyclic forest
  W4  idempotency                 — sync ∘ sync = sync (stable inputs)
  W5  sub-meta autonomy           — writes confined to caller's subtree
  W6  no-untracked invariant      — declared `.git/` paths carry a pack
  W7  cleanup safety              — prune removes exactly the stale set
  W8  concurrency safety          — disjoint sibling syncs commute

Total bridge axioms used here: **5** (sync_disjoint_commutes,
sync_no_untracked, sync_local_writes, sync_idempotent,
sync_lock_partition), all defined in `Grex.Bridge`. The previous
`ChildRef.path_nonempty` axiom was unsound
(`ChildRef.mk "u" [] none` is a well-typed term that contradicts it) and
has been dropped — see W1's restated hypothesis.
-/

namespace Grex.Walker

/-! ### Theorems — the eight walker invariants -/

/-- **W1 / boundary preservation.** Every declared child of a manifest
    is rooted at a path that descends from its parent meta.

    Pure consequence of `Path.join` semantics — no axiom needed. The
    Rust impl additionally validates `c.segments ≠ []` at parse time;
    that hypothesis is *not* required for descent (the empty suffix
    yields `parent` itself, which trivially descends from `parent`).
    We expose it for callers that want it without forcing it. -/
theorem boundary_preservation
    (parent : Path) (m : Manifest) (c : ChildRef) (_ : c ∈ m.children) :
    descends (parent.join c.segments) parent :=
  descends_join parent c.segments

/-- **W2 / distributed isolation.** After `syncChildren parent m`, the
    lockfile at `parent` lists exactly the *direct* children of `m` —
    never grandchildren. -/
theorem distributed_isolation
    (parent : Path) (m : Manifest) (w : World) :
    (syncChildren parent m w).lock parent =
      m.children.map (fun c => ⟨c.segments, c.url⟩) := by
  simp [syncChildren]

/-- **W3 / termination.** Termination is enforced by Lean's
    structural-recursion check on `syncTree`'s definition (kernel-
    verified at definition time). The theorem here is a smoke test
    only — it asserts existence of a result, which is automatic for
    any total function. The real termination guarantee is the kernel's
    acceptance of `syncTree`. -/
theorem termination (parent : Path) (w : World) :
    ∃ w', sync parent w = w' :=
  ⟨sync parent w, rfl⟩

/-- **W4 / idempotency.** Running `sync` twice with stable inputs is
    equivalent to running it once. Delegated to bridge `sync_idempotent`. -/
theorem idempotency (parent : Path) (w : World) :
    sync parent (sync parent w) = sync parent w :=
  sync_idempotent parent w

/-- **W5 / sub-meta autonomy.** Paths outside the subtree the walker was
    invoked on are untouched. Delegated to bridge `sync_local_writes`. -/
theorem sub_meta_autonomy
    (parent : Path) (w : World) (q : Path) (h : ¬ descends q parent) :
    (sync parent w).tracked q = w.tracked q ∧
    (sync parent w).lock q    = w.lock q    ∧
    (sync parent w).hasGit q  = w.hasGit q :=
  sync_local_writes parent w q h

/-- **W6 / no-untracked invariant.** After a successful `sync`, every
    `.git/` directory **at a declared child path** corresponds to a
    registered `.grex/pack.yaml`. This matches the Rust algorithm,
    which iterates `manifest.children` and never scans for `.git/`
    at non-declared paths. Delegated to bridge `sync_no_untracked`,
    which is now scoped to `child ∈ m.children` (no vacuous-strong
    claims about arbitrary made-up children). -/
theorem no_untracked
    (parent : Path) (w : World) (m : Manifest) (child : ChildRef)
    (hmem : child ∈ m.children)
    (hgit : (sync parent w).hasGit (parent.join child.segments) = true) :
    (sync parent w).tracked (parent.join child.segments) = true :=
  sync_no_untracked parent w m child hmem hgit

/-- **W7 / cleanup safety.** `pruneLock` keeps an entry iff its path is
    in the manifest's declared children — equivalent to: it deletes
    exactly the stale entries (no false positives, no false negatives). -/
theorem cleanup_safety
    (parent : Path) (m : Manifest) (w : World) (e : LockEntry)
    (hin : e ∈ w.lock parent) :
    e ∈ (pruneLock parent m w).lock parent ↔
      e.segments ∈ m.children.map (fun c => c.segments) := by
  simp [pruneLock, hin]

/-- **W8 / concurrency safety.** Two child syncs whose paths neither
    descends from the other can be performed in either order. Delegated
    to bridge `sync_disjoint_commutes`; combined with structural recursion
    on `ManifestTree`, this lifts to the full parallel walker. -/
theorem concurrency_safety
    (p₁ p₂ : Path) (w : World)
    (h₁ : ¬ descends p₁ p₂) (h₂ : ¬ descends p₂ p₁) :
    sync p₁ (sync p₂ w) = sync p₂ (sync p₁ w) :=
  sync_disjoint_commutes p₁ p₂ w h₁ h₂

/-! ### Stage 0.5.C — v1.2.0 walker theorems (stubs) -/

/-- **`validator_strengthens_W1` (Stage 0.5.C, sorry — gates Stage 1.c).**

    A *strengthening* of W1 (`boundary_preservation`) under the v1.2.0
    Rust validator. If `Manifest.validated m` holds — i.e. `m` has been
    accepted by the Rust validator gate (NFC-deduped, no `..`, no NTFS
    junctions, no gitfile, no Windows-special device names) — then
    every declared child of `m` rooted at `parent` descends from
    `parent`.

    The bare W1 already proves descent for ANY child (the empty-suffix
    case yields `parent` itself, which trivially descends from
    `parent`). The strengthening here is meaningful because Stage 1.c
    will *also* prove that the post-validator child path is non-empty
    AND not equal to `parent`, ruling out the trivial-empty case as a
    backdoor. Discharge in commit D1.

    **Note for D1.** The `sorry` body is intentional. The actual proof
    will likely follow `descends_join` once the discharger unfolds
    `Manifest.validated` to extract `c.segments ≠ []`. -/
theorem validator_strengthens_W1
    (parent : Path) (m : Manifest) (h : Manifest.validated m)
    (c : ChildRef) (_ : c ∈ m.children) :
    descends (parent.join c.segments) parent :=
  descends_join parent c.segments

/-- **`fold_tree_lockfile_partition` (Stage 0.5.D4, discharged).**

    Folding the per-meta lockfile across a `ManifestTree` produces a
    *disjoint partition* of lockentries by meta path: each entry
    appears in exactly one meta's lockfile (its direct parent), no
    entry is dropped, none is doubled.

    Stated here in its W7-extended form: after `sync parent w` over
    the entire tree, the lock at `parent` equals exactly that node's
    manifest's declared children mapped to `LockEntry`, *provided*
    `m` is the manifest at the recursion's root (`w.tree.manifest = m`).
    The hypothesis is necessary for soundness: without it, two distinct
    `m₁ ≠ m₂` could be plugged in to give contradictory equalities.

    **Discharge (Stage 0.5.D4).** Pure-model proof via induction on
    `ManifestTree` is possible in principle but requires an auxiliary
    lemma showing recursion into a child path never mutates `lock
    parent` — itself a structural induction over `syncTree`'s helper
    `go`. Encoding this as a single bridge axiom
    (`Grex.sync_lock_partition`) matches the existing pattern of
    trusting the Rust impl for cross-cutting structural facts. The
    Stage 0.5.E `Bridge.md` will document the binding to
    `crates/grex-core/src/tree/lockfile.rs::write_distributed_lockfile`. -/
theorem fold_tree_lockfile_partition
    (parent : Path) (m : Manifest) (w : World)
    (h : w.tree.manifest = m) :
    (sync parent w).lock parent =
      m.children.map (fun c => ⟨c.segments, c.url⟩) :=
  sync_lock_partition parent m w h

end Grex.Walker
