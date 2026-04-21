# feat-m6-3 — tasks (TDD: Lean-style — state lemmas before proving them)

**Convention**: in Lean-land "tests first" = "write the theorem statement with `sorry`, commit, then iteratively replace the `sorry`s". `sorry` is only tolerated during the in-progress PR commits; the merged commit has zero.

## Stage 1 — Scaffold

- [ ] `mkdir lean/ lean/Grex/`
- [ ] Write `lean/lean-toolchain` — pin current stable (document tag in the commit message).
- [ ] Write `lean/lakefile.lean` — single `lean_lib Grex` default target; no mathlib.
- [ ] Write `lean/Grex.lean` — `import Grex.Scheduler`.
- [ ] Write minimal `lean/Grex/Scheduler.lean` — empty namespace, compiles.
- [ ] `cd lean && lake build` green locally.

## Stage 2 — Model

- [ ] Declare `Lock` inductive type.
- [ ] Declare `tier : Lock → Nat` total function.
- [ ] Declare `Task` structure + `Schedule` alias.
- [ ] Declare `legalAcquire` predicate.
- [ ] Declare `TimeWindow` structure + `overlaps` relation.
- [ ] `lake build` green — no proofs yet, just types.

## Stage 3 — State lemmas (with `sorry`, WIP commit OK)

- [ ] State `axiom runtime_respects_ordering` — document rationale in `/--` doc comment linking to concurrency.md.
- [ ] State `axiom pack_lock_exclusive` — same.
- [ ] State `theorem no_double_lock : ... := by sorry` — verify compiles.
- [ ] State `theorem no_deadlock : ... := by sorry` — verify compiles.

## Stage 4 — Discharge `sorry`s

- [ ] Prove `no_double_lock` using `pack_lock_exclusive`. Straightforward application; < 5 lines.
- [ ] Prove `no_deadlock` via `Nat.lt_trichotomy` on `tier` of two held locks.
- [ ] Replace every `by sorry` with a real proof term.
- [ ] `lake build` green; `grep -r '\bsorry\b\|\badmit\b' lean/Grex/` returns zero matches.
- [ ] `grep -c '^axiom' lean/Grex/Scheduler.lean` returns exactly `2`.

## Stage 5 — CI integration

- [ ] Add `lean-proof` job to `.github/workflows/ci.yml` — ubuntu-latest, `leanprover/lean-action@v1`, `lake build`.
- [ ] Push branch + open draft PR; verify CI lean-proof job green.
- [ ] Coordinate with repo-settings owner to add `lean-proof` to branch-protection required checks.
- [ ] Add `lean/.lake/` + `lean/build/` to `.gitignore`.

## Stage 6 — Polish + acceptance

- [ ] Doc pass: top of `Grex/Scheduler.lean` has module docstring summarising the theorem + linking to `.omne/cfg/concurrency.md`.
- [ ] Axiom doc-comments justify each axiom explicitly as a model-bridge.
- [ ] `lake build` wall-time < 30 s on CI (warm). If over: simplify model.
- [ ] Manual check: introduce a rogue `sorry` in a feature branch, confirm CI fails, revert.
- [ ] Update `progress.md` with closing M6 state (all 3 changes merged).
- [ ] Final: `milestone.md` M6 acceptance boxes all ticked.
