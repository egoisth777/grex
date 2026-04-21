-- Root module for the `Grex` library.
--
-- Re-exports the scheduler proof module so that `lake build` compiles the
-- entire deliverable scope via a single default target.
--
-- See `.omne/cfg/concurrency.md` §Lean4 invariant.
import Grex.Scheduler
