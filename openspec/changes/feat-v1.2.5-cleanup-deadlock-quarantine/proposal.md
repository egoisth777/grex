---
slug: feat-v1.2.5-cleanup-deadlock-quarantine
type: spec
status: active
last_updated: 2026-05-02
---

# feat-v1.2.5 — partial-clone cleanup + pool deadlock guard + quarantine GC/restore

**Status**: draft
**Milestone**: v1.2.5 (PATCH per maintainer Rule 6 — additive on-disk hygiene + concurrency safety net + opt-in operator tooling; no public API removal, no manifest/lockfile schema break)
**Depends on**: v1.2.4 (SHIPPED 2026-05-02 — main @ `2136bce`, tag `v1.2.4`, all 4 crates @ 1.2.4 live on crates.io). A1 cancellation token from v1.2.4 is the precondition for A2 partial-clone cleanup.
**Branch**: feat-v1.2.5

## Why now

Three v1.2.x carry-forwards converged on the same release window. Each is independently small and additive; bundling them into a single PATCH cuts review noise vs. three separate point releases.

- **A2 partial-clone cleanup** — v1.2.4 shipped the rayon cancellation token (siblings observe an `Arc<AtomicBool>` and short-circuit on first `CycleDetected`). The signal halts further recursion, but bytes already written by a cancelled sibling (a partial `git clone` mid-checkout, a freshly-`mkdir_p`'d ancestor segment) stay on disk. Operators see leaked half-clones under `<meta>/<child.path>/` after every cycle abort. The v1.2.4 design doc explicitly defers this to v1.2.5 (proposal §"Out of scope" line 37; design.md §"What v1.2.4 does NOT do"). Closes the v1.2.4 hygiene gap.
- **A3 pool deadlock guard** — `phase3_recurse` builds a fresh `rayon::ThreadPool` per recursion frame (`pool.install` blocks for the lifetime of the inner `sync_meta`). Reviewer R#1 (v1.2.2 carry-forward, MED) flagged that under high parallelism + deep recursion + long per-pack lock holds, a pathological interleaving where worker thread T holds `PackLock(P1)` and is blocked on `pool.install` waiting for thread T' (which is blocked acquiring `PackLock(P1)` from a deeper recursion frame) is theoretically possible. No real-world reproducer to date; the guard is a defence-in-depth invariant pin to make the absence of the cycle a static property of the lock-acquisition order.
- **Quarantine GC + restore + retention policy** — v1.2.1 shipped `--force-prune --quarantine` with snapshot-before-delete semantics. Per `quarantine.md` §Retention: "Retention: indefinite under v1.2.1. The trash bucket grows unboundedly per meta. Operators are responsible for periodic cleanup." Two operator-facing follow-ups deferred at the time:
  - `grex doctor --prune-quarantine` — scan + auto-prune entries older than the retention TTL.
  - `grex doctor --restore-quarantine <ts> [<basename>]` — in-band restore replacing the manual `mv` + `pack.yaml` re-add operator workflow.
  - Retention policy: a `--retain-days N` flag on `grex sync` and a default TTL (90 days) for opt-in auto-pruning. Closes the unbounded-growth foot-gun without changing the v1.2.1 default behavior.

Each item is small (<300 LOC), independent in its own file scope, and cleanly partitions across parallel workers. Together they finish the cycle-detection + quarantine story before v1.2.6 takes on the TreeError variant split / cap-std hardening / drift root cause.

## Scope

- **A2 partial-clone cleanup** (architecture) — when `phase3_handle_child` returns `Phase3ChildOutcome::Cancelled` OR returns `Phase3ChildOutcome::Failed(_)` mid-clone, any partial work staged at `dest` MUST be removed before the closure returns. Cleanup is best-effort: failure to clean is logged via `tracing::warn!` but does NOT mask the original cycle/cancellation error. Concurrency invariant → Lean theorem `partial_clone_cleanup_idempotent` required (Rule 8).
- **A3 pool deadlock guard** (architecture) — pin the lock acquisition order (per-meta-sync → semaphore → pack-lock → repo-backend → manifest-lock per `concurrency.md`) as a static invariant covering the rayon `pool.install` boundary. Specify that no closure inside a `pool.install` may re-enter `pool.install` while holding `PackLock` from an outer frame. Concurrency invariant → Lean theorem `pool_deadlock_guard_terminates` required (Rule 8).
- **Quarantine GC** — new `grex doctor --prune-quarantine [--retain-days N]` subcommand. Scans `<meta>/.grex/trash/<ISO8601>/` directories under each meta in the cwd-meta tree, deletes entries whose timestamp is older than the retention TTL (default 90 days). Best-effort (per-entry failures logged + skipped, do not halt the sweep). Pure filesystem op — Rule 8 "simple" (no concurrency invariant beyond per-meta lock that doctor already takes).
- **Quarantine restore** — new `grex doctor --restore-quarantine <ts> [<basename>]` subcommand. Moves the snapshot at `<meta>/.grex/trash/<ts>/<basename>/` back into place at `<meta>/<basename>/`. Refuses if the dest already exists (operator must `--force` or manually resolve). Records the restore in `events.jsonl` as a new `QuarantineRestored` audit variant. Pure filesystem op — Rule 8 "simple".
- **Retention policy** — `--retain-days N` flag on `grex sync` (NOT just doctor); when set, the doctor GC sweep fires automatically at sync start, BEFORE Phase 1. Default = unset (no auto-prune; v1.2.1 behavior preserved). New `RetentionConfig` struct in `crates/grex-core/src/tree/quarantine.rs`.

## Out of scope

The following items remain deferred to v1.2.6 or later:

- TreeError variant split (separate `CycleDetected` from `DepthCapExceeded` from `UntrackedGitRepos` into a richer enum) → v1.2.6.
- cap-std snapshot hardening (replace ambient `std::fs::copy` calls in `quarantine.rs` with cap-std bounded variants) → v1.2.6.
- Stale `manifest.md` doc cleanup → v1.2.6.
- Working-tree drift root cause (statusline-probe.txt, `crates/grex/.grex/` runtime artifact) → v1.2.6 spike.
- `--workspace` → `--pack` CLI rename → v1.3.0.
- v1.3.0 contract freeze + MINOR cut → v1.3.0.

No public API removal. Two new doctor subcommand flags (`--prune-quarantine`, `--restore-quarantine`) and one new `grex sync` flag (`--retain-days`). New `QuarantineRestored` event variant. New `RetentionConfig` struct. All additive.

## Acceptance bar

1. **Lean obligations green.**
   - `Grex.Walker.partial_clone_cleanup_idempotent` proved in `proof/Grex/Walker.lean`. Statement: for any `Phase3ChildOutcome` ∈ {`Cancelled`, `Failed(_)`}, the post-state of `dest` on disk equals the pre-clone state (no partial bytes). `lake build` green; 0 `sorry`, 0 `admit`.
   - `Grex.Scheduler.pool_deadlock_guard_terminates` proved in `proof/Grex/Scheduler.lean`. Statement: the lock acquisition graph (per-meta-sync, semaphore, pack-lock, repo-backend, manifest-lock, pool-install) is acyclic across recursion depth. `lake build` green; 0 `sorry`, 0 `admit`.
   - Axiom budget: target ZERO new bridge axioms. If A2 requires a `filesystem_unlink_idempotent` bridge axiom (atomic unlink visibility), enumerate in `Bridge.lean` (10 → 11) and justify inline. If A3 requires a `pool_install_does_not_re_enter` bridge axiom, enumerate (Bridge → 12) and justify. Goal is ≤ 12 Bridge total post-v1.2.5.
   - Quarantine GC + restore = "simple" per Rule 8 (filesystem GC carries no concurrency invariant beyond the per-meta lock that `grex doctor` already takes; restore is a single `rename` under the per-pack lock). Document this exemption in `tasks.md` Stage 1.
2. **A2 regression test.** `partial_clone_cleanup_after_cancellation` — induce a cycle that cancels a sibling mid-clone (use a slow-clone test backend that signals when cleanup must run); assert `dest` does NOT exist on disk after `Err(CycleDetected)` returns.
3. **A3 deadlock guard test.** `pool_deadlock_guard_panics_on_violation` — debug-assert in `phase3_handle_child` that detects `PackLock` re-acquisition under nested `pool.install`; test asserts the assertion fires under the contrived nested-lock pattern (release-mode behavior is "no-op + tracing::warn" — the assert is a debug-build-only safety net).
4. **Quarantine GC test.** `prune_quarantine_removes_old_entries` — populate `<meta>/.grex/trash/` with timestamped dirs spanning ages 0d / 30d / 100d / 200d; run `doctor --prune-quarantine --retain-days 90`; assert 100d + 200d gone, 0d + 30d intact.
5. **Quarantine restore test.** `restore_quarantine_replaces_dest` — quarantine a child, run `doctor --restore-quarantine <ts> <basename>`; assert dest restored byte-identical, `QuarantineRestored` event appended to `events.jsonl`. Negative test: refuse-on-existing-dest unless `--force`.
6. **Retention policy test.** `sync_retain_days_triggers_gc` — `grex sync --retain-days 7` runs doctor GC on each meta visited before Phase 1; assert old entries gone post-sync.
7. **v1.3.0 readiness regression.** Existing `e2e_v1_3_0_readiness_smoke` (added v1.2.4) continues to pass: meta-pack + 1 sub-pack acyclic sync returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` variant in any error path. Plus `grex ls`, `grex doctor`, `grex migrate-lockfile` smoke-runs continue to pass.
8. **No regression on the 380+ existing unit tests + the v1.2.4 `cancellation_aborts_siblings` integration test + the v1.2.3 `e2e_cycle_aborts` integration test.**
9. **Local gates clean:** `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cd proof && lake build`, axiom-policy check (Bridge ≤ 12 if both new axioms needed; ≤ 11 if only one; ≤ 10 if neither).
10. **CI gate.** `#print axioms` smoke check (added v1.2.4) extended to cover both new theorems. Drift fails CI.
11. **Versioning.** Workspace version 1.2.4 → 1.2.5 in workspace `Cargo.toml`, `crates/xtask/Cargo.toml` path-dep pin, `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION`. Man pages regenerated via `cargo xtask gen-man`.
12. **Changelog/history.** `CHANGELOG.md [1.2.5]` entry + `.omne/cfg/history.md` v1.2.5 entry (separate SSOT repo per Rule 7).

## v1.3.0 readiness constraint (maintainer directive)

Same constraint as v1.2.4: v1.2.x is stabilization on path to v1.3.0 MINOR ("full-blown grex with sub-pack under meta-pack feature complete + CLI rename to `--pack`"). Per maintainer constraint:

- **Sub-pack under meta-pack** flow MUST remain functional after v1.2.5 ships.
- **Basic action commands** (`grex sync`, `grex ls`, `grex doctor`, `grex migrate-lockfile`) MUST NOT be broken.
- The existing `e2e_v1_3_0_readiness_smoke` test (codified in v1.2.4) continues to be the regression gate; v1.2.5 adds NO new e2e test for this constraint, only inherits.

v1.2.5 is additive: A2 cleanup only fires on the same cyclic input that v1.2.4 already aborts (no real-world acyclic regression). A3 guard is a debug-build assertion + release-build no-op (no observable behavior change in production builds). Quarantine GC + restore are opt-in (default behavior preserved). No CLI surface removal.

v1.2.x → v1.3.0 roadmap (planning, unchanged from v1.2.4):

- v1.2.4: cancellation + polish + tests + axiom CI (SHIPPED 2026-05-02)
- v1.2.5: A2 partial-clone cleanup + A3 pool deadlock guard + quarantine GC/restore + retention policy (THIS PR)
- v1.2.6: TreeError variant split + cap-std hardening + stale doc + drift root cause
- v1.3.0: `--workspace` → `--pack` CLI rename + behavior contract freeze + MINOR cut

## SemVer

PATCH (1.2.4 → 1.2.5) per maintainer Rule 6. The cleanup logic is internal to `phase3_handle_child` (no public API change). The deadlock guard is a debug-build assertion + release-build no-op. The quarantine subcommand additions and `--retain-days` flag are additive (new flags + new doctor subcommands, no removal). No manifest/lockfile/binary compatibility break. No event-schema break (`QuarantineRestored` is a new variant, additive per the existing JSONL forward-compat policy).

## Process gates (per cfg/workflow.md)

Order of operations is fixed:

1. Phase 1 — OpenSpec (this proposal triplet).
2. Phase 2 — Lean theorems `partial_clone_cleanup_idempotent` (Walker) + `pool_deadlock_guard_terminates` (Scheduler) written and `lake build` green BEFORE any Rust impl change (Rule 8). Rust impl second.
3. Phase 3 — review (parallel reviewers + Codex pass).
4. Phase 4 — PR + merge.
5. Phase 5 — wrap-up (CHANGELOG, SSOT history entry, crates.io publish, tag).

A code change that lands before the Lean proofs is a process violation per `.omne/schemas/rules.md` Rule 8. Per discipline 13, commits MUST NOT carry a `Co-Authored-By` trailer.
