# `lean/` — Lean4 mechanised proof of grex runtime invariant I1

This directory contains the Lean4 formalisation of grex's core concurrency
invariant **I1 (no double lock)**: no two distinct concurrent tasks
simultaneously hold the per-pack lock on the same pack path.

See [`.omne/cfg/concurrency.md`](../.omne/cfg/concurrency.md) §Lean4
invariant for the primary specification.

## Layout

```
lean/
├── lean-toolchain        # pinned to leanprover/lean4:v4.16.0
├── lakefile.lean         # single `lean_lib Grex` target, no external deps
├── Grex.lean             # root module — re-exports Grex.Scheduler
└── Grex/
    └── Scheduler.lean    # state machine, axioms, theorems
```

No `mathlib` — keeps CI wall-time minimal and avoids toolchain drift.

## Build

```bash
cd lean
lake build
```

Expected: exit 0; compiles `Grex.olean` + `Grex/Scheduler.olean`. Wall time
is a few seconds on a warm cache.

CI runs the same command on `ubuntu-latest` via the `lean-proof` job in
[`.github/workflows/ci.yml`](../.github/workflows/ci.yml). Linux-only is
acceptable because `.olean` is platform-agnostic: one successful build
proves the invariant on every OS.

## Axiom justification

`Grex/Scheduler.lean` declares exactly **two** `axiom`s — both are
model-bridge axioms, unavoidable at v1 because Lean cannot reason directly
about the Rust runtime:

1. **`runtime_respects_ordering`** — asserts that the Rust scheduler
   (`Scheduler::run` in `crates/grex-core`) acquires every lock in the
   fixed 5-tier order (`workspace-sync → semaphore → pack-lock →
   repo-backend → manifest`). Promotion to theorem requires modelling
   tokio's await semantics in Lean. Deferred to v2.

2. **`pack_lock_exclusive`** — asserts that two distinct tasks holding
   `Lock.pack p` for the same path `p` have non-overlapping time windows.
   Encodes the FIFO mutual-exclusion semantics of
   `fd_lock::RwLock::write` as used by `PackLock::acquire`. Promotion to
   theorem requires modelling `fd-lock`'s kernel-level FIFO queue in
   Lean. Deferred to v2.

Both axioms are documented inline with `/--` doc-comments linking back to
the corresponding mechanism in the Rust implementation.

## Theorems

* **`no_double_lock`** — direct application of `pack_lock_exclusive`.
  This is invariant I1.
* **`no_deadlock`** — corollary: because `tier : Lock → Nat` is a strict
  total order, any two held locks are comparable; combined with
  `legalAcquire` this rules out acquisition cycles.

Both theorems compile with zero `sorry` / zero `admit`.

## Adding new axioms

Do NOT add axioms without updating both:

1. This README (axiom justification section).
2. [`.omne/cfg/concurrency.md`](../.omne/cfg/concurrency.md) §Lean4
   invariant — source-of-truth spec.

The CI job enforces `lake build` success; it does not count axioms. Axiom
hygiene is maintained by code review.
