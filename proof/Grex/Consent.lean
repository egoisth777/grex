import Grex.Types
import Grex.Bridge

/-!
# `Grex.Consent` — Phase 2 prune-only-on-clean-consent (Stage 0.5.C stub)

This module hosts the safety theorem for the Phase 2 recursive consent
walk introduced by v1.2.0. It establishes that prune is *conditionally
inert*: lockfile and FS are unchanged at `d` whenever the consent walk
returns anything other than `Clean`.

The theorem delegates to bridge axiom `consent_walk_reflects_fs_state`
in `Grex.Bridge`. The body is `sorry` in this commit so the proof
skeleton compiles while Stage 1.f lands the actual `recursive_consent_walk`
discharge in commit D3.

Source-of-truth links:
* `.omne/cfg/walker.md` §Phase 2 — prune safety + recursive consent
* `progress.md` — Stage 1.f (`prune_only_on_clean_consent`) discharge
-/

namespace Grex.Walker

/-! ### Phase 2 — prune safety -/

/-- **`prune_only_on_clean_consent` (Stage 0.5.C, sorry — gates Stage 1.f).**

    If `recursive_consent_walk d w` returns anything other than
    `ConsentResult.Clean`, then `pruneAt d w = w` — i.e. the lockfile
    and filesystem at `d` are unchanged.

    Equivalently: prune executes (mutates the world) iff the consent
    walk returned `Clean`. This is the v1.2.0 safety guarantee for
    pruning undeclared dests: an undeclared dest with a dirty tree, an
    in-progress git operation, or dirty sub-meta children is REFUSED
    until the user explicitly forces it (Stage 1.l adds the
    `--force-prune` opt-in for the `DirtyTreeWithIgnored` subset).

    **Note for D3.** Discharge is a one-liner once
    `consent_walk_reflects_fs_state` (bridge 6) is in scope: the bridge
    axiom is exactly the contrapositive form of this theorem. The
    `sorry` skeleton lets `lake build` continue while Stage 1.f
    finalises. -/
theorem prune_only_on_clean_consent
    (d : Path) (w : World)
    (h : recursive_consent_walk d w ≠ ConsentResult.Clean) :
    pruneAt d w = w := by
  sorry

end Grex.Walker
