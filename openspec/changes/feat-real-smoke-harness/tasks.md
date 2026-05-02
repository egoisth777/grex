---
slug: feat-real-smoke-harness-tasks
type: spec
status: active
last_updated: 2026-05-02
---

# feat-real-smoke-harness — tasks

## Stage 0 — branch + dirs (DONE)
- [x] Cut `feat-real-smoke-harness` from main
- [x] mkdir `openspec/changes/feat-real-smoke-harness`

## Stage 1 — Lean obligation (rule 8 simple exemption)
- [x] No Lean theorem required: no algorithm, no walker invariant, no concurrency primitive added. Harness drives an existing binary as a subprocess. Documented in `design.md` § Lean obligation.

## Stage 2 — parallel impl (file-scope partition per discipline 14)

Six workers; write-sets disjoint at file granularity. Single reviewer pass over all six outputs.

### 2a — provisioning script + fixture seeds
- [ ] Create `scripts/provision-real-smoke-fixtures.ps1` (idempotent: `gh repo create … || true`, then content seed push).
  - Reads from `scripts/real-smoke-fixtures/<name>/` (per-fixture seed dir).
  - Authenticates over SSH (`git@github.com:egois777/<name>.git`).
  - Supports `-Repair` switch to force-push seed back when a fixture has drifted.
  - Backs off on GH 403 / 429; gives up after 5 retries.
- [ ] Seed dirs under `scripts/real-smoke-fixtures/`:
  - `grex-test-leaf/` — `README.md`, `pseudo-app.txt`, `.grex/pack.yaml` (leaf: `name`, `version`, no `children`).
  - `grex-test-meta-flat/` — `README.md`, `.grex/pack.yaml` (meta with 3 leaf children, all bare-named paths). Includes a `<meta>/<bucket>/foo` directory and one child with `path: foo` to seed the B15 collision case.
  - `grex-test-meta-nested/` — `README.md`, `.grex/pack.yaml` (meta with 1 sub-pack child whose `path` carries a slash, e.g. `path: nested/sub`). Mirrors v1.3.0 readiness AC.
  - `grex-test-cycle-a/` — `README.md`, `.grex/pack.yaml` referencing cycle-b.
  - `grex-test-cycle-b/` — `README.md`, `.grex/pack.yaml` referencing cycle-a.
  - `grex-test-broken-manifest/` — `README.md`, `.grex/pack.yaml` with intentional schema error (e.g. unknown enum or missing required field) + a `.gitignore` listing `target/` so the B5 doctor test has fodder.
- [ ] README under `scripts/real-smoke-fixtures/README.md` documenting maintainer rotation procedure (key rotation + repair switch).

### 2b — harness crate
- [ ] Add `crates/real-smoke/` workspace member (`Cargo.toml`, `[[bin]] name = "real-smoke"`, `src/main.rs`, plus modules):
  - `src/main.rs` — CLI entry, `--all` / `--test <name>` / `--list` flags.
  - `src/worktree.rs` — `Worktree` struct with Drop guard, `git worktree add` + `remove --force` lifecycle.
  - `src/grex_cli.rs` — subprocess driver: `run_grex(args, cwd, env_overrides) -> Output`.
  - `src/assertions.rs` — `assert_lockfile_under_grex_dir`, `assert_gitignore_unchanged`, `assert_stdout_no_warn`, `assert_event_log_has_id_and_schema_version`, etc.
  - `src/fixtures.rs` — fixture clone management (`git clone --bare` once, reuse).
- [ ] Workspace `Cargo.toml`: register `crates/real-smoke` as a member.
- [ ] DO NOT publish: harness crate carries `publish = false`.
- [ ] Crate-local `README.md` describing the harness's contract.

### 2c — regression tests for B1–B15
- [ ] `crates/real-smoke/tests/regression.rs` (or per-bug split if cleaner) implementing the 15 cases from `design.md` § Bug-to-test mapping.
- [ ] Each test follows the worktree pattern from `design.md` § Worktree pattern.
- [ ] Each test wraps its worktree in a Drop guard.
- [ ] Each test, on a v1.3.0 binary, FAILS with a clear assertion message naming the bug (RED on buggy build is the contract).
- [ ] Test names: `t_b01_*` … `t_b15_*` (zero-padded so `--list` sorts naturally).

### 2d — CI workflow
- [ ] Create `.github/workflows/real-smoke.yml`:
  - Trigger: `workflow_dispatch` + `pull_request` on PRs labelled `real-smoke` (avoids running on every PR — opt-in gate).
  - Runner: `ubuntu-latest` (Linux, `git` pre-installed and on PATH).
  - Steps: checkout → install Rust → start ssh-agent → add key from `secrets.REAL_SMOKE_SSH_KEY` → `ssh-keyscan github.com` → `cargo run -p real-smoke --release -- --all` → upload artefacts on failure → cleanup ssh-agent in `if: always()`.
- [ ] Job-level concurrency group `real-smoke` so two PRs can't race the fixtures.
- [ ] Maintainer adds `REAL_SMOKE_SSH_KEY` repo secret out-of-band (not in this PR; documented in 2a README).

### 2e — SSOT update (separate `grex-inst` repo per Rule 7)
- [ ] Create `.omne/cfg/real-smoke.md` (G2 frontmatter; `type: design`; canonical reference for the harness contract).
- [ ] Update `.omne/schemas/rules.md` G1 routing table: add row for `cfg/real-smoke.md` (hand-edited, design type).
- [ ] Run `scripts/build_index.py` to regenerate `.omne/INDEX.yaml`.
- [ ] Run `scripts/validate.py` (Rule 15) — exit 0 before SSOT commit.
- [ ] SSOT lands via separate commit in the `grex-inst` working tree (Rule 7); NOT in this grex feature branch.

### 2f — CHANGELOG (grex repo)
- [ ] `CHANGELOG.md` `## [Unreleased]` section: add `### Added` entry "Real-smoke harness gate against fixture GH repos (B1-B15 regression coverage); see `crates/real-smoke/`."
- [ ] No version bracket bump.

## Stage 3 — local gates
- [ ] `cargo fmt --all -- --check` exit 0.
- [ ] `cargo build --workspace` green (real-smoke compiles).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean for the new crate.
- [ ] `cargo test --workspace` — existing 380+ tests still pass; real-smoke's own `cargo test` is intentionally NOT part of the default harness (it requires SSH key + GH access). Gate it behind `#[ignore]` or a feature flag, surface via `cargo run -p real-smoke -- --all` instead.
- [ ] Maintainer runs `cargo run -p real-smoke -- --all` locally against a v1.3.0 binary; verify ALL 15 tests RED with expected assertion messages (the regression-gate contract).
- [ ] `scripts/validate.py` exit 0 (SSOT side; Rule 15).
- [ ] `git worktree list` post-run on each fixture clone shows only the bare entry (cleanup contract verified).

## Stage 4 — review pass
- [ ] Single reviewer pass over the six 2a–2f outputs (infra-only; no Codex deep dive needed).
- [ ] Reviewer scope: provisioning idempotency, harness cleanup contract, CI key handling (no on-disk leak), bug-to-test mapping completeness (all 15), G1+G2+INDEX consistency (SSOT side), CHANGELOG line.
- [ ] Apply review fix-ups in separate workers per discipline 14 (never the original writers).

## Stage 5 — commit + PR + merge
- [ ] Conventional Commit `feat(real-smoke): add real-environment smoke harness against GH fixture repos (B1-B15 regression gate)` — NO `Co-Authored-By` per discipline 13.
- [ ] `git push origin feat-real-smoke-harness`.
- [ ] `gh pr create --base main --head feat-real-smoke-harness` (PR body summarises why-what-how + links proposal/design/tasks).
- [ ] `gh pr checks <num> --watch --interval 30` until green. Note: the `real-smoke.yml` workflow will RED the harness gate against current main (v1.3.0+) — that's expected; reviewers should NOT block on harness-RED, only on the workflow itself running cleanly.
- [ ] After CI green (existing matrix; real-smoke gate excluded from blocking on this PR by design — opt-in label only): `gh pr merge <num> --squash --delete-branch`.
- [ ] Local: `git checkout main; git pull`.

## Stage 6 — NOT APPLICABLE (no version bump, no publish, no tag)
Infra-only change. No `cargo publish`. No `git tag`. No SemVer label.

## Stage 7 — wrap-up
- [ ] Append `## Endpoint (2026-05-XX, main — feat-real-smoke-harness MERGED)` to `progress.md` (grex repo).
- [ ] Update top "Where we are" block to note real-smoke is now the gating layer for v1.3.x patch series.
- [ ] Append section to `.omne/cfg/history.md` (SSOT repo) — infra ship, no version, links to PR.
- [ ] Carry-forward list to v1.3.1: implement the B4/B11/B12/B14/B8/B7 fixes; verify the corresponding `t_b*` tests turn GREEN.

## Out of scope (future work)
- Multi-host fixture replication.
- Performance / load tests against fixtures.
- B1–B15 fixes themselves (those ship in v1.3.1 → v1.3.4 per dogfood-findings § Backlog).
- Auto-running real-smoke on every PR (opt-in label only for now; flip to default-on once the v1.3.1 critical bundle ships).
