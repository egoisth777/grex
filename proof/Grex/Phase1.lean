import Grex.Types
import Grex.Bridge

/-!
# `Grex.Phase1` — five-way destination classifier (Stage 0.5.C stub)

This module hosts the totality theorem for the Phase 1 destination
classifier introduced by v1.2.0's walker rewrite. It is a thin module
that imports `Grex.Types` (for the `DestClass` inductive and the opaque
`classify_dest`) and `Grex.Bridge` (for `git_in_progress_decidable`,
which the discharger will need to reflect on the in-progress probe).

Source-of-truth links:
* `.omne/walker.md` §Phase 1 — five-way classifier contract
* `progress.md` — Stage 1.e (`classify_dest_total`) discharge endpoint

The theorem body is `sorry` in this commit; discharge lands in commit D2.
-/

namespace Grex.Walker

/-! ### Phase 1 — five-way destination classifier -/

/-- **`classify_dest_total` (Stage 0.5.C, sorry — gates Stage 1.e).**

    For every `(parent, child, world)` triple, `classify_dest` returns
    *exactly one* of the five `DestClass` constructors. Stated here as
    "the result inhabits the union of the five tags," which holds
    structurally for any total function into `DestClass` — but the
    skeleton is needed because Stage 1.e will *strengthen* the claim
    to "exactly one" (i.e. proving the classification is well-defined
    and not just total) using `git_in_progress_decidable` from
    `Grex.Bridge`.

    The five tags partition the destination state space exhaustively:
    * `Missing` — dest does not exist
    * `PresentDeclared` — dest exists with recognised pack
    * `PresentDirty` — dest exists with porcelain-dirty tree
    * `PresentInProgress` — dest exists with `.git/`-marker mid-flight
    * `PresentUndeclared` — dest exists but is not in parent's `pack.yaml`

    **Note for D2.** Discharge proceeds by case-splitting on
    `classify_dest`'s opaque result; each branch corresponds to one
    constructor. Lean's exhaustivity checker on `match` over
    `DestClass` automatically gives the totality, but expressing the
    "exactly one" intent (mutual exclusion) requires the
    `git_in_progress_decidable` instance from `Grex.Bridge`. -/
theorem classify_dest_total
    (parent : Path) (child : ChildRef) (w : World) :
    classify_dest parent child w = DestClass.Missing ∨
    classify_dest parent child w = DestClass.PresentDeclared ∨
    classify_dest parent child w = DestClass.PresentDirty ∨
    classify_dest parent child w = DestClass.PresentInProgress ∨
    classify_dest parent child w = DestClass.PresentUndeclared := by
  cases h : classify_dest parent child w
  · left; rfl
  · right; left; rfl
  · right; right; left; rfl
  · right; right; right; left; rfl
  · right; right; right; right; rfl

end Grex.Walker
