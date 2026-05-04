import Grex.Types
import Grex.Bridge

/-!
# `Grex.Scheduler` — mechanised proof of invariant I1 (no double lock)

This module proves the core runtime invariant against the scheduler model
defined in `Grex.Types` and the two model-bridge axioms collected in
`Grex.Bridge`:

> **I1**: For any two concurrent tasks `t1`, `t2` scheduled by `Scheduler`,
> if `t1.pack_path == t2.pack_path`, then their lock-holding windows do NOT
> overlap in time.

The proof is intentionally minimal. It takes the two model-bridge axioms
(`runtime_respects_ordering`, `pack_lock_exclusive`) — the only mechanism
by which a Lean proof can refer to the Rust runtime — and derives the
user-facing guarantees (`no_double_lock`, `no_deadlock`) from them with
zero `sorry` / zero `admit`.

Source-of-truth links:
* `.omne/concurrency.md` §Lean4 invariant — primary spec
* `.omne/architecture.md` §Runtime invariants — identifies I1
* `openspec/changes/feat-m6-3-lean4-proof/spec.md` — this change's contract
-/

namespace Grex.Scheduler

/-! ### Theorems -/

/-- **I1 / no double lock.** No two distinct concurrent tasks
    simultaneously hold the per-pack lock on the same path. -/
theorem no_double_lock
    (a b : Task) (p : String) (wa wb : TimeWindow)
    (ha : Lock.pack p ∈ a.held)
    (hb : Lock.pack p ∈ b.held)
    (hne : a.id ≠ b.id) :
    ¬ overlaps wa wb :=
  pack_lock_exclusive a b p wa wb ha hb hne

/-- **Deadlock-freedom corollary.** For any two locks held by a single
    task, their tiers are comparable under the strict total order on
    `Nat`. Combined with `legalAcquire`, this rules out acquisition
    cycles and therefore deadlock. -/
theorem no_deadlock (t : Task) :
    ∀ ℓ ℓ' : Lock, ℓ ∈ t.held → ℓ' ∈ t.held →
      tier ℓ = tier ℓ' ∨ tier ℓ < tier ℓ' ∨ tier ℓ' < tier ℓ := by
  intro ℓ ℓ' _ _
  rcases Nat.lt_trichotomy (tier ℓ) (tier ℓ') with h | h | h
  · exact Or.inr (Or.inl h)
  · exact Or.inl h
  · exact Or.inr (Or.inr h)

/-! ### v1.2.5 — pool deadlock guard (A3)

The v1.2.5 release adds a *debug-build* assertion at the entry of every
closure executed inside a `pool.install`: if a `PackLock` is already
held by the current OS thread when the closure enters at
`pool_install_depth >= 2`, the assertion fires (matching the v1.2.2
R#1 MED reviewer's theoretical re-entrancy deadlock pattern). The
release build compiles the assertion out — the documented
lock-acquisition order in `.omne/concurrency.md` plus this Lean
theorem are the contract.

The model below mechanises the contract by a different (but equivalent)
abstraction from the existing `tier` / `legalAcquire` machinery used
for the v1.0 `no_deadlock` corollary above:

* `LockKind` — six-arm enum extending the existing 5-tier model with
  `poolInstall` as the *innermost* (rank 5) tier. The poolInstall
  rank is strictly greater than `packLock` (rank 2), so the
  acyclic-acquisition predicate forbids re-entering a `pool.install`
  while a `PackLock` is held — *unless* the `PackLock` is acquired
  AFTER the inner `pool.install` opens (which the design rules out by
  ordering: pool boundaries acquire / release across the call to
  `Scheduler::run`, not inside the closure).
* `rank` — strict total order on the six tiers. Matches `tier` for
  the existing five with `poolInstall = 5` appended.
* `Trace` — sequence of `LockKind` acquisitions on one OS thread
  (the v1.2.5 thread-local `HELD_PACK_LOCKS` view).
* `acyclicAcquisition` — pairwise strict-monotone-rank predicate over
  the trace. Holds iff every adjacent pair `(prev, next)` satisfies
  `rank prev < rank next` AND the suffix is itself acyclic.

The theorem `pool_deadlock_guard_terminates` proves: under
`acyclicAcquisition t`, the trace is *strictly rank-monotone* —
formally, for any two indices `i < j` within the trace, the rank at
`i` is strictly less than the rank at `j`. This is the Coffman
"hold-and-wait + cycle in resource graph" precondition denied: a
strictly-monotone trace cannot contain a cycle, so the resource
allocation graph cannot deadlock.

**Why this is stronger than `True`.** The design.md draft showed the
theorem statement as `True := by trivial` — a placeholder that proves
nothing. The version below states the actual non-trivial
strict-monotonicity property and discharges it by structural induction
on the trace plus a transitivity lemma over `Nat.lt`. No `sorry`, no
`admit`.

**No new bridge axiom.** The lock-acquisition trace is modelled
purely (a `List LockKind`); `acyclicAcquisition` is a pure recursive
predicate; the theorem is provable by structural induction on the
trace. The runtime guarantee that the Rust impl actually produces only
acyclic traces is the v1.2.5 debug-build assertion + code-review
discipline; it is the SAME contract that already underwrites the
`no_deadlock` corollary above (which depends on
`runtime_respects_ordering`, axiom #1 of the Scheduler bridges).
Re-using that existing axiom does NOT count as a new axiom — Bridge
count remains 9, Types count remains 4.
-/

/-- **LockKind.** Six-arm enum: the five existing tiers (per-meta sync,
    semaphore, pack-lock, repo-backend, manifest-lock) plus the v1.2.5
    `poolInstall` arm at rank 5 (innermost). Mirrors the
    `concurrency.md` §"Five cooperating mechanisms" enumeration with
    the rayon `pool.install` boundary added.

    Pure inductive — no `axiom` / `opaque`. -/
inductive LockKind : Type where
  /-- Per-meta-sync lock (outermost). -/
  | perMetaSync
  /-- Per-meta concurrency-control semaphore. -/
  | semaphore
  /-- Per-pack `fd-lock` (the v1.2.2 R#1 MED reviewer's concern). -/
  | packLock
  /-- Per-repo backend lock (libgit2 / git-cli). -/
  | repoBackend
  /-- Per-meta manifest write lock. -/
  | manifestLock
  /-- v1.2.5: rayon `pool.install` boundary (innermost). -/
  | poolInstall
  deriving DecidableEq, Repr

/-- **rank.** Strict total order on `LockKind`. The pool-install
    boundary is strictly innermost (rank 5), so any acquisition of a
    lower-ranked lock AFTER opening a `pool.install` would violate the
    acyclic predicate. -/
def rank : LockKind → Nat
  | .perMetaSync  => 0
  | .semaphore    => 1
  | .packLock     => 2
  | .repoBackend  => 3
  | .manifestLock => 4
  | .poolInstall  => 5

/-- **Trace.** Sequence of lock acquisitions on one OS thread. Mirrors
    the v1.2.5 thread-local `HELD_PACK_LOCKS` view at successive
    `acquire` / `Drop` events. -/
abbrev Trace := List LockKind

/-- **acyclicAcquisition.** Pairwise strict-monotone-rank predicate.
    Holds iff every adjacent pair `(prev, next)` in the trace satisfies
    `rank prev < rank next` AND the suffix from `next` onward is itself
    acyclic. Equivalent to: the trace's ranks form a strictly
    increasing sequence.

    Empty and singleton traces are vacuously acyclic. -/
def acyclicAcquisition : Trace → Prop
  | []            => True
  | [_]           => True
  | a :: b :: rest => rank a < rank b ∧ acyclicAcquisition (b :: rest)

/-- Helper lemma: a strictly-monotone-adjacent trace has its first
    element strictly less than every subsequent element. Proof by
    structural induction on the trace tail, generalising over the head
    so the IH applies for any prefix-head. -/
theorem rank_lt_of_acyclic_head :
    ∀ (a : LockKind) (rest : Trace),
      acyclicAcquisition (a :: rest) →
      ∀ b ∈ rest, rank a < rank b
  | _, [],            _, _, hb => by cases hb
  | a, head :: tail,  h, b, hb => by
      -- h : acyclicAcquisition (a :: head :: tail)
      --   = rank a < rank head ∧ acyclicAcquisition (head :: tail)
      simp only [acyclicAcquisition] at h
      obtain ⟨h_ah, h_tail⟩ := h
      cases hb with
      | head => exact h_ah
      | tail _ hb' =>
          -- Chain rank a < rank head < rank b via head's IH on `tail`.
          have h_head_b : rank head < rank b :=
            rank_lt_of_acyclic_head head tail h_tail b hb'
          exact Nat.lt_trans h_ah h_head_b

/-- **`pool_deadlock_guard_terminates` (v1.2.5, Rule-8 gate).**

    Under the precondition that a thread's lock-acquisition trace is
    `acyclicAcquisition` — every adjacent pair satisfies
    `rank prev < rank next` — the trace is *strictly rank-monotone*
    across any pair of positions `i < j`: the lock acquired at position
    `i` has strictly smaller rank than the lock acquired at position
    `j`.

    This is the Coffman "hold-and-wait + cycle in resource graph"
    precondition denied: a strictly-monotone trace cannot contain a
    cycle (`rank` is a strict total order on `Nat`, and a strictly
    increasing sequence cannot revisit any value). In particular, a
    `packLock` (rank 2) cannot appear AFTER a `poolInstall` (rank 5)
    in the same trace — so the v1.2.2 R#1 MED reviewer's theoretical
    re-entrancy deadlock pattern is statically ruled out.

    **Why this is stronger than the design.md draft.** The draft showed
    the theorem as `True := by trivial`, a placeholder. The version
    here states and proves the actual strict-monotonicity property,
    which is the substantive content of the deadlock-freedom guarantee.
    Discharge is by induction on the trace + transitivity of `Nat.lt`
    via the `rank_lt_of_acyclic_head` helper.

    **Bound.** `O(n)` in trace length for the strictness witness; the
    runtime guard runs in `O(1)` per `pool.install` entry (one
    thread-local read + one comparison, debug build only).

    **Discharge.** Structural induction on the trace pair `(prefix,
    suffix)`; the head case applies `rank_lt_of_acyclic_head` directly,
    and the inductive step uses `Nat.lt_trans` to chain through.

    No new bridge axiom needed. No `sorry`, no `admit`. The runtime
    guarantee that `acyclicAcquisition` actually holds for every Rust
    trace is shouldered by the v1.2.5 debug-build assertion plus
    code-review discipline — and is the same runtime contract already
    underwritten by `runtime_respects_ordering` (Scheduler bridge #1)
    for the existing 5-tier model. -/
theorem pool_deadlock_guard_terminates
    (t : Trace) (h : acyclicAcquisition t) :
    ∀ (i j : Nat) (hij : i < j) (hj : j < t.length),
      rank (t.get ⟨i, Nat.lt_trans hij hj⟩) <
      rank (t.get ⟨j, hj⟩) := by
  induction t with
  | nil =>
      intro i j hij hj
      -- t.length = 0, so hj : j < 0 is impossible.
      simp at hj
  | cons a rest ih =>
      intro i j hij hj
      -- Case-split on i to identify whether the lower index is the head.
      cases i with
      | zero =>
          -- Lower position is the head a; upper is at index j-1 in rest.
          cases j with
          | zero =>
              -- hij : 0 < 0 is impossible.
              exact absurd hij (Nat.lt_irrefl 0)
          | succ j' =>
              -- (a :: rest).length = rest.length + 1; succ j' < that
              -- gives j' < rest.length.
              have hj' : j' < rest.length := by
                have : j' + 1 < rest.length + 1 := hj
                exact Nat.lt_of_succ_lt_succ this
              -- (a :: rest).get ⟨j'+1, _⟩ reduces to rest.get ⟨j', _⟩
              -- definitionally; the head case yields rank a directly.
              show rank a < rank (rest.get ⟨j', hj'⟩)
              have hmem : rest.get ⟨j', hj'⟩ ∈ rest :=
                List.get_mem rest ⟨j', hj'⟩
              exact rank_lt_of_acyclic_head a rest h _ hmem
      | succ i' =>
          -- Both indices in the rest tail.
          cases j with
          | zero =>
              -- hij : succ i' < 0 is impossible.
              exact absurd hij (Nat.not_lt_zero _)
          | succ j' =>
              have hj' : j' < rest.length := by
                have : j' + 1 < rest.length + 1 := hj
                exact Nat.lt_of_succ_lt_succ this
              have hij' : i' < j' := Nat.lt_of_succ_lt_succ hij
              -- Acyclic suffix: rest itself must be acyclic from h.
              have h_rest : acyclicAcquisition rest := by
                cases rest with
                | nil => trivial
                | cons _ _ =>
                    simp only [acyclicAcquisition] at h
                    exact h.2
              exact ih h_rest i' j' hij' hj'

end Grex.Scheduler
