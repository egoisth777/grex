---
slug: feat-v-1-4-0-tasks
type: spec
status: active
last_updated: 2026-05-15
topic: stub-verb-completion-tasks
---

# feat-v1.4.0 — tasks

## Branch + version

- [x] Rename branch `feat-v1.3.4` → `feat-v1.4.0`
- [x] Bump `Cargo.toml [workspace.package] version` from `1.3.3` → `1.4.0`

## Verb implementations

- [ ] **V1 `init`** — `crates/grex/src/cli/verbs/init.rs`: add optional path, create `.grex/pack.yaml` skeleton, idempotency exit 1, JSON envelope.
- [ ] **V2 `rm`** — `crates/grex/src/cli/verbs/rm.rs`: load manifest, refuse meta-with-children unless `--force`, run `sync::teardown`, rmtree.
- [ ] **V3 `update`** — `crates/grex/src/cli/verbs/update.rs`: delegate to `verbs::sync::run`.
- [ ] **V4 `status`** — `crates/grex/src/cli/verbs/status.rs`: `sync::run` with `dry_run=true`, render per-pack drift.
- [ ] **V5 `run`** — `crates/grex/src/cli/verbs/run.rs`: walk tree, filter actions by name, execute via PlanExecutor.
- [ ] **V6 `exec`** — `crates/grex/src/cli/verbs/exec.rs`: spawn `cmd[0]` with `current_dir=pack_root`, stream stdio.
- [ ] **V7 `add` B15** — `crates/grex/src/cli/verbs/add.rs`: pre-check parent manifest children for path collision.

## Arg-struct updates

- [ ] `InitArgs` — add `path: Option<PathBuf>`.
- [ ] `RmArgs` — add `--force` boolean.
- [ ] `StatusArgs` — add `pack_root: Option<PathBuf>`.
- [ ] `RunArgs` — add `pack_root: Option<PathBuf>`.
- [ ] `ExecArgs` — add `--pack <path>` optional.

## Tests

- [ ] `crates/grex/tests/init_cli.rs` — happy path + idempotency-exits-1 + JSON envelope.
- [ ] `crates/grex/tests/rm_cli.rs` — happy path + refuses-meta-with-children + `--force` override.
- [ ] `crates/grex/tests/update_cli.rs` — smoke that update exits same as sync against a fixture.
- [ ] `crates/grex/tests/status_cli.rs` — clean state + drift state + JSON envelope.
- [ ] `crates/grex/tests/run_cli.rs` — matched action runs + no-match-exits-0.
- [ ] `crates/grex/tests/exec_cli.rs` — happy path + exit code propagation + cwd check.
- [ ] `crates/grex/tests/add_cli.rs` — extend with B15 collision case.

## Verification

- [ ] `cargo check --workspace --all-targets` clean.
- [ ] `cargo test --workspace --all-targets --no-fail-fast` clean (no regressions vs baseline).
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] `/codex:review` on the diff; address blockers; re-run on retry per goal hook.

## Release

- [ ] CHANGELOG.md `## [1.4.0] — 2026-05-15` block.
- [ ] PR open to `main`, squash-merge after green CI.
- [ ] SSOT `inst/grad/progress.md` endpoint added post-merge (separate SSOT commit).
