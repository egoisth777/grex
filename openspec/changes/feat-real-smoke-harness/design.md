---
slug: feat-real-smoke-harness-design
type: design
status: active
last_updated: 2026-05-02
---

# feat-real-smoke-harness — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/cfg/real-smoke.md` (canonical reference, to be created in grex-inst repo per Rule 7) · `.omne/cfg/dogfood-findings-v1.3.0.md` (origin post-mortem)

## Why

The dogfood post-mortem (`.omne/cfg/dogfood-findings-v1.3.0.md`) enumerates 11 reasons the existing test layer missed B1–B15. Three of them are structural: in-memory fixtures, same-process round-trips, and an env-blocked CI runner. No amount of additional unit tests under the same fixtures would have caught the bugs — the *substrate* is the gap. This design specifies the new substrate.

## Architecture (textual diagram)

```
+----------------------------------------------------------+
| GitHub (egois777, SSH only, 6 stable fixture repos)      |
|   grex-test-leaf                grex-test-cycle-a        |
|   grex-test-meta-flat           grex-test-cycle-b        |
|   grex-test-meta-nested         grex-test-broken-manifest|
+----------------------------------------------------------+
              ^                            ^
              | (idempotent provision)     | (SSH clone over agent)
              |                            |
+-----------------------+        +----------------------------+
| provision script      |        | CI runner (Linux, git+ssh) |
| scripts/provision-... |        | .github/workflows/         |
|     ...ps1            |        |     real-smoke.yml         |
| (run by maintainer    |        | secret REAL_SMOKE_SSH_KEY  |
|  once on rotation)    |        +-------------+--------------+
+-----------------------+                      |
                                               v
                              +----------------------------------+
                              | crates/real-smoke (cargo bin)    |
                              |  - worktree manager              |
                              |  - subprocess driver (grex CLI)  |
                              |  - assertion helpers (FS, lock,  |
                              |    .gitignore SHA256, stdout/err)|
                              |  - 15 regression cases B1..B15   |
                              +----------------------------------+
                                               |
                                               v
                                       +----------------+
                                       | grex CLI       |
                                       | (subprocess,   |
                                       |  current build)|
                                       +----------------+
```

Key shape: real-smoke is *out-of-process*. It exec's `grex` and reads back artefacts the way an operator would. No `grex_core::sync_meta(...)` direct calls — that would re-introduce the same-process trap dogfood-findings § Test-coverage post-mortem #5/#8 identified.

## Repo matrix

| Fixture repo | Pseudo-content (≤10 files) | Has `.grex/pack.yaml`? | Primary scenarios |
|---|---|---|---|
| `grex-test-leaf` | `README.md`, `pseudo-app.txt` | yes (leaf, no children) | empty leaf sync; lockfile-location (B11); `.gitignore` no-op (B12 baseline) |
| `grex-test-meta-flat` | `README.md`, `.grex/pack.yaml` (3 children) | yes (meta) | meta+flat children; `grex ls` rendering (B1); `synthetic` field absence (B6); collision warn (B15) |
| `grex-test-meta-nested` | `README.md`, `.grex/pack.yaml` (sub-pack child uses slash path) | yes (meta with nested child path) | sub-pack-under-meta-pack (v1.3.0 readiness AC); slash paths (B13); cwd-as-pack-root default (B2); `--pack .` / `--workspace .` (B3) |
| `grex-test-cycle-a` | `README.md`, `.grex/pack.yaml` referencing `cycle-b` | yes | cycle pair partner |
| `grex-test-cycle-b` | `README.md`, `.grex/pack.yaml` referencing `cycle-a` | yes | cycle pair partner; cycle-detect regression baseline (carry-forward from v1.2.x) |
| `grex-test-broken-manifest` | `README.md`, `.grex/pack.yaml` (intentional schema error: bad enum value or missing required field) | yes (intentionally invalid) | parse-failure path; doctor drift consult of `.gitignore` (B5); stub-verb exit codes against this fixture (B9); `grex add --ref` (B10) |

Total: 6 repos. Cycle pair counted as 2; could collapse to 1 self-cycle repo if a future cleanup prefers, but A↔B mirrors the dogfood reproducer more faithfully.

## Worktree pattern

Each test:

```text
1. Pick fixture repo (already cloned at $REAL_SMOKE_FIXTURES_DIR/<name>.git as a bare clone).
2. let tmp = mktemp -d                                       # tmpdir per test
3. git -C <fixture-bare> worktree add <tmp> <branch>         # default: main
4. cd <tmp>; run `grex <verb> ...` as subprocess, capture stdout/stderr/exit.
5. Snapshot FS: lockfile path + content, .gitignore SHA256 (pre + post), event-log path + content.
6. Run assertions (per-test).
7. git -C <fixture-bare> worktree remove --force <tmp>       # teardown, even on failure (drop guard).
8. rm -rf <tmp>                                              # belt + braces; worktree remove should clear it.
```

Fixture repos themselves never get mutated by the harness — only worktrees do, and worktrees are removed on teardown. If a test wants to test mutation behavior (e.g. B12 `.gitignore` append), the worktree carries the mutation and is then discarded.

## Subprocess assertions

- **stdout/stderr split.** `Command::output()` returns `Output { stdout, stderr, status }`. Tests assert against each independently — never collapsed. (Closes B7's stream-confusion class.)
- **Filesystem assertions.** Direct `std::fs::read_to_string` / `std::fs::metadata` post-run, on the worktree path. Lockfile location asserted under `.grex/` (B11). `.gitignore` SHA256 compared pre vs post (B12).
- **Lockfile field assertions.** Parse the lockfile (TOML or JSON, whichever ships) and assert specific fields: `branch != ""` when manifest carries `ref:` (B14); no `synthetic: true` keys (B6); `id` field present and `schema_version` populated on event-log records (B8).
- **Exit-code assertions.** Stub verbs (`grex status`, `grex update`) MUST exit non-zero or carry a machine-readable stub marker (B9).
- **CLI surface assertions.** `grex add --ref <ref>` parses (B10); `--pack .` and `--workspace .` parse and resolve (B3); `grex sync` from inside a `.grex/pack.yaml`-bearing cwd defaults pack root to cwd (B2); `grex sync --dry-run` does NOT clone (B4 — assert by checking that no `.git/` directory was materialised under any expected child path post-run, AND that no network egress hit the fixture remote — the latter via SSH-agent connection count or, more tractable, by setting `GIT_SSH_COMMAND="ssh -o BatchMode=yes -o ConnectTimeout=2 -i /dev/null"` so any clone attempt deterministically fails).
- **`grex ls` rendering.** Stdout text-equality against a golden snapshot per fixture; nested children must NOT carry `(scripted, synthetic)` label (B1).
- **`grex doctor` drift logic.** Worktree primed with a `.gitignore`-listed dir; `doctor` MUST NOT flag it (B5).

## CI key management

```yaml
# pseudocode — see tasks.md 2d for actual yaml
- name: Start ssh-agent
  run: eval "$(ssh-agent -s)"; echo "SSH_AUTH_SOCK=$SSH_AUTH_SOCK" >> $GITHUB_ENV; echo "SSH_AGENT_PID=$SSH_AGENT_PID" >> $GITHUB_ENV
- name: Add key
  run: ssh-add - <<< "${{ secrets.REAL_SMOKE_SSH_KEY }}"
- name: Known hosts
  run: mkdir -p ~/.ssh && ssh-keyscan github.com >> ~/.ssh/known_hosts
- name: Run harness
  run: cargo run -p real-smoke --release -- --all
- name: Cleanup ssh-agent (always)
  if: always()
  run: ssh-add -D || true; kill $SSH_AGENT_PID || true
```

Key never lands on disk: piped to `ssh-add` from `secrets.REAL_SMOKE_SSH_KEY` via heredoc. Cleanup runs in `if: always()` block so a failed test still tears down the agent. Maintainer rotates the key by updating the repo secret; no script change needed.

## Bug-to-test mapping

| Bug | Test name | Fixture | Assertion logic |
|---|---|---|---|
| B1 | `t_b01_ls_label_path_shape` | meta-flat + meta-nested | `grex ls` stdout: nested child line has no `(scripted, synthetic)` substring; flat children unchanged. |
| B2 | `t_b02_sync_default_pack_root_cwd` | meta-nested | `cd <worktree>; grex sync` (no `--pack`) exits 0; report claims pack root = cwd. |
| B3 | `t_b03_pack_dot_and_workspace_dot` | meta-nested | `grex sync --pack .` AND `grex sync --workspace .` from worktree both exit 0; no "missing pack root" stderr. |
| B4 | `t_b04_dry_run_no_network` | meta-flat | `grex sync --dry-run` with `GIT_SSH_COMMAND` rigged to fail; exit 0; no `.git/` under any child path; no fetch attempts in stderr. |
| B5 | `t_b05_doctor_consults_gitignore` | broken-manifest (uses .gitignore) | Worktree has `<dir>/` listed in `.gitignore`; `grex doctor` stdout MUST NOT report `<dir>` as drift. |
| B6 | `t_b06_no_synthetic_field` | meta-flat | Post-`sync` lockfile parse: zero entries carry `synthetic: true`. |
| B7 | `t_b07_warn_stderr_op_name` | broken-manifest | `grex sync` produces a warn record; assert it lands in stderr (NOT stdout); assert op name is a Display-formatted string, not `Discriminant(N)`. |
| B8 | `t_b08_event_log_id_and_schema_version` | meta-flat | Event log records (in `.grex/events.jsonl` or wherever the contract puts it) have `id` field (not `pack`) AND `schema_version` field populated. |
| B9 | `t_b09_stub_verbs_exit_nonzero` | leaf | `grex status` and `grex update` exit non-zero OR emit a machine-readable stub marker on stdout. |
| B10 | `t_b10_add_ref_flag` | leaf | `grex add --ref refs/heads/main <url>` parses (no "unknown flag" stderr); resulting manifest entry carries the ref. |
| B11 | `t_b11_lockfile_location_under_grex_dir` | leaf + meta-flat | After `grex sync`, lockfile files (`grex.lock`, `.grex.sync.lock`, etc.) live under `<worktree>/.grex/` and NOT under `<worktree>/` root. |
| B12 | `t_b12_gitignore_no_silent_mutation` | meta-flat | SHA256 of `.gitignore` pre-`sync` == SHA256 post-`sync` (no opt-in flag); OR if mutation IS opted-in, stderr carries an explicit notice line. |
| B13 | `t_b13_nested_slash_path_supported` | meta-nested | Slash-path child (`path: foo/bar`) syncs without "path separators not allowed" error. |
| B14 | `t_b14_lockfile_branch_carries_ref` | meta-flat | Manifest entries with `ref: <ref>` produce lockfile entries where `branch == <ref>` (not empty string). |
| B15 | `t_b15_path_collision_warn` | meta-flat (seed includes a colliding path) | `grex sync` stderr (or report.warnings) carries a collision-warning line for the overlapping `<meta>/<bucket>/foo` vs `path: foo`. |

The 15 tests are RED on a v1.3.0 binary (each maps to a known dogfood bug); they turn GREEN as the v1.3.x patch series fixes the underlying defects. This is intentional — the harness is a regression gate, not a "passing suite" decoration.

## Lean obligation

NONE per Rule 8 simple-exemption. The harness:

- Adds no algorithm. It exec's an existing binary and reads back stdout / stderr / files.
- Modifies no walker invariant, no concurrency primitive, no scheduler / locking primitive.
- Carries no proof obligation.

This exemption is documented inline rather than waved through silently — Rule 8 explicitly says "when in doubt, ask the maintainer," and the doubt here was resolved by the maintainer's 2026-05-02 directive that scoped this work as infra.

## Failure modes

- **GH API rate limit during provisioning.** `gh repo create` has a documented limit (~5000/hr authenticated). Provisioning script backs off on 403 / 429, retries with exponential delay, exits non-zero after 5 attempts so the operator notices.
- **SSH key expiry / rotation.** Repo secret `REAL_SMOKE_SSH_KEY` is the single source. Maintainer rotates by updating the secret; harness picks it up next run. Local-dev rotation: maintainer drops a fresh key at `~/.ssh/real_smoke` and exports `REAL_SMOKE_SSH_KEY_PATH`.
- **Fixture repo state drift.** If a fixture's main branch diverges from the seed (e.g. an accidental push), `provision-real-smoke-fixtures.ps1 --repair` force-pushes the seed back. Manual invocation only — never automatic from CI.
- **Worktree directory leak on test panic.** Each test wraps the worktree in a Drop guard that runs `git worktree remove --force` even on panic unwind. Belt-and-braces post-loop sweep removes any survivors.
- **Concurrent test runs sharing fixtures.** Tests are designed to run serially within a process (single binary, sequential test cases). Two concurrent CI jobs against the same fixture repos risk worktree collisions. Mitigated by per-job `tmp` namespacing; full parallel-job-safety is not in scope (one CI job at a time is sufficient).

## Cleanup contract

Harness binary leaves zero state behind:

- All worktrees registered with `git worktree add` are removed on teardown (Drop guard + post-loop sweep).
- All env vars set for a test (`GIT_SSH_COMMAND`, `GREX_*`) are scoped to the subprocess `Command` call — never to the harness's own process env.
- The `ssh-agent` started by CI is killed in the `if: always()` cleanup step.
- No tmp directories survive (`mktemp -d` outputs are removed in the same Drop guard).
- Fixture repos themselves untouched: no force-push, no branch creation, no tag.

Verified by post-run assertions in CI: `git worktree list` (per fixture clone) shows only the bare repo's default entry; `ssh-add -L` exits non-zero (no identities); `df` of the runner tmpfs shows no growth across runs.
