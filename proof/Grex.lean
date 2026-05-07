-- Root module for the `Grex` library.
--
-- Re-exports the types model, the bridge axioms, and the scheduler/walker
-- proof modules so that `lake build` compiles the entire deliverable scope
-- via a single default target.
--
-- Dependency graph (Stage 0.5.C):
--                Grex.Types
--             /    |    |    \
--            /     |    |     \
--      Walker  Scheduler |   Bridge
--            \    |    |     /
--             \   |    |    /
--      Phase1  \  |    |  /  Consent
--           \   \ |    | /   /
--            \---Grex (root)----
--
-- `Grex.Phase1` and `Grex.Consent` are Stage 0.5.C additions that import
-- Types + Bridge to state v1.2.0 walker theorems (D1–D4 all discharged
-- — zero `sorry`, zero `admit` across `proof/Grex/`).
--
-- See `inst/concurrency.md` §Lean4 invariant and `inst/walker.md`.
import Grex.Types
import Grex.Bridge
import Grex.Scheduler
import Grex.Walker
import Grex.Phase1
import Grex.Consent
import Grex.Quarantine
import Grex.Lockfile
import Grex.Ref
