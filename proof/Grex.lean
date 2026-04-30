-- Root module for the `Grex` library.
--
-- Re-exports the types model, the bridge axioms, and the scheduler/walker
-- proof modules so that `lake build` compiles the entire deliverable scope
-- via a single default target.
--
-- Dependency graph:
--                Grex.Types
--               /     |    \
--              /      |     \
--        Walker   Scheduler  Bridge
--              \      |     /
--               \     |    /
--                  Grex (root)
--
-- See `.omne/cfg/concurrency.md` §Lean4 invariant and `.omne/cfg/walker.md`.
import Grex.Types
import Grex.Bridge
import Grex.Scheduler
import Grex.Walker
