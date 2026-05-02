---
slug: feat-v1.2.4-tasks
type: spec
status: active
last_updated: 2026-05-02
---

## Stage 0 — branch + dirs (DONE)
- [x] Cut feat-v1.2.4 from main
- [x] mkdir openspec/changes/feat-v1.2.4-cancellation-token-polish

## Stage 1 — Lean theorem (rule 8 gate, MUST land before Rust)
- [ ] Extend `proof/Grex/Walker.lean` with theorem `cancellation_terminates_promptly`
  - Add `cancelled : Bool` parameter to `sync_meta_inner_model` (or sibling def matching the syncMetaChildren mutual)
  - State: cyclic input + cancellation flag set during recurse → walker terminates with bound = cancellation-depth
  - Prove via mutual structural recursion (existing pattern from v1.2.2/v1.2.3 work)
- [ ] Lake build green; 0 sorry; 0 admit
- [ ] `#print axioms cancellation_terminates_promptly` shows `[propext]` only (or 1 new bridge axiom max — justify in `Bridge.lean` if added; current count Bridge=9 → ≤10)
- [ ] Verify existing theorem `sync_meta_no_cycle_infinite_clone` still green (theorem extension must not break prior proof)

## Stage 2 — Rust implementation (parallel workers, after Stage 1 green)

### 2a — A1 cancellation token (walker.rs)
- [ ] Add `cancelled: Arc<AtomicBool>` parameter to `phase3_recurse` and `phase3_handle_child` signatures
- [ ] In `phase3_recurse`, create token at top: `let cancelled = Arc::new(AtomicBool::new(false));`
- [ ] In each rayon child closure: early-out if `cancelled.load(Relaxed)` → return `Phase3ChildOutcome::Cancelled`
- [ ] In `phase3_handle_child`: when cycle detected, `cancelled.store(true, Relaxed)` BEFORE returning `Phase3ChildOutcome::Failed(CycleDetected)`
- [ ] Aggregate `Cancelled` outcomes: skip merge into report (no valid sub-report data); count separately if needed for diagnostics

### 2b — Polish bundle (parallel workers, non-conflicting per discipline 14)
- [ ] P1: Rename `visited` → `ancestors` in walker.rs + graph_build.rs (signatures + call sites + comments)
- [ ] P2: Doc comment cleanup on public `sync_meta` — strip impl-detail leakage (Phase 3, A.1, rayon vocabulary)
- [ ] P3: Delete unused `PackLock::acquire` sync variant (verify zero call sites first via grep)
- [ ] P4: Delete unused `Scheduler::permits()` (verify zero call sites first)
- [ ] P5: Inline `DEFAULT_MANAGED_GITIGNORE_PATTERNS` constant at single use site
- [ ] P6: Rename `OwnCycleGuard` → `VisitedInsertGuard`

### 2c — New tests
- [ ] T-cancel: cancellation behavior test in walker.rs `#[cfg(test)] mod tests`. Topology: 4-node cycle. Assert: at least one sibling returns `Cancelled` outcome OR `metas_visited` count is strictly less than full-tree count. Deterministic (no race via large fanout + small clone latency).
- [ ] T1-spot-check: extend existing `cycle_diamond_shared_descendant_no_cycle` to assert C visited via BOTH arms (track destinations in fixture).
- [ ] T-proptest: add `cycle_proptest` in `crates/grex-core/tests/property.rs` (or new test file). Generator: random DAG (acyclic) ∪ random graph with injected back-edge (cyclic). Assert detector dichotomy. Use seed-stable proptest config (10-100 cases).

### 2d — CI gate
- [ ] Add axiom-stability smoke check step to `.github/workflows/ci.yml` Lean4 proof gate (lines ~218-265). Asserts `#print axioms` for both `sync_meta_no_cycle_infinite_clone` and `cancellation_terminates_promptly` shows expected axioms only.

### 2e — Version bumps + man pages
- [ ] Workspace `Cargo.toml` version 1.2.3 → 1.2.4 + 3 path-deps (grex-core, grex-mcp, grex-plugins-builtin)
- [ ] `crates/xtask/Cargo.toml` grex-cli path-dep 1.2.3 → 1.2.4
- [ ] `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION` 1.2.3 → 1.2.4
- [ ] Regenerate man pages: `cargo xtask gen-man`

### 2f — CHANGELOG + history
- [ ] CHANGELOG.md: promote v1.2.3 entry to dated SHIPPED 2026-05-02; add v1.2.4 entry (Added: cancellation, polish renames, proptest, CI axiom check; Changed: cyclic-input behavior — siblings stop on first cycle)
- [ ] `.omne/cfg/history.md`: append v1.2.4 draft section

### 2g — v1.3.0 readiness smoke test (maintainer directive)
- [ ] Add `e2e_v1_3_0_readiness_smoke` in `crates/grex/tests/sync_e2e.rs`
  - Topology: meta-pack with 1 sub-pack child (acyclic, real git backend or in-mem fixture matching existing e2e tests)
  - Assert: returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` variant in any error path
  - This guards the "sub-pack under meta-pack flow must work" constraint per maintainer directive (v1.3.0 readiness)
- [ ] Verify `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs (manual or scripted) post-impl, before PR push

## Stage 3 — local gates (full pre-push sequence per cfg/workflow.md Phase 2 Step 7)
- [ ] `cargo fmt --all -- --check` exit 0
- [ ] `cargo build --workspace` green
- [ ] `cargo test --workspace` (excluding `dispatch_parallel.rs` per pre-existing UAC issue)
- [ ] Verify NO regression: 380+ existing lib tests pass; `e2e_cycle_aborts` pass; `same_repo_two_refs_no_cycle` pass; cycle_self_loop_aborts/three_node_aborts/four_node_aborts/nested_prefix_aborts/diamond all pass
- [ ] `cargo doc --no-deps --workspace -D warnings` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cd proof && lake build` green
- [ ] axiom counts: Bridge=9 (or 10 if 1 new bridge added), Types=4, Other=0

## Stage 4 — review pass (parallel + Codex per cfg/workflow.md Phase 3)
- [ ] 4-6 parallel subagent reviewers: Lean-Rust correspondence / correctness / cancellation determinism / test coverage / SemVer + BC / idiomatic Rust + atomic ordering
- [ ] Apply review fix-ups (separate workers — never the original writers)
- [ ] Codex rescue second pass; skip if no return
- [ ] Re-run gates after fix-ups

## Stage 5 — commit + PR + merge (per cfg/workflow.md Phase 4)
- [ ] Conventional Commit (NO Co-Authored-By per discipline 13)
- [ ] git push origin feat-v1.2.4
- [ ] gh pr create --base main --head feat-v1.2.4
- [ ] Watch CI: `gh pr checks <num> --watch --interval 30`
- [ ] After CI green: `gh pr merge <num> --squash --delete-branch`
- [ ] Local: git checkout main; git pull

## Stage 6 — ship (cargo publish + tag)
- [ ] cargo publish topo: grex-core → grex-mcp + grex-plugins-builtin (parallel) → grex-cli
- [ ] git tag -a v1.2.4 -m "v1.2.4 — cancellation token + polish (PATCH)" <merge-commit>
- [ ] git push origin v1.2.4

## Stage 7 — wrap-up (per cfg/workflow.md Phase 5)
- [ ] Append `## Endpoint (2026-05-02, main — v1.2.4 SHIPPED)` to progress.md
- [ ] Update top "Where we are" block
- [ ] Promote draft entry in `.omne/cfg/history.md` to SHIPPED with date + commit SHA
- [ ] Commit progress.md (grex) + history.md (SSOT)
- [ ] Carry-forward list to v1.2.5: A2 partial-clone cleanup, A3 pool deadlock guard, T3 chain index strengthen, v1.2.0 follow-ups, drift root cause

## Out of scope (defer to v1.2.5+)
- A2 partial-clone cleanup (builds on A1)
- A3 pool.install deadlock guard (edge-case)
- T3 chain index assertion strengthen (folded into proptest)
- v1.2.0 follow-ups (quarantine GC/restore, retention policy, TreeError variant split, cap-std snapshot hardening, --workspace→--pack rename, stale manifest.md)
- Working-tree drift root cause investigation
