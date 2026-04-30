-- Root module for the `Grex` library.
--
-- Re-exports the scheduler and walker proof modules so that `lake build`
-- compiles the entire deliverable scope via a single default target.
--
-- See `.omne/cfg/concurrency.md` §Lean4 invariant and `.omne/cfg/walker.md`.
import Grex.Scheduler
import Grex.Walker
