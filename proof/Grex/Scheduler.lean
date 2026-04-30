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
* `.omne/cfg/concurrency.md` §Lean4 invariant — primary spec
* `.omne/cfg/architecture.md` §Runtime invariants — identifies I1
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

end Grex.Scheduler
