# feat-v1.2.2 — sync_meta cycle detection

**Status**: draft
**Milestone**: v1.2.2 (PATCH; bug fix, no API change)
**Depends on**: v1.2.1 (SHIPPED 2026-04-30 — main @ 2c23c6f, tag v1.2.1, all 4 crates published 2026-05-01)
**Branch**: feat/v1.2.2

## Why now

v1.2.1 split the sync pipeline into a mutating pass (`tree::walker::sync_meta`) followed by a read-only graph build (`tree::graph_build::build_graph`). Cycle detection migrated cleanly into the read pass — `graph_build::handle_child` runs a `pack_identity_for_child` stack check at `crates/grex-core/src/tree/graph_build.rs:174-179`. The mutating pass does not.

Result: a cyclic manifest (a meta whose URL transitively re-references itself) makes `sync_meta` clone-and-fetch the same repo forever in Phase 1 + Phase 3 BEFORE `build_graph` ever runs. The legacy v1.1.x `Walker::walk` carried cycle detection inline (still visible in the legacy method at `walker.rs:180-185`); that mechanism was lost when the responsibilities were separated.

The gap is recorded as a v1.2.2 BLOCKER in `progress.md` "Where we are" (line 34) and in the v1.2.1 endpoint's "Known follow-up gaps" list. The end-to-end test `e2e_cycle_aborts` at `crates/grex/tests/sync_e2e.rs:260` is currently `#[ignore]`-gated with the comment "would clone forever" — it documents the bug rather than guarding against it.

## Scope

Add cycle detection to `sync_meta` only. Mirror the legacy mechanism: thread a `visited: HashSet<String>` through `sync_meta_inner`, push `pack_identity_for_child(child)` at frame entry, pop at exit, return `TreeError::CycleDetected { chain }` on re-entry. No API change. PATCH bump (1.2.1 → 1.2.2).

## Out of scope

The 14 deferred items from `progress.md` "Known v1.2.2+ follow-up gaps" stay deferred. Notably:

- `grex doctor --prune-quarantine` GC verb (v1.3 candidate)
- `grex doctor --restore-quarantine` recovery verb (v1.3 candidate)
- Dedicated `TreeError::QuarantineFailed` variant (MINOR-bump candidate)
- cap-std bounded recursive copy for snapshot read TOCTOU hardening (v1.3 candidate)
- `--workspace → --pack` flag rename (v1.3.0 deprecation, v1.3.1 removal)
- Stale `grex-doc/src/concepts/manifest.md` doc-debt sweep (separate doc PR)
- mdbook doc-debt sweep + rayon parallel scheduler tuning + CLI migrate-lockfile dispatcher (separate v1.2.x slices)
- SSOT side files not yet committed (`force-prune.md`, `toctou.md`, AuditKind/quarantine doc, `snapshot_recursive` axiom migration to `Bridge.lean`)

This change is intentionally narrow: one bug, one fix, one PATCH bump. No bundling.

## Acceptance bar

1. Lean4 theorem `Grex.Walker.sync_meta_no_cycle_infinite_clone` proved in `proof/Grex/Walker.lean`. `lake build` green, zero `sorry`, zero `admit`. **No new axiom.** Precondition `manifest_forest_acyclic`; postcondition `terminates_in_finite_steps`. The bridge axiom count CI gate (`.github/workflows/ci.yml:218-235`) stays at 9 propositional axioms in `Bridge.lean` + 4 model-placeholder axioms in `Types.lean`.
2. `e2e_cycle_aborts` re-enabled (`#[ignore]` removed at `crates/grex/tests/sync_e2e.rs:258`) and passes — a self-referencing meta surfaces `TreeError::CycleDetected` instead of looping forever.
3. Two new unit tests added in the `walker.rs` test module:
   - self-loop A→A — pack with `children[0].url` pointing at its own bare repo
   - 3-node cycle A→B→C→A
   - positive case A@v1, A@v2 (same URL, two distinct refs) succeeds — must NOT trigger cycle detection (identity is `url:<url>@<ref>`, not `url:<url>` alone)
4. Local gates clean: `cargo fmt --all -- --check`, `cargo doc --no-deps -D warnings`, axiom-policy CI step (counts in `Bridge.lean` / `Types.lean`).
5. PR checklist on the impl PR includes those three gates explicitly.

## SemVer

PATCH (1.2.1 → 1.2.2). Bug fix; no public API change. `TreeError::CycleDetected` already exists (`crates/grex-core/src/tree/error.rs:47-50`) and is already produced by `build_graph`; this change extends production to `sync_meta` without altering its shape. No change to `pack.yaml` schema, lockfile schema, `SyncMetaOptions`, `SyncMetaReport`, or any MCP envelope.

## Process gates (per Rule 8)

Order of operations is fixed: Lean theorem first, Rust code second.

1. Extend `proof/Grex/Walker.lean` with the theorem statement + proof.
2. `lake build` green with zero `sorry` / zero `admit` in scope.
3. THEN modify Rust code in `walker.rs` to match.

A code change that lands before the proof is a process violation per `.omne/schemas/rules.md` Rule 8.
