import Lake
open Lake DSL

package «grex» where
  -- No external dependencies (no mathlib) — keeps CI time minimal and
  -- dep surface small. See inst/concurrency.md §Lean4 invariant.

@[default_target]
lean_lib «Grex» where
  -- Root: proof/Grex.lean ; submodules under proof/Grex/.
