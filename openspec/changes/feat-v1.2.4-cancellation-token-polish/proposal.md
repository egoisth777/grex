---
slug: feat-v1.2.4-cancellation-token-polish
type: spec
status: active
last_updated: 2026-05-02
---

# feat-v1.2.4 — cancellation token + polish bundle

**Status**: draft
**Milestone**: v1.2.4 (PATCH per maintainer Rule 6 preference for additive shipping; cancellation behavior change is additive — cyclic input stops earlier, no regression for acyclic flow)
**Depends on**: v1.2.3 (SHIPPED 2026-05-02 — main @ 6f996fb post-purge, tag v1.2.3, all 4 crates @ 1.2.3 live on crates.io)
**Branch**: feat-v1.2.4

## Why now

Reviewer findings carried over from the v1.2.2 and v1.2.3 cycles surfaced one architectural gap and a cluster of low-LOC polish items that are cheap to ship together:

- Rayon `par_iter` in walker Phase 3 does NOT cancel sibling tasks when one returns `Err(CycleDetected)`. Siblings keep cloning to completion, wasting work and delaying error surfacing. (R#4 v1.2.2 HIGH; R#2 v1.2.2 MED.)
- Six polish items from the m7 scope carry-forward (rename, dead-code drop, doc cleanup) are low-risk and low-LOC; bundling them now reduces review noise on future PRs.
- Test coverage gaps from the v1.2.3 review pass (R#3 v1.2.3 MAJOR + MINOR) — diamond destination spot-check and a randomised cycle generator — were deferred and need to land.
- The v1.2.2 Lean theorem `Grex.Walker.sync_meta_no_cycle_infinite_clone` has no CI smoke check on its `#print axioms` output, so silent axiom drift could go unnoticed across PRs (R#4 v1.2.3 LOW).

Each item is small and independent; bundling them into a single PATCH release is preferable to four separate point releases or to deferring them into the v1.3 cycle, where they would race feature work.

## Scope

- **A1 cancellation token** (architecture) — add a shared atomic flag (`AtomicBool` wrapped in `Arc`) propagated through the Phase 3 closure. `phase3_handle_child` checks the flag at entry and short-circuits with the original `Err(CycleDetected)` before issuing any clone/copy work. The first sibling to detect a cycle sets the flag; in-flight siblings abort on their next check. Concurrent invariant → Lean theorem `cancellation_terminates_promptly` required (Rule 8).
- **Polish (6 items)** — rename `visited` → `ancestors` in `walker.rs` and `graph_build.rs` (clearer intent for the cycle-detection set); doc comment cleanup on `sync_meta`; delete unused `PackLock::acquire` synchronous variant; delete unused `Scheduler::permits()`; inline `DEFAULT_MANAGED_GITIGNORE_PATTERNS`; rename `OwnCycleGuard` → `VisitedInsertGuard`. No Lean change.
- **Tests (3)** — `cancellation_aborts_siblings` (mandatory pair for A1; deterministic, asserts in-flight siblings short-circuit on the shared flag, not flaky); T1 destination spot-check (verify the diamond's shared C node is actually visited via both arms, not skipped); proptest cycle generator (random DAG vs random graph with one injected back-edge; the walker must accept all DAGs and reject all back-edge graphs).
- **CI/doc** — `#print axioms` smoke check in `.github/workflows/ci.yml` for `Grex.Walker.sync_meta_no_cycle_infinite_clone`; expected output is `[propext]` only. Locks the proof foundation against silent axiom drift.

## Out of scope

The following items are deliberately deferred:

- A2 partial-clone cleanup (delete partial clones left behind by a cancelled sibling) → v1.2.5; builds on the A1 cancellation token wiring landing first.
- A3 `pool.install` deadlock guard → v1.2.6 or later; edge case only, no observed reproducer.
- T3 chain-index strengthen → folded into the proptest coverage above (the property already asserts chain shape on every back-edge sample).
- v1.2.0 follow-ups (`grex doctor --prune-quarantine` GC, `--restore-quarantine`, retention policy, `TreeError::QuarantineFailed` variant, etc.) → v1.3 candidates; orthogonal to cancellation.
- Working-tree drift root cause investigation → separate spike; not blocking v1.2.4.

No public API change, no rename of exported items, no architecture change beyond the additive cancellation flag. v1.2.4 is one architectural addition + polish + tests + one CI gate.

## Acceptance bar

1. Lean4 theorem `cancellation_terminates_promptly` extended in `proof/Grex/Walker.lean`. `lake build` green; zero `sorry`, zero `admit`. Axiom counts: Bridge ≤ 10 (one new axiom permitted in `Bridge.lean` for atomic visibility, justified inline if added; otherwise 9 unchanged), Types=4, Other=0.
   - Lean theorem `Grex.Walker.cancellation_terminates_promptly` proved with signature `∀ (cancelled : Bool) (t : ManifestTree) (visited : List String), ...` matching the design's mutual-recursion model extension. lake build green; 0 sorry/admit; axiom counts per AC#1 above.
2. `cancellation_aborts_siblings` regression test asserts that on the first cycle detection, in-flight Phase 3 siblings short-circuit on their next flag check (deterministic, not flaky).
3. T1 destination spot-check passes: a diamond manifest (root → A, root → B, A → C, B → C) leaves C present on disk reachable via both arms.
4. Proptest cycle generator passes: random DAG inputs all succeed; random graphs with one injected back-edge all surface `Err(CycleDetected)`. Generator converges within the default proptest budget.
5. The 380+ existing unit tests and the `e2e_cycle_aborts` integration test continue to pass.
6. Local gates clean: `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cd proof && lake build`, axiom-policy check.
7. CI smoke check verifies `#print axioms` output for both theorems equals `[propext]` only OR `[propext, <new_bridge_axiom_name>]` if a single bridge axiom is justified. Drift to any other axiom set fails CI.
8. Workspace version bumped 1.2.3 → 1.2.4 in workspace root `Cargo.toml`, `crates/xtask/Cargo.toml` path-dep pin, and `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION`.
9. CHANGELOG `[1.2.4]` entry + SSOT `.omne/history.md` v1.2.4 entry (separate SSOT repo per Rule 7).

## v1.3.0 readiness constraint (maintainer directive)

v1.2.x cycle is stabilization on path to v1.3.0 MINOR ("full-blown grex with sub-pack under meta-pack feature complete + CLI rename to `--pack`"). Per maintainer constraint:

- **Sub-pack under meta-pack** flow MUST remain functional after each v1.2.x PATCH ship.
- **Basic action commands** (`grex sync`, `grex ls`, `grex doctor`, `grex migrate-lockfile`) MUST NOT be broken by any v1.2.x ship.
- Each v1.2.x PR includes an end-to-end smoke test verifying meta-pack + 1 sub-pack acyclic sync: returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` variant in any error path.

v1.2.4 is additive: cancellation only fires on cyclic input (broken manifests, no real-world acyclic regression). Polish + dead-code removals verified zero-call-sites. No CLI API change.

v1.2.x → v1.3.0 roadmap (planning only, not locked):
- v1.2.4: cancellation + polish + tests + axiom CI (this PR)
- v1.2.5: A2 partial-clone cleanup + A3 pool deadlock guard + quarantine GC/restore + retention policy
- v1.2.6: TreeError variant split + cap-std hardening + stale doc + drift root cause
- v1.3.0: `--workspace` → `--pack` CLI rename + behavior contract freeze + MINOR cut

## SemVer

PATCH (1.2.3 → 1.2.4) per maintainer Rule 6. The cancellation token is observable behavior (cyclic input now aborts earlier; no observable change for acyclic input), and the polish items are either internal renames or dead-code drops below the `pub(crate)` boundary. No public API surface changes; no manifest/lockfile/binary compatibility break.

## Process gates (per cfg/workflow.md)

Order of operations is fixed:

1. Phase 1 — OpenSpec (this proposal).
2. Phase 2 — Lean theorem `cancellation_terminates_promptly` written and `lake build` green BEFORE any Rust impl change (Rule 8). Rust impl second.
3. Phase 3 — review (parallel reviewers + Codex pass).
4. Phase 4 — PR + merge.
5. Phase 5 — wrap-up (CHANGELOG, SSOT history entry, crates.io publish, tag).

A code change that lands before the Lean proof is a process violation per `.omne/schemas/rules.md` Rule 8. Per discipline 13, commits MUST NOT carry a `Co-Authored-By` trailer.
