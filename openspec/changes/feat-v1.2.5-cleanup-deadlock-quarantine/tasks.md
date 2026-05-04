---
slug: feat-v1.2.5-tasks
type: spec
status: active
last_updated: 2026-05-02
---

## Stage 0 — branch + dirs (DONE)
- [x] Cut feat-v1.2.5 from main @ 2136bce
- [x] mkdir openspec/changes/feat-v1.2.5-cleanup-deadlock-quarantine

## Stage 1 — Lean theorems (rule 8 gate, MUST land before Rust)
- [ ] Extend `proof/Grex/Walker.lean` with theorem `partial_clone_cleanup_idempotent`
  - Add `Phase3CleanupInvariant` inductive predicate (4 cases: Recursed, Skipped, Cancelled, Failed)
  - State: for any non-Recursed outcome, post-state of `dest` equals pre-state
  - Prove via case-split on outcome variant (model `remove_dir_all` as pure `DiskState.unlink`)
- [ ] Extend `proof/Grex/Scheduler.lean` with theorem `pool_deadlock_guard_terminates`
  - Add `LockKind` enum (perMetaSync | semaphore | packLock | repoBackend | manifestLock | poolInstall)
  - Add `rank` fn assigning strict total order
  - Add `acyclicAcquisition` predicate over `Trace = List LockKind`
  - State: acyclic acquisition trace → no Coffman cycle → no deadlock
  - Prove by induction on trace length; bridge axiom only if pure model insufficient
- [ ] Lake build green; 0 sorry; 0 admit
- [ ] `#print axioms partial_clone_cleanup_idempotent` shows `[propext]` only
- [ ] `#print axioms pool_deadlock_guard_terminates` shows `[propext]` only OR `[propext, pool_install_does_not_re_enter_with_pack_lock]` (1 new bridge axiom max — justify in `Bridge.lean` if added; current count Bridge=10 → ≤11)
- [ ] Verify existing theorems `sync_meta_no_cycle_infinite_clone` (v1.2.2/3) and `cancellation_terminates_promptly` (v1.2.4) still green (theorem extensions must not break prior proofs)
- [ ] Quarantine GC + restore: document Rule 8 "simple" exemption inline in this file (no Lean obligation — pure filesystem ops under existing per-meta lock; no new concurrency invariant)

## Stage 2 — Rust implementation (parallel workers, after Stage 1 green)

### 2a — A2 partial-clone cleanup (walker.rs)
- [ ] In `phase3_handle_child` (`crates/grex-core/src/tree/walker.rs`): snapshot `dest_existed_before = dest.exists()` BEFORE any clone work
- [ ] On `Failed(CycleDetected)` return path: if `!dest_existed_before && dest.exists()`, call `cleanup_partial_clone(&dest)` (best-effort, log + continue on failure)
- [ ] Add helper fn `cleanup_partial_clone(dest: &Path)` — wraps `std::fs::remove_dir_all` + `tracing::warn!` on failure, returns `()`
- [ ] Verify cleanup does NOT fire on `Cancelled` outcome (EARLY-OUT case never started clone work)
- [ ] Verify cleanup does NOT fire when `dest_existed_before == true` (do not delete legitimate prior content from a `git_fetch` path)

### 2b — A3 pool deadlock guard (walker.rs + pack_lock.rs + scheduler.rs)
- [ ] Add `PoolInstallDepthGuard` RAII type in `crates/grex-core/src/scheduler.rs` (debug-only via `#[cfg(debug_assertions)]`); enter increments `POOL_INSTALL_DEPTH` thread-local, drop decrements
- [ ] Add `HELD_PACK_LOCKS: RefCell<Vec<PathBuf>>` thread-local in `pack_lock.rs` (debug-only)
- [ ] In `PackLock::acquire` (or its async analog): debug-only push `path.to_owned()` into `HELD_PACK_LOCKS`
- [ ] In `PackLock::Drop`: debug-only remove the path from `HELD_PACK_LOCKS`
- [ ] In `phase3_recurse` (`walker.rs`): create `_depth_guard = PoolInstallDepthGuard::enter()` BEFORE `pool.install`; inside the closure, debug-only assert that `HELD_PACK_LOCKS.is_empty() || POOL_INSTALL_DEPTH == 1` (panic message includes the held lock paths for diagnostics)
- [ ] Verify release-build assertion compiles out via `cargo build --release` + disassembly spot-check (or rely on `#[cfg(debug_assertions)]` gating)

### 2c — Quarantine GC + restore + retention (parallel workers, non-conflicting per discipline 14)
- [ ] In `crates/grex-core/src/tree/quarantine.rs`: add `prune_quarantine(meta_dir: &Path, retain_days: u32) -> Result<PruneReport>` fn (per design.md pseudocode); skip non-existent trash root, skip malformed timestamps with warn, best-effort per-entry remove
- [ ] In `quarantine.rs`: add `restore_quarantine(meta_dir, ts, basename: Option<&str>, force: bool) -> Result<RestoreReport>` fn; refuse on existing dest unless `--force`; refuse on ambiguous restore (multi-entry under `<ts>/`); cross-device fallback (copy then unlink); append `QuarantineRestored` event
- [ ] In `quarantine.rs`: add `parse_iso8601_quarantine(name: &str) -> Option<SystemTime>` helper (parses the `:`-replaced-with-`-` ISO8601 layout per quarantine.md §"On-disk layout")
- [ ] In `quarantine.rs`: add `RetentionConfig`, `PruneReport`, `RestoreReport` structs
- [ ] In `quarantine.rs`: add `QuarantineError::SnapshotNotFound { ts: String }`, `AmbiguousRestore { count: usize }`, `DestExists { dest: PathBuf }` variants
- [ ] In `crates/grex-core/src/manifest/event.rs`: add `Event::QuarantineRestored { ts, basename, dest }` and `Event::QuarantineGCSwept { meta, pruned_count, retained_count }` variants (additive per JSONL forward-compat policy)
- [ ] In `crates/grex-core/src/doctor/mod.rs` (or doctor command dispatch): wire `--prune-quarantine [--retain-days N]` and `--restore-quarantine <ts> [<basename>] [--force]` subcommand handlers
- [ ] In `crates/grex/src/cli.rs` (clap defs): add `--retain-days N` flag on `grex sync`; `--prune-quarantine`, `--restore-quarantine <ts> [<basename>]`, `--force` on `grex doctor`
- [ ] In `crates/grex-core/src/sync.rs`: thread `--retain-days` through to `sync_meta_inner`; call `prune_quarantine(meta_dir, days)` (best-effort, ignore err) at start of each meta sync when set

### 2d — New tests
- [ ] T-A2: `partial_clone_cleanup_after_cancellation` in walker.rs `#[cfg(test)] mod tests`. Topology: 4-node cycle that cancels a sibling mid-clone (use a slow-clone test backend with a signal). Assert: after `Err(CycleDetected)`, `dest` for the cancelled sibling does NOT exist on disk.
- [ ] T-A3: `pool_deadlock_guard_panics_on_violation` in walker.rs (gated `#[cfg(debug_assertions)]`). Construct a contrived nested `pool.install` while holding a `PackLock`; assert the debug assertion fires (use `std::panic::catch_unwind`).
- [ ] T-Q1: `prune_quarantine_removes_old_entries` in quarantine.rs `#[cfg(test)] mod tests`. Populate trash dirs at ages 0d / 30d / 100d / 200d; run `prune_quarantine(meta, 90)`; assert 100d + 200d gone, 0d + 30d intact.
- [ ] T-Q2: `restore_quarantine_replaces_dest` in quarantine.rs. Quarantine a child via existing flow; run `restore_quarantine(meta, ts, Some(basename), false)`; assert dest restored byte-identical, `QuarantineRestored` event in events.jsonl.
- [ ] T-Q3: `restore_refuses_existing_dest_without_force` in quarantine.rs. Pre-populate dest; assert restore returns `Err(DestExists)`. Then re-run with `force=true`; assert restore succeeds.
- [ ] T-Q4: `restore_ambiguous_without_basename` in quarantine.rs. Quarantine two children under same `<ts>/`; assert `restore_quarantine(meta, ts, None, false)` returns `Err(AmbiguousRestore { count: 2 })`.
- [ ] T-R1: `sync_retain_days_triggers_gc` in `crates/grex-core/tests/sync_retention.rs` (new file). Pre-populate trash with 100d-old entry; run `grex sync --retain-days 90` programmatically; assert old entry gone post-sync.

### 2e — CI gate
- [ ] Extend `.github/workflows/ci.yml` axiom-stability smoke check (added v1.2.4) to also assert `#print axioms partial_clone_cleanup_idempotent` and `#print axioms pool_deadlock_guard_terminates`. Update the grep target string to accept the v1.2.5 expected sets (`[propext]` for both, optionally `[propext, pool_install_does_not_re_enter_with_pack_lock]` for the scheduler one if bridge axiom is added).

### 2f — Version bumps + man pages
- [ ] Workspace `Cargo.toml` version 1.2.4 → 1.2.5 + 3 path-deps (grex-core, grex-mcp, grex-plugins-builtin)
- [ ] `crates/xtask/Cargo.toml` grex-cli path-dep 1.2.4 → 1.2.5
- [ ] `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION` 1.2.4 → 1.2.5
- [ ] Regenerate man pages: `cargo xtask gen-man` — expect new flags in `grex.1` (`--retain-days` on sync) and `grex-doctor.1` (`--prune-quarantine`, `--restore-quarantine`, `--force`)

### 2g — CHANGELOG + history
- [ ] CHANGELOG.md: promote v1.2.4 entry to dated SHIPPED 2026-05-02 if not already; add v1.2.5 entry (Added: doctor subcommands + sync flag + event variants; Changed: cyclic-input cleanup behavior; Internal: pool deadlock guard; Tests: 6 new)
- [ ] `.omne/history.md`: append v1.2.5 draft section (separate SSOT repo per Rule 7 — ships through grex-inst)

### 2h — v1.3.0 readiness regression (maintainer directive)
- [ ] Verify existing `e2e_v1_3_0_readiness_smoke` (added v1.2.4) in `crates/grex/tests/sync_e2e.rs` still passes — meta-pack + 1 sub-pack acyclic sync returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` in any error path
- [ ] Verify `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs (manual or scripted) post-impl, before PR push
- [ ] No new e2e test required for v1.3.0 readiness in v1.2.5 — the v1.2.4 smoke covers the constraint; v1.2.5 only inherits

## Stage 3 — local gates (full pre-push sequence per cfg/workflow.md Phase 2 Step 7)
- [ ] `cargo fmt --all -- --check` exit 0
- [ ] `cargo build --workspace` green
- [ ] `cargo test --workspace` (excluding `dispatch_parallel.rs` per pre-existing UAC issue)
- [ ] Verify NO regression: 380+ existing lib tests pass; v1.2.4 `cancellation_aborts_siblings` pass; v1.2.3 `e2e_cycle_aborts` pass; v1.2.2 `same_repo_two_refs_no_cycle` pass; cycle_self_loop_aborts/three_node_aborts/four_node_aborts/nested_prefix_aborts/diamond all pass; v1.2.4 `e2e_v1_3_0_readiness_smoke` pass
- [ ] `cargo doc --no-deps --workspace -D warnings` clean
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cd proof && lake build` green
- [ ] axiom counts: Bridge=10 (or 11 if 1 new A3 bridge axiom added; goal 10), Types=4, Other=0
- [ ] Release-build sanity: `cargo build --release --workspace` exit 0 (verifies `#[cfg(debug_assertions)]`-gated code compiles out cleanly)

## Stage 4 — review pass (parallel + Codex per cfg/workflow.md Phase 3)
- [ ] 4-6 parallel subagent reviewers: Lean-Rust correspondence (A2 + A3) / cleanup correctness (A2 best-effort + idempotence) / deadlock guard correctness (thread-local visibility, debug-only gating) / quarantine GC + restore (timestamp parsing, cross-device fallback, ambiguity refuse) / SemVer + BC (additive flags + new event variants) / idiomatic Rust (RAII guard, `cfg(debug_assertions)`, error variants)
- [ ] Apply review fix-ups (separate workers — never the original writers per discipline 14)
- [ ] Codex rescue second pass; skip if no return
- [ ] Re-run gates after fix-ups

## Stage 5 — commit + PR + merge (per cfg/workflow.md Phase 4)
- [ ] Conventional Commit (NO Co-Authored-By per discipline 13)
- [ ] git push origin feat-v1.2.5
- [ ] gh pr create --base main --head feat-v1.2.5
- [ ] Watch CI: `gh pr checks <num> --watch --interval 30`
- [ ] After CI green: `gh pr merge <num> --squash --delete-branch`
- [ ] Local: git checkout main; git pull

## Stage 6 — ship (cargo publish + tag)
- [ ] cargo publish topo: grex-core → grex-mcp + grex-plugins-builtin (parallel) → grex-cli
- [ ] git tag -a v1.2.5 -m "v1.2.5 — partial-clone cleanup + pool deadlock guard + quarantine GC/restore (PATCH)" <merge-commit>
- [ ] git push origin v1.2.5

## Stage 7 — wrap-up (per cfg/workflow.md Phase 5)
- [ ] Append `## Endpoint (2026-05-XX, main — v1.2.5 SHIPPED)` to progress.md
- [ ] Update top "Where we are" block
- [ ] Promote draft entry in `.omne/history.md` to SHIPPED with date + commit SHA (separate SSOT repo per Rule 7)
- [ ] Commit progress.md (grex) + history.md (SSOT)
- [ ] Carry-forward list to v1.2.6: TreeError variant split, cap-std snapshot hardening, stale `manifest.md` doc cleanup, working-tree drift root cause investigation
- [ ] Carry-forward list to v1.3.0: `--workspace` → `--pack` CLI rename, behavior contract freeze, MINOR cut

## Out of scope (defer to v1.2.6+)
- TreeError variant split (separate `CycleDetected` from `DepthCapExceeded` from `UntrackedGitRepos`) → v1.2.6
- cap-std snapshot hardening (replace ambient `std::fs::copy` in `quarantine.rs` with cap-std bounded variants) → v1.2.6
- Stale `.omne/manifest.md` doc cleanup → v1.2.6
- Working-tree drift root cause investigation (statusline-probe.txt, `crates/grex/.grex/` runtime artifact) → v1.2.6 spike
- `--workspace` → `--pack` CLI rename → v1.3.0
- v1.3.0 contract freeze + MINOR cut → v1.3.0
- SSOT v2 (owners.yaml, topic-reorg cfg/, lib/cfg dedup, history.md aggregator) → SSOT roadmap, separate repo
