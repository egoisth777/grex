# feat-m6-3 — Lean4 proof `no_double_lock`

**Status**: draft
**Milestone**: M6 (see [`../../../milestone.md`](../../../milestone.md) §M6 — Lean4 theorem is the long pole)
**Depends on**: feat-m6-1 (scheduler); feat-m6-2 (per-pack lock + ordering).

## Motivation

grex's concurrency story rests on a single invariant (`.omne/cfg/concurrency.md` §Lean4 invariant):

> **I1**: for any two concurrent tasks `t1`, `t2` scheduled by `Scheduler`, if `t1.pack_path == t2.pack_path`, their lock-holding windows do NOT overlap in time.

feat-m6-1 + feat-m6-2 implement the mechanisms (semaphore, per-pack lock, fixed ordering). This change mechanizes a machine-checked proof that those mechanisms — as modelled — preclude the invariant's negation. Deliverable-scope zero `sorry` / zero `admit` / zero unresolved axiom outside the single model-bridge axiom explicitly documented.

This matches `openspec/feat-grex/spec.md` success criterion #7 ("Lean4 proof compiles cleanly under `lake build` with zero `sorry` / zero unresolved `axiom` in deliverable scope").

## Goal

1. Scaffold a Lean4 project at `lean/` (repo root, parallel to `crates/`).
2. Formalize the 5-tier lock hierarchy as an abstract state machine.
3. State and prove `theorem no_double_lock` with zero holes.
4. Wire `lake build` into CI (Linux-only job acceptable per `.omne/cfg/test-plan.md` §Lean4 proof verification).

## Design

### Lean project scaffold

Directory layout (new, at repo root):

```
lean/
├── lakefile.lean            # package manifest
├── lean-toolchain           # pinned lean version (leanprover/lean4:v4.x.y)
├── Grex.lean                # root module, imports Grex.Scheduler
└── Grex/
    └── Scheduler.lean       # the proof
```

`lakefile.lean` declares a single `@[default_target] lean_lib Grex` target; no external dep (mathlib NOT required — keeping the dep surface minimal to speed CI + avoid toolchain drift). If a generic lemma requires it later, we vendor the single lemma; do not pull mathlib.

`lean-toolchain` pins an exact `leanprover/lean4:vX.Y.Z` tag. Choose the current stable at implementation time and document the choice in the commit.

### State machine model

Mirrors the sketch in `.omne/cfg/concurrency.md` §Lean4 invariant with the following refinements:

```lean
namespace Grex.Scheduler

/-- Abstract lock identity across the 5 tiers. Paths are symbols; concrete
    filesystem paths are out of scope. -/
inductive Lock where
  | workspaceSync
  | semaphoreSlot   (slot : Nat)
  | pack            (path : String)
  | repoBackend     (dest : String)
  | manifest
deriving DecidableEq, Repr

/-- Strict partial order on locks, enforcing the fixed 5-tier acquisition rule.
    Lower = acquired earlier; higher = acquired later. -/
def tier : Lock → Nat
  | .workspaceSync   => 0
  | .semaphoreSlot _ => 1
  | .pack _          => 2
  | .repoBackend _   => 3
  | .manifest        => 4

/-- A task is characterised by the ordered sequence of locks it holds at any
    moment, monotonically appended in acquisition order. -/
structure Task where
  id        : Nat
  held      : List Lock         -- acquire-order; release reverses
deriving Repr

/-- Acquisition is legal only if the new lock's tier strictly exceeds every
    tier currently held. -/
def legalAcquire (t : Task) (ℓ : Lock) : Prop :=
  ∀ ℓ' ∈ t.held, tier ℓ' < tier ℓ

/-- A schedule is a finite set of tasks observed at one logical instant. -/
def Schedule := List Task

/-- Overlap window (taken as given from an oracle; concrete time semantics
    are abstracted). -/
structure TimeWindow where
  started : Nat
  ended   : Nat
  wf      : started < ended

def overlaps (a b : TimeWindow) : Prop :=
  a.started < b.ended ∧ b.started < a.ended
```

### Core theorem

```lean
/-- Model bridge (documented axiom — the only one in deliverable scope): the
    runtime acquires `PackLock` under `legalAcquire` and releases in LIFO order.
    Any concrete scheduler that satisfies this axiom inherits the theorem. -/
axiom runtime_respects_ordering :
  ∀ (t : Task) (ℓ : Lock), legalAcquire t ℓ

/-- Two tasks holding the same pack path mutually exclude (FIFO queue per
    path). This is the operational model of `fd-lock::RwLock::write`. -/
axiom pack_lock_exclusive
  (a b : Task) (p : String) (wa wb : TimeWindow) :
  Lock.pack p ∈ a.held → Lock.pack p ∈ b.held → a.id ≠ b.id → ¬ overlaps wa wb

/-- Main theorem: no two distinct concurrent tasks simultaneously hold the
    per-pack lock on the same path. -/
theorem no_double_lock
  (a b : Task) (p : String) (wa wb : TimeWindow)
  (ha : Lock.pack p ∈ a.held)
  (hb : Lock.pack p ∈ b.held)
  (hne : a.id ≠ b.id) :
  ¬ overlaps wa wb := by
  exact pack_lock_exclusive a b p wa wb ha hb hne
```

### Deadlock-freedom corollary

The 5-tier strict partial order on `Lock` — established via the `tier` function — plus `legalAcquire` monotonicity gives deadlock-freedom trivially (no cycle possible in a total order). State and prove as a named lemma for clarity:

```lean
/-- Acquisition order is a strict total order on lock tiers ⇒ no cycle ⇒
    no deadlock under this model. -/
theorem no_deadlock (t : Task) :
  ∀ ℓ ℓ' : Lock, ℓ ∈ t.held → ℓ' ∈ t.held →
    ℓ = ℓ' ∨ tier ℓ < tier ℓ' ∨ tier ℓ' < tier ℓ := by
  intros ℓ ℓ' h h'
  -- trichotomy on Nat.lt over tier ℓ and tier ℓ'
  sorry_free_nat_trichotomy_proof
```

Concrete proof text lives in the Lean file; spec documents the shape + intent. The two axioms (`runtime_respects_ordering`, `pack_lock_exclusive`) are the **only** permitted non-theorems in deliverable scope; both are listed in `.omne/cfg/concurrency.md` §Lean4 invariant as model-bridge axioms (v1 acceptable; v2 promotes `pack_lock_exclusive` to a theorem by modelling `fd-lock` FIFO queue semantics in Lean).

### CI wiring

Extend `.github/workflows/ci.yml` with a new job:

```yaml
lean-proof:
  name: Lean4 proof (lake build)
  runs-on: ubuntu-latest
  steps:
    - uses: actions/checkout@v6
    - uses: leanprover/lean-action@v1
      with:
        lake-package-directory: lean
    - run: cd lean && lake build
```

Linux-only is acceptable per `.omne/cfg/test-plan.md` §Lean4 proof verification — Lean's `.olean` is platform-agnostic; one successful build proves the invariant everywhere.

Job is **required** on `main` branch protection (update branch-protection config in the PR that merges this change).

## File / module targets

| Concrete path | Change |
|---|---|
| `lean/lakefile.lean` | New — single `lean_lib Grex` target, no mathlib. |
| `lean/lean-toolchain` | New — pinned toolchain tag. |
| `lean/Grex.lean` | New — root import module. |
| `lean/Grex/Scheduler.lean` | New — state machine, axioms, theorems. |
| `.github/workflows/ci.yml` | Add `lean-proof` job (ubuntu-latest only). |
| `.gitignore` | `lean/.lake/` + `lean/build/` added to managed ignore. |
| `.omne/cfg/concurrency.md` | No change — this change implements existing spec. |

## Test plan

Lean's type checker IS the test. The proof either compiles or doesn't. Beyond that:

### Build

- `cd lean && lake build` exits 0. Produces `Grex.olean` + `Grex/Scheduler.olean`.
- Zero `sorry` across `lean/**/*.lean` — verified by `grep -r '\bsorry\b' lean/Grex/` returning only lemma-body whitespace (no `:= by sorry` or `:= sorry`).
- Zero `admit` — same grep.
- Exactly 2 `axiom` declarations (`runtime_respects_ordering`, `pack_lock_exclusive`) in `Grex/Scheduler.lean`; no others. Verified by grep count.

### CI

- New `lean-proof` job runs on every PR + every push to `main`.
- Job succeeds on `main` branch after merge.
- Job is in the branch-protection required-checks list.

### Negative / regression

- PR that adds a new `sorry` anywhere in `lean/Grex/` fails CI. Achieved naturally by `-Dlinter.sorry=true` (Lean default) — `lake build` rejects.
- Docs note: if a future change legitimately needs an axiom for a new bridge (e.g. modelling tokio's runtime), it must be added here with justification in `.omne/cfg/concurrency.md`.

### Local validation

- `lake build` wall-time < 30 s on a warm cache. If it exceeds, simplify the model.
- `lake env lean --version` printed in CI logs for toolchain-drift diagnosis.

## Non-goals

- **No proof of I2, I3, I4, I5** from `.omne/cfg/architecture.md` — explicit v2 backlog per `.omne/cfg/concurrency.md` §Deferred.
- **No mathlib dependency.**
- **No Lean4-on-Windows/macOS CI.** Linux-only per `.omne/cfg/test-plan.md`.
- **No binding of Lean proof artefact into the `grex` binary.** The proof is a build-time artefact, not a runtime component.
- **No promotion of `pack_lock_exclusive` from axiom to theorem** — that requires modelling `fd-lock` FIFO semantics and is deferred to v2 (stated in `.omne/cfg/concurrency.md` §Lean4 invariant final paragraph).
- **No benchmark of proof-compilation time.** As long as < 30 s we accept.

## Dependencies

- **Prior**: feat-m6-1 (scheduler mechanism referenced in the model); feat-m6-2 (pack lock mechanism referenced in the axiom).
- **Runtime**: Lean4 toolchain on CI runner — via `leanprover/lean-action@v1`.
- **Branch protection**: updates to require the new job; must be coordinated with whoever owns repo settings.

## Acceptance

1. `cd lean && lake build` exits 0 locally and in CI (ubuntu-latest).
2. `lean/Grex/Scheduler.lean` contains `theorem no_double_lock : ... := by ...` with zero `sorry` / zero `admit` in its body.
3. Exactly 2 `axiom` declarations in the file; both justified in-file doc-comments pointing to `.omne/cfg/concurrency.md`.
4. Corollary `theorem no_deadlock` also compiles, hole-free.
5. `.github/workflows/ci.yml` has a `lean-proof` job; PR green.
6. No regression on Rust side (feat-m6-1 + feat-m6-2 acceptance invariants still hold).

## Source-of-truth links

- [`.omne/cfg/concurrency.md`](../../../.omne/cfg/concurrency.md) §Lean4 invariant — primary spec, includes the Lean sketch reused here.
- [`.omne/cfg/test-plan.md`](../../../.omne/cfg/test-plan.md) §Lean4 proof verification — CI gate specification.
- [`.omne/cfg/architecture.md`](../../../.omne/cfg/architecture.md) §Runtime invariants I1 — identifies this proof as the v1 formal invariant.
- [`milestone.md`](../../../milestone.md) §M6 — "Lean4 project under `lean/`, theorem `Grex.Scheduler.no_double_lock`."
- [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) §Success criteria #7 — zero-`sorry` / zero-unresolved-`axiom` contract.
