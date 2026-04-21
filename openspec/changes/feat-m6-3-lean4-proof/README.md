# feat-m6-3-lean4-proof

**Status**: draft

Mechanized Lean4 proof of `Grex.Scheduler.no_double_lock`. Scaffolds `lean/` at repo root (`lakefile.lean`, `lean-toolchain`, `Grex/Scheduler.lean`). Models the 5-tier lock hierarchy as an abstract state machine and proves no cycle is possible under the fixed ordering rule, so no two concurrent tasks hold `PackLock` on the same path. Wires `lake build` into `.github/workflows/ci.yml` (Linux-only job). Zero `sorry` / `admit` in deliverable scope.
