---
slug: feat-real-smoke-harness
type: spec
status: active
last_updated: 2026-05-02
---

# feat-real-smoke-harness — real-environment smoke harness against real GH repos

**Status**: draft
**Branch**: feat-real-smoke-harness
**Depends on**: v1.3.0 SHIPPED (CLI rename + contract freeze) and dogfood-findings-v1.3.0 SSOT post-mortem
**SemVer**: NONE — infra-only; touches no published crate, manifest, lockfile, or binary surface.

## Why now

Dogfooding `grex 1.3.0` against the `cfg` meta-repo on 2026-05-02 surfaced 15 distinct defects (B1–B15, six of them critical or high), none of which were caught by the existing 380+ unit tests, the integration suite, or the `e2e_v1_3_0_readiness_smoke` gate. The SSOT post-mortem (`.omne/var/dogfood-findings-v1.3.0.md`) traces the gap to three concurrent flaws in the existing smoke layer:

1. **In-memory fixtures.** `InMemGit` + tmpdirs replace real network and real filesystem. Real-network bugs (B4 dry-run cloning) and real-`.gitignore` bugs (B12 silent mutation) are invisible by construction.
2. **Same-process round-trips.** Tests deserialize their own writers' output. Field-name mismatches that a real reader (`grex` event-log consumer) would flag (B7 stdout/stderr split, B8 schema_version) are self-cancelling under a unified-process harness.
3. **Env-blocked CI.** The `e2e_v1_3_0_readiness_smoke` gate failed every Windows-runner attempt with `git on PATH: program not found`, so the gate that was supposed to catch the dogfood bugs never executed once. The smoke layer was theatrical, not gating.

Maintainer directive (2026-05-02): "From now on, you need to create different real gh repos that contains pseudo test files for test purposes. Log your findings to SSOT. Next steps we will design those real smoke test first." This proposal is the design instalment.

## Scope

- **6 GH fixture repos** under personal account `egois777` (per maintainer locked decision 1), provisioned via an idempotent script (decision 2), kept stable indefinitely (decision 7), accessed over SSH only (decision 4).
- **Provisioning script** `scripts/provision-real-smoke-fixtures.ps1` that creates / updates the fixtures on demand (idempotent: `gh repo create … || true` then content-seed push).
- **Harness crate** `crates/real-smoke/` (new Cargo workspace member, `[[bin]] name = "real-smoke"`) that drives the `grex` CLI as a subprocess against worktree slices of the fixture repos and asserts FILESYSTEM + stdout/stderr + lockfile + `.gitignore` state.
- **Worktree-per-test** isolation (decision 6): each test calls `git worktree add` on a local clone of the fixture, runs `grex`, asserts, then removes the worktree. Repos themselves untouched.
- **15 regression gates**, one per dogfood bug B1–B15 (per maintainer mandate "log findings to SSOT" → all 15 covered from day 1).
- **Dedicated CI workflow** `.github/workflows/real-smoke.yml` (decision 5), independent from the existing matrix; SSH key injected via `REAL_SMOKE_SSH_KEY` repo secret; `ssh-agent` lifecycle bounded to job duration.
- **SSOT documentation** at `.omne/real-smoke.md` (new file — G1 routing entry + INDEX update).
- **CHANGELOG** `## [Unreleased]` note describing the new gate (no version bump).

## Out of scope

- **Replacement of unit/integration tests.** Real-smoke is a complementary layer — the 380+ existing tests stay, fast feedback stays.
- **B1–B15 fixes themselves.** This proposal lands the *gate* that proves fixes; the fixes ship in the v1.3.x patch plan (v1.3.1 critical bundle → v1.3.4 cleanup per dogfood-findings § Backlog).
- **Cross-host fixture replication.** Single account on a single host (egois777). Multi-host throw-away infra is a future spike.
- **Performance / load testing.** Smoke = correctness on representative inputs, not throughput.
- **Version bump.** Infra-only; no crates.io publish, no tag.

## Acceptance bar

1. `scripts/provision-real-smoke-fixtures.ps1` is idempotent: re-runs after partial failure converge to the same end-state. 6 fixtures (`grex-test-leaf`, `grex-test-meta-flat`, `grex-test-meta-nested`, `grex-test-cycle-a`, `grex-test-cycle-b`, `grex-test-broken-manifest`) live under `git@github.com:egois777/<name>.git` with the documented content seed.
2. `cargo build -p real-smoke` green; `cargo run -p real-smoke -- --help` lists 15 named test cases (B1–B15).
3. Running the harness against a v1.3.0 binary FAILS expected gates (B1–B15) — the gates are RED on the buggy binary; this is the regression-test contract.
4. Running the harness against a v1.3.x binary that fixes the relevant subset turns those gates GREEN, others stay RED until their fix lands.
5. Each test cleans up after itself: no leftover worktrees, no leftover `ssh-agent`, no env-var leaks. Verified by `git worktree list` + `ssh-add -L` post-run.
6. CI workflow `.github/workflows/real-smoke.yml` runs on a Linux runner with `git` on PATH, restores the SSH key from secret, runs the harness, captures artefacts (stdout/stderr per test) on failure.
7. SSOT update lands in the **separate** `grex-inst` repo (Rule 7): `.omne/real-smoke.md` (new), `.omne/schemas/rules.md` G1 row added, `.omne/INDEX.yaml` regenerated by `scripts/build_index.py`.
8. CHANGELOG `## [Unreleased]` carries an "Added: real-smoke harness gate" line; no version bracket bump.
9. All three OpenSpec files (`proposal.md`, `design.md`, `tasks.md`) carry G2-compliant frontmatter and pass `scripts/validate.py` (Rule 10).

## SemVer

NONE. The harness lives in a non-published crate, ships no public API change, and changes no manifest / lockfile / binary contract. Per Rule 6, the maintainer's call here is "infra change, no label."

## Lean obligation

NONE per Rule 8 simple-exemption: this change adds no algorithm, alters no walker invariant, modifies no concurrency primitive. It is a test-infrastructure addition that runs the existing binary as a subprocess and observes externally. Documented in `design.md` § Lean obligation.

## Process gates (per cfg/workflow.md)

1. Phase 1 — OpenSpec triplet (this PR set, branch `feat-real-smoke-harness`).
2. Phase 2 — parallel impl across 2a–2f file-scope partition (see `tasks.md`).
3. Phase 3 — single reviewer pass (infra-only, no Codex deep dive needed).
4. Phase 4 — PR + merge to main.
5. Phase 5 — wrap-up: progress.md endpoint + SSOT history.md note. NO Stage 6 (no publish, no tag).

A code change that races the OpenSpec triplet is a process violation. Per discipline 13, commits MUST NOT carry a `Co-Authored-By` trailer.
