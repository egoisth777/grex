# progress — grex

## Where we are
**M8 COMPLETENESS PASS LANDED ON `feat/m8-release` 2026-04-23.** PR #37 open vs `main`. Five commits on branch build the v1.0.0 release scaffolding end-to-end — release pipeline, crates.io rename, mdBook docs + SemVer policy, pack-template reference + smoke, and the `--json`/MCP-parity/man-pages completeness pass:
- `7fe709c` feat(m8-3,m8-5): mdBook docs site + CHANGELOG + SemVer policy
- `44efdb3` feat(m8-2): bump workspace to 1.0.0, rename bin crate to grex-cli
- `7bebcb6` feat(m8-1): cargo-dist 0.31.0 release pipeline
- `a9941b0` feat(m8-4): pack-template reference + end-to-end smoke
- `0f540ef` feat(m8-6,m8-7,m8-8): v1.0.0 completeness — --json, MCP parity, man pages

All final gates green post-`0f540ef`: `cargo fmt --check` + `cargo clippy --all-targets --all-features --workspace -D warnings` + `cargo test --workspace` (**682 passed / 0 failed / 0 ignored**) + `cargo test -p grex-mcp --test parity` (un-ignored, green) + `cargo xtask gen-man` drift-free + `python .scripts/test.py` + `bash docs/build.sh` + `cargo publish --dry-run -p grex-core` — all green. Issues #32 and #33 are closed by `0f540ef` (MCP parity); #34 (branch-protection on `mcp-conformance`) parked for v1.0.1 as governance, not code.

M7 remains fully shipped on `main` (see prior endpoint block). Post-merge of PR #37, user-owned handoffs for the actual v1.0.0 release: `git tag v1.0.0` + push (fires `release.yml`), `cargo publish` per crate in order (`grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`), `gh repo create egoisth777/grex-pack-template` + initial push, CHANGELOG date swap.

## Endpoint (2026-04-23, feat/m8-release — M8-6/7/8 completeness pass landed; PR #37)
- Branch: `feat/m8-release` at `0f540ef`; PR #37 open vs `main`.
- **M8-6 (`--json` fan-out)**: `--json` wired for the 10 remaining verbs (was 2/12). Stub verbs emit `{"status":"unimplemented","verb":"X"}` as a v1-stable shape; `sync`/`teardown` emit structured `SyncReport`. Missing `<pack_root>` now yields a usage-error envelope + exit 2 (was stub + exit 0). `crates/grex/tests/json_output.rs` 12 tests cover the two-envelope-family contract; `docs/src/cli-json.md` documents it.
- **M8-7 (MCP parity — closes #32 + #33)**: `crates/grex-mcp/src/tools/import.rs` + `doctor.rs` wired through `grex_core` for real. Path-traversal guard `resolve_in_workspace()` with 2 negative tests. MCP `doctor` dropped `fix` param entirely (CLI retains `--fix`); `annotations.read_only_hint = true` is now honest. Parity tests un-ignored + rewritten: tempdir fixture + field-level CLI↔MCP JSON parity (prior skeleton was an error-path false positive). Canonical shapes aligned: doctor `{exit_code, worst_severity, findings[]}`, import `{dry_run, imported[], skipped[], failed[]}`.
- **M8-8 (man pages)**: `crates/xtask/` with `gen-man` subcommand; reuses `Cli::command()` via a new `crates/grex/src/lib.rs` carve-out so the CLI crate exposes its clap definition without duplicating it. 15 man pages generated into `man/`. `ci.yml` gains a `man-drift` job; `[workspace.metadata.dist].include += "man/"` ships them with cargo-dist artifacts; `.cargo/config.toml` alias for `cargo xtask`; `docs/src/man-pages.md` chapter + README subsection.
- **Review methodology**: parallel per-stage reviewers (correctness / security / reliability / simplicity / maintainability / architecture) + `codex:rescue`, two rounds. Blockers fixed before commit.
- **Final gates (post-`0f540ef`)**: `cargo fmt --check` + `cargo clippy --all-targets --all-features --workspace -D warnings` + `cargo test --workspace` (**682 passed / 0 failed / 0 ignored**) + `cargo test -p grex-mcp --test parity` (un-ignored, green) + `cargo xtask gen-man` drift-free + `python .scripts/test.py` + `bash docs/build.sh` + `cargo publish --dry-run -p grex-core` — all green.
- **Parked for v1.0.1**: #34 branch-protection on `mcp-conformance` (governance action, not code).
- **Next action**: land PR #37; then user-owned release handoffs (tag `v1.0.0` → `release.yml` fires; `cargo publish` per crate in order `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`; create `egoisth777/grex-pack-template` + initial push; swap CHANGELOG `[Unreleased - 1.0.0]` date).

## Sub-endpoint (2026-04-22, feat/m8-release — M8-4 pack-template reference + smoke)
- Branch: `feat/m8-release` at `a9941b0`.
- **M8-4 (pack-template)**: `examples/pack-template/` declarative pack — `type=declarative`, `require cmd_available:git`, `mkdir` + `symlink` targeting `$HOME/.grex-pack-template`, explicit `teardown:` for reversibility. Referenced by `docs/src/pack-template.md` mdBook chapter with an external-repo handoff appendix for the forthcoming `egoisth777/grex-pack-template` mirror (user-owned creation).
- **End-to-end smoke**: `crates/grex/tests/pack_template_smoke.rs` runs `grex_core::sync::run` in a tempdir with redirected `$HOME`, then re-runs and asserts the no-op / idempotent outcome. Catches regressions in the exemplar, not just the code.

## Sub-endpoint (2026-04-22, feat/m8-release — M8-1 cargo-dist 0.31.0 release pipeline)
- Branch: `feat/m8-release` at `7bebcb6`.
- **M8-1 (release pipeline)**: `cargo-dist` bumped `0.24.1 → 0.31.0` (pre-`aarch64-linux` + retired `ubuntu-20.04`). 5 targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`. GitHub Attestations for provenance.
- **`.github/workflows/release.yml` hygiene**: workflow-default `contents: read`; build job grants `attestations: write` + `id-token: write`; host/announce jobs grant `contents: write`; fork-PR guard on the release-creation steps. Per-job `timeout-minutes`. Partial-matrix guard before `gh release create`. Idempotency guard aborts if the target tag already exists.
- **`docs/release.md`**: tag procedure, `cargo publish --wait-for-publish --timeout 300` (no `sleep 30` race), verified install via `gh attestation verify`, supported-platforms table, rollback limits.

## Sub-endpoint (2026-04-22, feat/m8-release — M8-2 workspace 1.0.0 + bin-crate rename)
- Branch: `feat/m8-release` at `44efdb3`.
- **M8-2 (version + crate rename)**: `[workspace.package] version = "1.0.0"`; internal deps pinned `{ path, version = "1.0.0" }`; `keywords.workspace = true` + `categories.workspace = true` propagated so all 4 crates inherit. Bin crate **package** renamed `grex → grex-cli` (crates.io `grex` is squatted by pemistahl/grex regex tool v1.4.6); `[[bin]] name = "grex"` preserved so the installed binary is unchanged.
- **Audit + publish order**: `openspec/changes/feat-m8-release/crates-io-names.md` documents the rationale + publish order (`grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`). `cargo publish --dry-run -p grex-core` green.

## Sub-endpoint (2026-04-22, feat/m8-release — M8-3 + M8-5 mdBook + CHANGELOG + SemVer)
- Branch: `feat/m8-release` at `7fe709c` (branch-initial commit off `main @ d5cd99c`).
- **M8-3 (docs site)**: mdBook scaffolding mirrors `.omne/cfg/` content into `docs/src/`; `.github/workflows/docs.yml` builds + deploys with **job-scoped permissions** (pages + id-token only on the deploy job). `[package.metadata.docs.rs]` added to the 3 lib crates (`grex-core`, `grex-plugins-builtin`, `grex-mcp`) so docs.rs picks up feature + target config deterministically.
- **M8-5 (CHANGELOG + SemVer)**: `CHANGELOG.md` in Keep-a-Changelog format with a rolling `[Unreleased - 1.0.0]` that rolls up M1–M7. `docs/semver.md` codifies stability policy spanning manifest schema, CLI surface, MCP tool schemas, and `pack.yaml` — the four public contracts that carry v1 semver weight.
- **Endpoint (this branch)**: scaffolding + version bump + release pipeline + pack-template all landed; completeness pass (M8-6/7/8) shipped in `0f540ef` on 2026-04-23 (see Endpoint above).

## Endpoint (2026-04-23, main, post-M7 closure)
- Branch: `main` at `d5cd99c`; `feat/m8-release` forked from here.
- Worktrees pruned (`.claude/worktrees/{m7-3, rebase-m7-4a, agent-a5cfd746, agent-ac2157de, agent-afed5e7f}` removed); merged feature branches (`feat/m7-3-mcp-ci-conformance`, `feat/m7-4a-import`, `feat/m7-4b-doctor`, `feat/m7-4c-license`) + `worktree-agent-*` branches deleted.
- OpenSpec: `openspec/changes/feat-m7-3-mcp-ci-conformance/` and `openspec/changes/feat-m7-4-import-doctor-license/` archived to `openspec/archive/`.
- `milestone.md`: M7 block marked ✓ COMPLETE 2026-04-23 with commit SHAs + PR numbers.
- CLAUDE.md root: active feature pointer flipped from stale `feat-grex M4` to `feat-grex M8` post-M7.
- **M7 FULLY SHIPPED 2026-04-23.** All six M7 PRs squash-merged to `main`: M7-1 PR #25 → `0b80a63`; M7-2 PR #26 → `e98af8c`; M7-3 PR #28 → `ce01eb5`; M7-4a PR #31 → `aa8c7d1`; M7-4b PR #29 → `5ce880e`; M7-4c PR #30 → `262770a`. Post-merge follow-ups tracked in issues #32, #33, #34, #35.

## Sub-endpoint (2026-04-22, feat/m7-4a-import — M7-4a COMPLETE, awaiting PR review)
- Branch: `feat/m7-4a-import` rebased onto post-M7-4b `main`; ahead of `main` by 2 commits (feat + polish/test-fixups).
- **M7-4a (`grex import --from-repos-json`) — COMPLETE**:
  - New `crates/grex-core/src/import.rs` module: `import_from_repos_json`, `ImportPlan`/`ImportEntry`/`ImportSkip`/`ImportFailure`, `ImportOpts`, `ImportedKind`, `SkipReason`, `ImportError` + 27 `#[cfg(test)]` unit cases (classify heuristic × 9, parse edge-cases × 7, plan/dispatch/idempotence × 9, empty-array + property-style roundtrip × 2).
  - Flat `REPOS.json` → classify (`https`/`git@`/`.git` → `scripted`; empty/path → `declarative`) → plan → skip-on-collision (existing manifest row or dup-in-input) → append `Event::Add` per imported row. `--dry-run` short-circuits before any manifest write; idempotent re-runs are all-skipped; post-`Rm` re-import is non-colliding.
  - CLI wiring: `crates/grex/src/cli/verbs/import.rs` renders the plan as a human table (with `DRY-RUN:` prefix when applicable) or structured JSON under `--json`; `ImportArgs { --from-repos-json, --manifest, --dry-run/-n }` on `crates/grex/src/cli/args.rs`. 10 integration cases in `crates/grex/tests/import_cli.rs`.
  - Parity-test carry-forward: `crates/grex-mcp/tests/parity.rs::parity_import` is `#[ignore]`d with a FIXME block — MCP `tools/import` is still the M7-1 stub, so CLI-now-real + MCP-still-stub diverge on ParitySignal (`PackOpError` vs `Unimplemented`). Re-enable when MCP handler is wired through `grex_core::import::import_from_repos_json` (follow-up to M7-4a).
  - Rebase fix-up on top of M7-4b: `STUB_VERBS` and `each_verb_accepts_required_args` exclude union (`serve`/`doctor`/`import`) now that both `import` and `doctor` are real verbs.
  - **Workspace state (pre-rebase)**: 620 passed, 0 failed, 1 ignored across 56 test binaries; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- **Next action**: push rebased branch, watch CI green on PR #31; follow up with MCP-side import wiring to flip the parity test back on.

## Sub-endpoint (2026-04-22, feat/m7-3-mcp-ci-conformance — CI conformance job landed)
- Branch: `feat/m7-3-mcp-ci-conformance` (forked from `origin/main` `d8dad5f`, rebased onto post-M7-4c `main`).
- **M7-3 (mcp-ci-conformance) — IN PROGRESS**:
  - Append-only change to `.github/workflows/ci.yml`: new `mcp-conformance` job running `mcp-validator` (Janix-ai, tag `v0.3.1`, SHA `d766d3ee94076b13d0b73253e5221bbc76b9edb2`) against a release build of `grex serve` at protocol `2025-06-18`.
  - Self-contained job (own `cargo build --release -p grex`; no `needs:` coupling to the debug `build` matrix). Distinct `Swatinem/rust-cache@v2` key `release` so release target does not thrash the debug cache.
  - Artefact upload `mcp-conformance-reports` (14d retention), always.
  - New doc: `docs/ci/mcp-conformance.md` (pin rationale, bypass procedure, local repro, deliberate-regression smoke).
  - Stage-1 probe SKIPPED locally on Windows (validator Python module + release-build bring-up infeasible outside Linux CI); Option A per user directive — CI is the oracle, iterate via `gh pr checks --watch` on failure. Documented tradeoff in PR body.
- **Spec drift corrected (observed vs. draft proposal)**:
  1. **PyPI publication missing**: `mcp-validator==0.3.1` is NOT on PyPI (only `0.1.1`). Canonical install is `pip install 'git+https://github.com/Janix-ai/mcp-validator@<SHA>'`. Proposal listed PyPI as primary + git as fallback; reality is git-only.
  2. **CLI entry point**: the proposal's `mcp-validator --server-command "<cmd>" --protocol-version <ver>` was unverified. Upstream `ref_gh_actions/stdio-validation.yml@v0.3.1` confirms the real invocation is `python -m mcp_testing.stdio.cli "<server-cmd>" --protocol-version <ver> --output-dir <dir> --timeout <secs>` — server command is POSITIONAL, not a flag.
  3. `docs/ci/mcp-conformance.md` + `ci.yml` job comments document both deltas so future bumps start from reality, not draft.
- **Carry-forward**: maintainer action — add `MCP protocol conformance (2025-06-18)` as a required status check on `main` via branch-protection UI once this PR lands (Acceptance #3). Procedure documented in `docs/ci/mcp-conformance.md` §Bypass.
- **Active branch**: `feat/m7-3-mcp-ci-conformance`.
- **Next action**: push branch, open PR against `main`, watch CI. If validator exits non-zero on baseline, treat as real conformance bug (L6 gate working) or adjust invocation.

## Last endpoint (2026-04-22, feat/m7-4c-license — M7-4c shipped, PR open)
- Branch: `feat/m7-4c-license` (from `origin/main` head `cae9734`).
- **M7-4c (dual-license) — SHIPPED**: workspace now `MIT OR Apache-2.0` across all 4 crates (`grex`, `grex-core`, `grex-mcp`, `grex-plugins-builtin`) via root `[workspace.package].license` + `license.workspace = true` in each crate toml. `grex-mcp`'s m7-2-era inline override (`license = "MIT OR Apache-2.0"`) is removed in favour of the workspace-inherited form, so all 4 crates go through a single source of truth.
- **LICENSE layout at repo root**:
  - `LICENSE-APACHE` — verbatim Apache-2.0 text (sha256 `cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30`, 11358 bytes, fetched from apache.org canonical URL).
  - `LICENSE-MIT` — standard MIT, Copyright (c) 2026 egoisth777.
  - `LICENSE` — dual-license pointer notice + contribution boilerplate.
- **README `## License`**: rewritten with the standard Rust ecosystem "Licensed under either of" block + contribution paragraph dual-licensing inbound contributions. Badge flipped from MIT-only to `MIT OR Apache-2.0`.
- **deny.toml**: allowlist already included both MIT and Apache-2.0; only the stale `# MIT-licensed project` comment was refreshed. `cargo deny check licenses` → `licenses ok` (two informational `license-not-encountered` warnings for ISC + Unicode-DFS-2016 are unrelated to this scope).
- **TDD trail**: Stage-1 red commit landed `crates/grex/tests/license_metadata.rs` (6 asserts) with all 6 asserts failing; Stage-2 green commit flipped all 6 to passing. Red-then-green ordering visible in `git log feat/m7-4c-license`.
- **Verification gates (all GREEN on Windows)**:
  - `cargo test --workspace --all-features` → 615 passed (56 suites, 42.23s).
  - `cargo clippy --workspace --all-targets -D warnings` → No issues found.
  - `cargo fmt --check` → clean.
  - `cargo deny check licenses` → licenses ok.
  - `cargo metadata --format-version=1 --no-deps` shows `"MIT OR Apache-2.0"` for all 4 workspace crates.
- **Scope discipline**: zero edits under `crates/*/src/` — license sub-scope is metadata-only by design. Sibling M7-4a (import) + M7-4b (doctor) branches are untouched.
- **Next action**: push `feat/m7-4c-license`, open PR `feat(m7-4c): adopt MIT OR Apache-2.0 dual license` vs main, watch checks green.

## Sibling endpoint (2026-04-22, feat/m7-4b-doctor, Stage 3 docs complete)
- Branch: `feat/m7-4b-doctor` (forked off `main @ d8dad5f`).
- **grex doctor — IMPLEMENTED**: three default checks (manifest-schema,
  gitignore-sync, on-disk-drift) + opt-in `config-lint` under
  `--lint-config`. Exit code rolls up via worst severity (0 OK / 1
  WARN / 2 ERR).
- **`--fix` safety contract — proven by tests**: only heals
  gitignore-drift via M5-2 writer. Two dedicated unit tests
  (`run_doctor_fix_does_not_touch_manifest_on_schema_error`,
  `run_doctor_fix_does_not_touch_disk_on_drift_error`) plus two CLI
  tests assert byte-unchanged manifest + untouched filesystem on
  non-gitignore errors.
- **Tests**: 614 passed, 1 ignored (parity_doctor — MCP doctor still
  M7-1 not_implemented stub while CLI has real impl; breadcrumb left
  on the test). Property test asserts exit-code roll-up invariant over
  random finding sets.
- **Harness adjustments**: STUB_VERBS and zero-arg stub assertions
  drop doctor; `parity_doctor` is `#[ignore]`'d pending MCP-side impl.
- **Clippy**: `--workspace --all-targets -D warnings` clean; every fn
  ≤ 50 LOC per workspace gates (refactored `check_on_disk_drift` and
  `check_config_lint` into helpers).
- **Next action**: open PR `feat/m7-4b-doctor` → `main`; after merge
  resume with M7-4a (import) or M7-4c (license-dual).

## Earlier endpoint (2026-04-22, main, post-M7-2 squash-merge)
- Branch: `main` (HEAD `e98af8c`); no active feature branch.
- **M7-1 (mcp-server) — SHIPPED & MERGED**: PR #25 squash-merged into `main` as `0b80a63`.
- **M7-2 (test-harness L2-L5) — SHIPPED & MERGED**: PR #26 squash-merged into `main` as `e98af8c`. Stage 7 wired `Scheduler::acquire_cancellable` at the MCP edge (resolves m7-1 PR #25 reviewer flag).
- **Workspace state on main**: test count expectation ~570+ with `--features grex-mcp/test-hooks` (m7-1 baseline 553 + m7-2 L3/L4/L5 layer additions); clippy `--workspace --all-targets -D warnings` clean; schemars 0.8 → 1.0 workspace bump landed via m7-2; `grex-mcp` crate in workspace (license `MIT OR Apache-2.0` inline pending m7-4 workspace migration).
- **Carry-forwards owed to m7-3+**:
  - Wire `PackLock::acquire_cancellable` in production (closes m7-2 spec entry 6 / "L4 same-pack relaxed"; defined but unused on the hot path).
  - Wire `init_state_error()` (defined `error.rs:93`, unused) at the rmcp dispatch layer — closes m7-1 spec entries on pre-init + double-init gates.
- **Carry-forwards owed to m7-4**:
  - Wire CLI `--json` per verb (flips m7-2 ParitySignal from semantic-equiv to byte-equal).
  - Workspace license migration to `MIT OR Apache-2.0` dual (drops the `grex-mcp` inline license override).
  - Full `grex import --from-repos-json` impl + `grex doctor` (3 default checks + `--lint-config` opt-in).
  - The 9 stub MCP verbs swap from `-32601` to real impls.
- **Spec drift documented**:
  - m7-2 `spec.md` `## Known limitations`: 6 entries (L2.2 transport-close, L2.3 protocol-version-only, L2 burst substitute, L2 stderr-null substitute, L3 ParitySignal, L4 same-pack relaxed).
  - m7-1 `spec.md` `## Known limitations`: 3 entries (rmcp 1.5 batch-drop, pre-init gate, double-init gate).
  - m7-1 `spec.md` `## rmcp 1.5.0 wiring notes`: 7 surface quirks captured.
- **Active branch**: `main` (no active feature branch).
- **Next action**: start `feat-m7-3` from `main` head; spec lives at `openspec/changes/feat-m7-3-mcp-ci-conformance/`. (Or `feat-m7-4` first if user prefers — both are ready.)

## Prior endpoint (2026-04-22, feat-m7 — M7-1 shipped, PR #25 open)
- Branch: `feat-m7` (HEAD `19ca7c4`), pushed to origin, PR #25 open vs `main`.
- **M7-1 (mcp-server) — SHIPPED, PR #25 open vs main**:
  - 8 commits on `feat-m7` (chain tip prior to hygiene fix-up), PR: https://github.com/egoisth777/grex/pull/25.
  - Hygiene fix-up commit `19ca7c4` pushed (typos + cargo-deny wildcard + cargo fmt across 39 files).
  - All 8 stages: TDD red→green per stage, independent reviewer per stage, 1-2 fix loops per stage, fix-and-finalize.
  - Stages: 1 deps+scaffold | 2 cancel plumbing | 3 `Scheduler::acquire_cancellable` | 4 `PackLock::acquire_cancellable` | 5 server skeleton + handshake | 6 11 `#[tool]` handlers | 7 `notifications/cancelled` (rmcp built-in) | 8 `grex serve` CLI + smoke.
- **Spec drift documented**:
  - m7-1 `spec.md` `## Known limitations`: rmcp 1.5 batch-drop + pre-init gate + double-init gate (3 entries).
  - m7-1 `spec.md` `## rmcp 1.5.0 wiring notes`: 7 surface quirks captured.
  - Stage 7 `spec.md:21` surface fix patched (`send_request().cancel` → `send_cancellable_request`).
- **Workspace state**:
  - Test count: **553** with `--features grex-mcp/test-hooks` (m7-1 baseline); **555** default.

## Prior sub-endpoint (2026-04-22, feat-m7-2 — M7-2 COMPLETE, PR #26 open vs main)
- Branch: `feat-m7-2` (HEAD `8474c00`), rebased onto post-merge `main` (`0b80a63`, the squash of PR #25). Pushed to origin; PR #26 open vs `main`.
- **M7-2 (test-harness L2-L5) — COMPLETE on `feat-m7-2`, awaiting merge**:
  - 10 commits ahead of `main`: 8 stages + hygiene + progress doc; chain tip `8474c00` (full SHA list available via `git log --oneline main..feat-m7-2`).
  - All 8 stages shipped: Stage 1 RED scaffold → Stage 2 L2 GREEN (Client + duplex harness) → Stage 3 L2 real-pipe per-OS (Linux + macOS + Windows) → Stage 4 L3 normalize (2-token tracing normaliser) → Stage 5 L3 11-parity (verbs vs tools surface) → Stage 6 L4 stress RED → Stage 7 L4 stress GREEN + permit gate at MCP edge → Stage 8 L5 cancel chaos + budget recalibration.
  - PR #26 (`feat-m7-2` → `main`) — 12/12 hard CI gates green; meta-reviewer APPROVED; awaiting squash-merge.
- **Spec drift documented (m7-2)** — 6 entries in m7-2 `spec.md` `## Known limitations`:
  1. L2.2 transport-close deviation
  2. L2.3 protocol-version-only deviation
  3. L2 burst substitute
  4. L2 stderr-null substitute
  5. L3 ParitySignal
  6. L4 same-pack relaxed
- **Production change in m7-2**: Stage 7 wires `Scheduler::acquire_cancellable` at the MCP edge — resolves m7-1 PR #25 reviewer flag (`acquire_cancellable` was "unused in production" at m7-1 close).
- **Workspace state (m7-2 sub-branch)**:
  - Test count: bumped through L3/L4/L5 layers; all-features green.
  - Clippy `--workspace --all-targets -D warnings` clean.
  - schemars 0.8 → 1.0 workspace bump (rmcp 1.5 transitive constraint — `#[tool]` macro derives JsonSchema against schemars 1.x; mismatched majors hit orphan-rule errors at the tools/* boundary).
  - New crate `grex-mcp` (license `MIT OR Apache-2.0` inline pending m7-4 workspace migration).
- **Carry-forwards (open after m7-2 merge)**:
  - **M6 (still open per m6_scope.md)**: delete unused `PackLock::acquire` (sync), delete `Scheduler::permits()`, inline single-element const, rename `OwnCycleGuard` → `VisitedInsertGuard`. Stage 7 H2/H8 verification debt. M5 declarative install path.
  - **M7-1 Stage 2 reviewer flag**: re-export `CancellationToken` from `grex-core` to drop CLI direct `tokio-util` dep (DEFERRED — 16+ file edit).
  - **For m7-3**: wire `PackLock::acquire_cancellable` in production (still unused per m7-2 spec entry 6 / "L4 same-pack relaxed").
  - **For m7-3**: wire `init_state_error()` (defined `error.rs:93`, unused) at the rmcp dispatch layer — closes m7-1 spec entries on pre-init + double-init gates.
- **Active branch**: `feat-m7-2` (HEAD `8474c00`), awaiting PR #26 merge.
- **Next action**: merge PR #26, then start **M7-3** (mcp-validator CI conformance).

## Prior endpoint (2026-04-21, feat-m7 — M7 openspec drafts complete)
- Branch: `feat-m7` (not yet pushed to origin — pushing at session end).
- Commits on branch (chain tip → base): `da2d60c docs(openspec): round-3 revisions — align M7 specs post-review` → `c151e6c docs(openspec): draft M7 change proposals (feat-m7-1/2/3/4)`. (Task note flagged `cf78b16` as a tip SHA, but that commit is an older M6 draft on `feat-m6` — the actual `feat-m7` chain is the two SHAs above.)
- **M7 scope — MCP server + import + doctor + license** — 4 chained openspec changes drafted, NO code written yet.
  - **Review series (3 rounds)**:
    - **R1 REJECT (2/10)**: original drafts proposed custom `grex.<verb>` JSON-RPC surface; reviewers flagged as non-conformant to MCP; rewrote as Path B (MCP-native via rmcp).
    - **R2 REVISE (7.3/10)**: Path B rewrite landed but had 13 findings (tool-surface drift, lock-ordering gaps, tests-crate duplication, validator pin ambiguity, cancel budget missing, etc.); all 13 patched.
    - **R3 SHIP**: scope + feasibility clean from codex + CE parallel review; minor residuals patched (agent-safety annotations, `exec --shell` strip from MCP surface, Barrier-based saturation assertion tightened).
  - **Deliverables locked (all on `feat-m7` branch)**:
    - **feat-m7-1 MCP server** — `rmcp 1.5.0`, 11 tools exposed via `#[tool]` macro, cancellable `Scheduler` + `PackLock` APIs (`acquire_cancellable` via `tokio::select!`), stderr-only tracing, agent-safety annotations on each tool, `exec --shell` stripped from MCP surface (CLI keeps it), single-session stdio server, 5-tier lock ordering honoured end-to-end.
    - **feat-m7-2 test harness** — tests nest under `crates/grex-mcp/tests/` (NO separate `grex-mcp-tests` crate); L2-L5 layers; `tokio::io::duplex` in-process harness; 3-OS real-pipe smoke (Linux + macOS + Windows); 2-token tracing normalizer; Barrier-based saturation (`high_water >= PARALLEL && <= PARALLEL` via `Barrier::wait(N+1)`); cancel budget 250ms Linux/macOS / 500ms Windows.
    - **feat-m7-3 CI conformance** — `mcp-validator==0.3.1` + SHA `d766d3ee94076b13d0b73253e5221bbc76b9edb2` + repo `Janix-ai/mcp-validator`; self-contained release build (no `needs: [build]`); PR-blocking required check.
    - **feat-m7-4 import/doctor/license** — `grex import --from-repos-json`; `grex doctor` with 3 default checks + `--lint-config` opt-in; license `MIT OR Apache-2.0` dual.
  - **SSOT update**: `.omne/cfg/mcp.md` rewritten to Path B MCP-native (rmcp, tools/list, tools/call, notifications/cancelled, protocol 2025-06-18).
  - **Workspace dep drift (M7-1 Stage 6)**: schemars 0.8 → 1.0 workspace bump (rmcp 1.5 transitive constraint — `#[tool]` macro derives JsonSchema against schemars 1.x; mismatched majors hit orphan-rule errors at the tools/* boundary). Spec.md patched in lockstep.
  - **Carry-forward from M6** (still open, picked up in M7 impl window as maintenance): MED maint — unused `PackLock::acquire` sync variant, `Scheduler::permits()`; MED perf — top-level `sync::run` still sequential outside meta recursion, needs `FuturesUnordered` dispatch; verification debt — H2 `register_self_in_visited` ordering, H8 panic-safety test for `PackLockHold`.

## Prior endpoint (2026-04-21, main — M6 closed via PR #24)
- Main head: PR #24 squash-merged 2026-04-21T22:27Z. CI all green including `lake build`.
- **M6 — Concurrency + Lean4 proof** — fully shipped to main 2026-04-21 via 1 squash PR (#24) chaining 3 OpenSpec changes.
  - **feat-m6-1 — Parallel scheduler**: `tokio::sync::Semaphore` gated by `--parallel N` flag (default `num_cpus`); dynamic `worker_threads` on the tokio runtime; `ExecCtx` wired with scheduler permit + cancellation handle; pack execution acquires a semaphore slot before action dispatch. Covers M6 req "bounded Semaphore gated by --parallel N".
  - **feat-m6-2 — Per-pack `.grex-lock` + 5-tier ordering**: per-pack fd-lock file at `<path>/.grex-lock` prevents same-pack double-exec; 5-tier global ordering (workspace → manifest → registry → per-pack → per-action) enforced at runtime via `TierGuard` + `tokio::task_local!` tier stack (migrated from `thread_local!` during CI fix pass to survive work-stealing); `PackLock` acquire is async-safe via `spawn_blocking` for fd-lock syscalls. Covers M6 req "per-pack .grex-lock + global ordering prevents deadlock".
  - **feat-m6-3 — Lean4 mechanized proof**: `lean/` project with `Grex.Scheduler.no_double_lock` + `Grex.Scheduler.no_deadlock` theorems formally verifying that (a) no two tasks hold the per-pack lock for the same path simultaneously and (b) the 5-tier total order on lock acquisition admits no cycle. `lake build` added to CI matrix — green on close. Covers M6 req "Lean4 `.olean` builds green".
  - **Review findings addressed pre-merge (Codex + CE parallel reviews)**:
    - **B1 scheduler wiring**: `--parallel N` flag was parsed but not threaded to the semaphore; fixed by flowing through `SyncOptions::parallel` → `Scheduler::new(n)` → `ExecCtx`.
    - **B2 duplicate `--parallel` flag**: two clap definitions collided in `sync.rs` + `cli/args.rs`; deduped to single source in `cli/args.rs`.
    - **B3 field-order assert**: `PackLock` field drop order affected teardown correctness; added `const _: () = { ... }` static assert pinning the layout.
    - **H1 spawn_blocking fd-lock**: fd-lock acquire was sync-blocking the runtime; wrapped in `tokio::task::spawn_blocking`.
    - **H3 registry GC**: global `PackLockRegistry` leaked entries after pack completion; added refcount-based GC on `PackLockHold::drop`.
    - **H5 live tier enforcement**: tier ordering was documented but not enforced at runtime; added `TierGuard` that panics on out-of-order acquire.
    - **H6 dynamic worker_threads**: tokio runtime hardcoded to `num_cpus()`; now honors `--parallel N` via `Builder::new_multi_thread().worker_threads(n)`.
    - **H9 meta permit release**: `MetaPlugin` recursion held parent permit across child recursion, starving the pool; now releases parent permit before recursing and re-acquires after.
  - **CI regression fixes (post-review, pre-merge)**:
    - **tier stack migrated `thread_local!` → `tokio::task_local!`**: initial `thread_local!` tier stack lost state under tokio work-stealing (runner moves across threads); migrated to `tokio::task::LocalKey` via `task_local!` macro so the stack travels with the task.
    - **`num_cpus` dep cleanup**: added `num_cpus` as direct dep for the `Scheduler::default_parallelism` path (was transitively available via tokio but cargo-machete flagged implicit use).
  - **Carry-forward (M7+ or follow-up PRs)**: see m6_scope.md for full list — key items: MED maintainability (delete unused `PackLock::acquire` sync variant, `Scheduler::permits()`, inline single-element `DEFAULT_MANAGED_GITIGNORE_PATTERNS`, rename `OwnCycleGuard` → `VisitedInsertGuard`); MED perf (top-level `sync::run` still sequential — `FuturesUnordered` dispatch needed for true `--parallel N>1` gain outside meta recursion); H2 `register_self_in_visited` ordering verification; H8 panic-safety test for `PackLockHold`.
  - **Test counts at close**: 495 workspace tests / 501 with `--features grex-core/plugin-inventory` / `lake build` green.

## Prior endpoint (2026-04-21, main — M5 closed via PRs #22 + #23)
- Main head: `20ee5fa feat(m5-2): teardown semantics + gitignore managed blocks + meta recursion (#23)`.
- **M5 — Pack-Type Plugin System** — fully shipped to main 2026-04-21 via 2 PRs.
  - **PR #22 (M5-1)** squash `a2e313d feat(m5-1): pack-type plugin system (trait, 3 builtins, dispatch, inventory) (#22)` — `PackTypePlugin` trait + `PackTypeRegistry` + 3 builtins (meta/declarative/scripted) + executor dispatch swap + `plugin-inventory` auto-registration. Covers R-M5-01..07 + R-M5-12. **410 tests** at close.
  - **PR #23 (M5-2)** squash `20ee5fa feat(m5-2): teardown semantics + gitignore managed blocks + meta recursion (#23)` — gitignore managed-block writer (R-M5-08) + declarative/scripted/meta teardown semantics (R-M5-09/10/11) + MetaPlugin real recursion with cycle detection + multi_thread tokio runtime + `grex teardown` CLI verb + `Action::Unlink` variant for auto-reverse. **470 workspace tests / 382 with `plugin-inventory` feature** at close.
  - **Review process**: parallel Codex + CE reviews surfaced 4 MAJORs (0 blockers), all resolved pre-merge (meta teardown ordering, Symlink+When auto-reverse, cycle-detection under multi_thread, gitignore apply dedupe, plus mixed line-ending + visited_meta doc drift).
  - **Known deferred (M6+)**: declarative install path in `sync::run` still uses M4 action-loop instead of plugin path (`DeclarativePlugin::install` is dead code in production — only teardown routes through plugin). Non-blocking; convergence fix scheduled for next milestone.

## Prior endpoint (2026-04-20, main — M4-E merged via PR #21, M4 closed)
- PR #21 squash-merged to main 2026-04-20. CI: 260 runs, 0 failed.
- **M4-E shipped (2026-04-20)** on `main` via PR #21 squash-merge commit `5206f02 feat(m4-e): plugin-inventory auto-registration — M4 close (#21)` (collapses pre-merge branch work into a single commit). Stage E is additive and optional — default feature set is unchanged; no breaking surface changes.
  - **E1 — `PluginSubmission` wrapper type**: `#[non_exhaustive]` struct with a single private field holding the `&'static dyn ActionPlugin` + public `PluginSubmission::new(plugin: &'static dyn ActionPlugin) -> Self` constructor. Wrapping `inventory`'s submission in a `#[non_exhaustive]` newtype means future metadata fields (plugin version, source crate, etc.) can be added without a semver break — plugin crates `submit!(PluginSubmission::new(&MyPlugin))` today and pick up additions tomorrow.
  - **E2 — `inventory::collect!` + 7 `submit!` sites**: `inventory::collect!(PluginSubmission)` declared in `crates/grex-core/src/plugin/inventory.rs` (feature-gated module); each of the 7 builtins (`symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`) carries a `#[cfg(feature = "plugin-inventory")] inventory::submit!(PluginSubmission::new(&BuiltinPlugin))` adjacent to its `ActionPlugin` impl. Zero-cost when feature is off (module and all `submit!` invocations compile out entirely).
  - **E3 — `Registry::register_from_inventory()` + `Registry::bootstrap_from_inventory()`**: the first iterates all `inventory::iter::<PluginSubmission>` entries and calls `register_dyn` on each (idempotent — re-registering an existing name is a no-op, matching `Registry::register`); the second is a convenience constructor equivalent to `let mut r = Registry::new(); register_builtins(&mut r); r.register_from_inventory(); r`. Both are `#[cfg(feature = "plugin-inventory")]`.
  - **E4 — feature flag semantics**: `plugin-inventory` defined in `crates/grex-core/Cargo.toml` under `[features]` with `inventory = ["dep:inventory"]` added to dependencies as `optional = true`. Default build has zero `inventory` crate in dep tree (verified via `cargo tree --no-default-features`). `register_builtins` remains the canonical, always-available path; inventory is strictly opt-in.
  - **Review findings addressed (3 P2s, folded into squash commit `5206f02`)**:
    - **P2 semver — `#[non_exhaustive]` + `::new()` ctor on `PluginSubmission`**: initial E1 cut had a bare tuple struct `pub struct PluginSubmission(pub &'static dyn ActionPlugin)`; review flagged the public field as a future-compat footgun (can't add metadata without breaking). Hardened to `#[non_exhaustive] pub struct PluginSubmission { plugin: &'static dyn ActionPlugin }` with `pub fn new(...)` constructor. Matches the `#[non_exhaustive]` policy applied workspace-wide since M3 review PR #14.
    - **P2 idempotency — regression test `registry_register_from_inventory_is_idempotent`**: asserts that calling `register_from_inventory()` twice on the same `Registry` leaves `registry.len()` unchanged and every builtin name still resolves via `registry.get(name)`. Locks the no-op-on-duplicate-name contract that `register_dyn` already honored, preventing a future change from introducing double-registration silently.
    - **P2 doc clarification — `register_from_inventory` doc comment**: expanded to state (a) idempotency guarantee; (b) that `inventory` is a global linker-visible collection so every crate in the binary contributes; (c) that the canonical bootstrap for v1 remains `register_builtins` — inventory is a v2 foundation for out-of-tree plugin discovery.
  - **Tests added (+3, 399 default → 402 with feature on)**:
    - `plugin::inventory::tests::inventory_collects_all_seven_builtins` — asserts `inventory::iter::<PluginSubmission>().count() >= 7` and that every builtin name appears in the collected set.
    - `plugin::inventory::tests::bootstrap_from_inventory_registers_all_builtins` — asserts `Registry::bootstrap_from_inventory().get(name).is_some()` for all 7 builtins.
    - `plugin::inventory::tests::registry_register_from_inventory_is_idempotent` — the P2 regression test noted above.
  - **Verification**:
    - Default features: `rtk cargo fmt --check` clean, `rtk cargo clippy --all-targets --workspace -- -D warnings` clean, `rtk cargo test --workspace` **399 passed / 0 failed**.
    - With `--features grex-core/plugin-inventory`: `rtk cargo clippy --all-targets --workspace --features grex-core/plugin-inventory -- -D warnings` clean, `rtk cargo test --workspace --features grex-core/plugin-inventory` **402 passed / 0 failed**.
  - **Zero-drift audit**: (a) `inventory` crate appears in `[dependencies]` as `optional = true` only (0 unconditional references); (b) `#[cfg(feature = "plugin-inventory")]` guards every `inventory::submit!` + module + `Registry` method (0 unguarded `inventory::` references); (c) `PluginSubmission` struct field is private post-fix (0 `pub plugin:` or positional-public occurrences); (d) `register_builtins` remains the canonical path — `sync::run` constructor still calls `register_builtins`, not the inventory bootstrap (inventory is opt-in for downstream consumers, not for `grex` itself in v1).
  - **M4 summary — all 5 stages shipped**:
    - Stage A (PR #20, commit `2175a09`): `ActionPlugin` trait + `Registry` struct + `register_builtins()` + 7 builtins behind the trait.
    - Stage B (PR #20): executor dispatch routed via `Registry::get(name)` in both `FsExecutor` and `PlanExecutor`; `compute_actions_hash` + `ExecResult::Skipped` emission on unchanged-hash re-runs.
    - Stage C (PR #20): real `reg_key` (winreg) + `psversion` (powershell.exe) probes with `PredicateNotSupported` graceful degradation off-Windows.
    - Stage D (PR #20): CLI `--ref`, `--only`, `--force`; lockfile auto-read at start + auto-write at end; real commit-SHA plumbed from `GixBackend` through `PackNode::commit_sha` into `compute_actions_hash`.
    - Stage E (PR #21, squash-merge commit `5206f02`): optional `inventory::submit!` auto-registration behind `plugin-inventory` feature flag; default OFF; v2 foundation.
  - M4 closed; next milestone per `milestone.md` is **M5 — 3 pack-types + gitignore auto**.

## Prior endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-D post-review fix bundle shipped)
- **M4-D post-review fix bundle shipped (2026-04-20)** on `feat/m4-a-plugin-trait`: 11 fix streams close P1/P2 blockers surfaced by the 8-persona panel + codex review of M4-D.
  - **F1 — `--only` workspace-relative + forward-slash matching (P1 codex correctness)**: `skip_for_only_filter` now derives `pack_path.strip_prefix(workspace).unwrap_or(pack_path)` and converts via `.to_string_lossy().replace('\\', "/")` before matching. Uniform representation on Windows + POSIX; eliminates the prior platform skew where `display()` emitted `\\` (globset treats as escape) vs `/`. Root packs (outside workspace) fall back to absolute forward-slash path.
  - **F2 — drop `--only` name-OR-path fallback (P1 codex spec-drift)**: removed the `set.is_match(pack_name)` fallback. Spec §M4 req 6 + `milestone.md:57` + `.omne/cfg/cli.md:84` all say "pack paths"; name-fallback was undocumented widening. Matcher is workspace-relative path only post-fix.
  - **F3 — filtered packs preserve prior lock entries (P1 correctness)**: when `skip_for_only_filter` returns `true`, `run_actions` now inserts `prior_lock.get(pack_name).cloned()` into `next_lock` before `continue`. Prevents the prior-lock-drop regression where a subsequent unfiltered sync re-executed filtered packs from scratch. New e2e test `e2e_only_filter_preserves_prior_lock_entries_for_filtered_packs` locks the 3-run A/B/C sequence.
  - **F4 — `probe_head_sha` surfaces backend errors (P1 reliability)**: replaced silent `.ok()` with explicit `match`: `Ok(s) => Some(s)`, `Err(e) => tracing::warn!(target = "grex::walker", "HEAD probe failed for {}: {e}", dir.display()); None`. Absent `.git` directory remains a silent `None` (truly not a git repo). Operators now see transient gix failures / ACL-denied `.git` reads in logs.
  - **F5 — drop sha preservation carve-out (P1 correctness)**: removed the `prev.sha.clone()` branch in `upsert_lock_entry`. Now `sha` always reflects `commit_sha` verbatim (empty string when probe absent/failed). `actions_hash` is computed with the same `commit_sha`, so both fields stay internally consistent; a future non-empty probe correctly invalidates the skip. Spec §M4 req 4a: hash = `sha256(header || canonical_json(actions) || "\0" || commit_sha)` — empty commit_sha is a legitimate value.
  - **F6 — `globset::GlobSet` no longer in `SyncOptions` public API (P1 api-contract)**: replaced `pub only: Option<GlobSet>` with `pub only_patterns: Option<Vec<String>>` (raw pattern strings). Private `compile_only_globset` in `grex-core::sync` builds the GlobSet on the fly. `globset` dep removed from `crates/grex/Cargo.toml` — only `grex-core` depends on it. Upstream `globset` version bump no longer a breaking change for library consumers.
  - **F7 — `#[non_exhaustive]` on `SyncOptions` (P1 semver policy)**: added `#[non_exhaustive]` + 6 builder-style setters (`with_dry_run`, `with_validate`, `with_workspace`, `with_ref_override`, `with_only_patterns`, `with_force`). Cross-crate callers (CLI, e2e tests) use the builder chain; in-crate `SyncOptions { ... }` literals retained. Matches M3 PR #14 policy applied to all public structs + enums.
  - **F8 — `non_empty_string` rejects whitespace (P2 defense)**: changed `s.is_empty()` → `s.trim().is_empty()`. `--ref " "`, `--ref "\t"`, `--only "\n"` now all rejected by clap value_parser with message "value must not be empty or whitespace-only". New unit test `cli_non_empty_string_rejects_whitespace` covers 5 whitespace shapes × 2 flags.
  - **F9 — `InvalidOnlyGlob` exit-code routing (P2 reliability)**: new `SyncError::InvalidOnlyGlob { pattern, source }` variant on the `#[non_exhaustive]` error enum. CLI `run_impl` maps it to new `RunOutcome::UsageError` → exit code 2 (matches `cli.md` frozen "CLI usage error" slot). Operators no longer see invalid-glob failures masked as generic exit 3.
  - **F10 — `manifest.md` schema drift (P2 docs)**: `sha` field description extended to document empty-SHA semantics (non-git root, probe failure), hash-vs-sha consistency invariant, and the non-fatal lockfile-write policy. Aligned with post-F5 invariant.
  - **F11 — `.omne/cfg/cli.md` updates (P2 docs)**: `grex sync` section expanded to specify (a) `--only` matches workspace-relative pack paths normalized to forward-slash; (b) repeatable, OR-combined; (c) root pack path fallback semantics; (d) dependency-filter caveat (does NOT auto-include `depends_on` / children); (e) `--ref` root-pack exclusion caveat; (f) `--force` non-idempotent-action replay caveat; (g) whitespace rejection policy.
  - **Deferrals** (explicit, not drift):
    - Walker-level `--only` fetch suppression — carry-forward from M4-D landing; still fetch-full, filter-at-execution.
    - `log_force_flag` / `RunContext` / `prepare_run_context` inlining — minor, deferred.
    - `probe_head_sha` yaml / `.git` 2-levels-up heuristic refactor — M5 walker tidy pass.
    - Lockfile-write exit-code escalation — intentionally non-fatal; documented in manifest.md.
    - `walker_probe_head_sha_emits_warn_on_backend_error` test — mock-backend infra not present in `grex-core` walker integration tests; the warn! codepath is single-line and covered by inspection. Real-backend probe failures exercised indirectly via existing walker tests.
  - **Tests added / rewritten (net +1 on this Windows host → 399 total)**:
    - Rewritten: `e2e_only_filter_by_pack_name_runs_just_one_pack` (now uses workspace-relative glob `c` post-F1/F2); `e2e_only_filter_matches_workspace_relative_path` (replaces absolute-path locked test); `e2e_only_filter_multiple_patterns_or_combine` (path-only globs).
    - New: `e2e_only_absolute_path_glob_does_not_match` (F1+F2 negative regression), `e2e_only_filter_preserves_prior_lock_entries_for_filtered_packs` (F3), `e2e_force_plus_dry_run_plans_but_does_not_write_lockfile` (testing-reviewer P1), `e2e_upsert_lock_entry_sha_refreshes_on_commit_sha_change` (testing-reviewer P1 — reads `sha` from lockfile directly), `cli_non_empty_string_rejects_whitespace` (F8).
    - Retired: CLI-crate `only_globset_tests` module (semantics moved to `grex-core::sync::compile_only_globset`; e2e coverage now drives the same entry point the CLI uses).
  - **Verification**: `rtk cargo fmt --check` clean, `rtk cargo clippy --all-targets --workspace -- -D warnings` clean, `rtk cargo check --workspace` clean, `rtk cargo test --workspace` **399 passed / 0 failed** (30 binaries, 398 → 399 net).
  - **Zero-drift audit**: (a) `skip_for_only_filter` signature widened by one `workspace: &Path` parameter; all callers updated; (b) `SyncOptions::only` → `SyncOptions::only_patterns: Option<Vec<String>>`; no public `GlobSet` leak; (c) `globset` removed from `crates/grex/Cargo.toml` dependencies; (d) `#[non_exhaustive]` on `SyncOptions` + 6 builder setters; `crates/grex-core/tests/` + `crates/grex/tests/` migrated to builder chain; (e) `SyncError::InvalidOnlyGlob` variant + `RunOutcome::UsageError` routing; (f) `upsert_lock_entry` prev-sha carve-out: 0 occurrences; (g) `probe_head_sha` `.ok()` pattern: 0 occurrences; (h) `non_empty_string` uses `trim().is_empty()`; (i) spec §M4 req 6 ("pack paths") now matches code — name-OR-path widening removed.

## Prior endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-D shipped)
- **M4-D shipped (2026-04-20)** on `feat/m4-a-plugin-trait`: CLI `--ref` / `--only` / `--force` + lockfile auto r/w + real commit-SHA plumbing. Production code landed in prior agent pass; this pass closes the D1–D4 test coverage gap + doc sync.
  - **D1 — `--ref <REF>` override**: `Walker::with_ref_override` + `SyncOptions::ref_override` thread a global ref override through `walk_and_validate` → `resolve_destination`. Override wins over each child's declared `ref` in the parent manifest; empty strings are filtered at the builder so they no-op.
  - **D2 — `--only <GLOB>` filter**: `build_only_globset` compiles any number of CLI patterns into a single `globset::GlobSet` (empty vec → `None`). `sync::skip_for_only_filter` evaluates against BOTH `pack_path` and `pack_name`, OR-combined across repeated `--only` flags. Non-matching packs skip entirely — zero action execution, zero lockfile write.
  - **D3 — hash-based skip + commit_sha invalidation**: `PackNode::commit_sha` now carries the walker-probed HEAD SHA; `compute_actions_hash(actions, commit_sha)` mixes it in so ref drift invalidates the skip. Unchanged actions + unchanged SHA → `StepKind::PackSkipped`; actions unchanged but SHA changed → re-execute (matches spec §M4 req 4a).
  - **D4 — `--force` bypass**: `SyncOptions::force` + `try_skip_pack` short-circuit bypass. `force=true` → 0 `PackSkipped` steps; `force=false` + unchanged inputs → ≥1 `PackSkipped` step. `log_force_flag` emits a single `tracing::info!` line when active so operators see the bypass in logs.
  - **Tests added (net +13 on this Windows host → 398 total)**:
    - Unit (`crates/grex-core/tests/tree_walk.rs`): `walker_ref_override_wins_over_declared_on_clone`, `walker_ref_override_wins_over_declared_on_checkout`, `walker_empty_ref_override_is_equivalent_to_none` — mock-backend exercises of the `with_ref_override` surface.
    - Unit (`crates/grex/src/cli/verbs/sync.rs` module `only_globset_tests`): `empty_patterns_yield_none`, `single_pattern_compiles_and_matches`, `multiple_patterns_or_combine`, `invalid_glob_surfaces_error` — `build_only_globset` parser in isolation.
    - Integration (`crates/grex/tests/sync_e2e.rs`): `e2e_only_filter_by_pack_name_runs_just_one_pack`, `e2e_only_filter_multiple_patterns_or_combine`, `e2e_only_filter_non_matching_skips_everything`, `e2e_only_filter_matches_workspace_path` (D2); `e2e_commit_sha_change_invalidates_skip` (D3); `e2e_force_bypasses_skip_on_hash` (D4).
  - **Commit-SHA ruling**: a changing commit SHA invalidates `actions_hash` and therefore the skip short-circuit — matches spec §M4 req 4a. `upsert_lock_entry` refreshes `sha` when the walker returned a non-empty commit SHA; empty SHA (local-only root packs) preserves the prior value.
  - **Verification**: `cargo fmt --check` clean, `cargo clippy --all-targets --workspace -- -D warnings` clean, `cargo check --workspace` clean, `cargo test --workspace` **398 passed / 0 failed** (30 binaries, up from 385 on this Windows host).
  - **Zero-drift audit**: `TODO(M4)` marker in `sync.rs` closed — 0 occurrences of `TODO(M4)` remain in the crate. `""` placeholder at the `compute_actions_hash` call site replaced by `commit_sha` sourced from `PackNode`. `SyncOptions` gained `ref_override` / `only` / `force`; `Walker` gained `with_ref_override`; `PackNode` gained `commit_sha: Option<String>`.
  - **Deferrals** (explicit, not drift):
    - Walker-level `--only` fetch suppression — kept conservative: the walker still fetches the full graph, and filtering happens at the execution boundary in `sync::run_actions`. This avoids surprising breakage if a filtered pack declares `depends_on` targets that need to exist in the graph for validator correctness. A fetch-phase short-circuit is a perf refinement for M5+.
    - D1 real-backend coverage — no `--ref` override test against the real `GixBackend`; coverage is mock-only. A `GixBackend + override ref` integration test is a follow-up.

## Prior endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-C post-review fix bundle shipped)
- **M4-C post-review fix bundle shipped (2026-04-20)** on `feat/m4-a-plugin-trait`: 11 fix streams close P1/P2 blockers surfaced by the 8-persona panel + codex:rescue review.
  - **F1 — psversion minor-version bug**: `parse_ps_version_spec` now returns `Option<(u32, u32)>` (was `Option<u32>`, silently dropping minor). The PowerShell command emits `"$($PSVersionTable.PSVersion.Major).$($PSVersionTable.PSVersion.Minor)"`; comparison uses full tuple lexicographic ordering. `>=7.9` no longer passes on 7.0.
  - **F2 — powershell.exe hang**: probe spawns as `Child` and waits with a bounded 5 s deadline via a portable `try_wait` + 50 ms sleep-poll loop (no external `wait-timeout` dep). On timeout, `child.kill()` + `child.wait()` then surface as `ExecError::PredicateProbeFailed { predicate: "psversion", detail: "timeout after 5s" }`. ~50 LOC helper, below the `too_many_lines = 50` ceiling.
  - **F3 — spawn failure misclassified**: `io::ErrorKind::NotFound` (powershell.exe genuinely missing) now degrades to `Ok(false)` matching the `reg_key` NotFound shape. Other `io::Error` kinds surface as new `ExecError::PredicateProbeFailed`. No more bogus `PredicateNotSupported { platform: "windows" }` when the binary is gone.
  - **F4 — PATH-hijack resistance**: probe tries `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe` first, falls back to bare `powershell.exe` only if `SystemRoot` is unset or the absolute path returns NotFound. Bare-name lookup remains for stripped images.
  - **F5 — combiner tolerance for PredicateNotSupported**: new `predicate::evaluate_tolerant` helper converts `PredicateNotSupported` → `Ok(false)` and is used inside `Predicate::AllOf` / `AnyOf` / `NoneOf` **and** `WhenSpec.all_of` / `any_of` / `none_of`. Top-level `Combiner` on `RequireSpec` stays strict (still uses plain `evaluate`), so `require: [{reg_key: ...}]` on non-Windows still bubbles. `PredicateProbeFailed` never swallowed — a broken probe is not a rescue-eligible condition. Closes the cross-platform regression (pre-M4-C `any_of: [reg_key, path_exists]` worked via stub→false; M4-C short-circuited on `?`; fix restores the rescue pattern).
  - **F6 — reg_key forward-slash normalization**: `split_hive` normalizes `/` → `\` before splitting, so `HKCU/Software/X` evaluates identically to `HKCU\Software\X` (real-world YAML authors use both).
  - **F7 — ACL-denied reg_key loud surface**: `open_subkey` errors are classified via `io::Error::raw_os_error()`. `Some(2)` (ERROR_FILE_NOT_FOUND) and `Some(3)` (ERROR_PATH_NOT_FOUND) → `Ok(false)`; everything else → `PredicateProbeFailed { predicate: "reg_key", detail: "<err>: <path>" }`.
  - **F8 — BOM / banner resilience**: new `parse_ps_stdout` strips a leading UTF-8 BOM (`\u{feff}`) and scans `.lines().filter_map(parse_ps_version_spec).next()` so banner / warning lines preceding the numeric line no longer defeat the parse.
  - **F9 — non-zero PS exit loud surface**: `wait_with_timeout` reads both stdout and stderr from the child; non-zero exit yields `PredicateProbeFailed { predicate: "psversion", detail: "exit {code}: {stderr}" }` with stderr truncated to 2 KiB (matches the `ExecNonZero` precedent from M3 PR #18). No more silent `Ok(false)` on probe breakage.
  - **F10 — actions.md + error taxonomy doc sync**: `.omne/cfg/actions.md` predicate table now documents off-platform `PredicateNotSupported` on `reg_key` / `psversion`, the 5 s timeout + `%SystemRoot%` preference on `psversion`, the forward-slash + ACL behaviour on `reg_key`, and the combiner-tolerance vs. top-level-strictness split. Error taxonomy table extended with `PredicateNotSupported` and `PredicateProbeFailed` rows.
  - **F11 — split_hive HKEY leak fix**: introduced `enum HiveTag { Hklm, Hkcu, Hkcr, Hku }`, `split_hive` returns `Option<(HiveTag, String)>`. The `HiveTag → HKEY_*` mapping lives inside the `#[cfg(windows)] eval_reg_key` so the parser layer stays Windows-agnostic and unit-testable off-platform.
  - **Error enum additions**: new `ExecError::PredicateProbeFailed { predicate: &'static str, detail: String }` variant on the existing `#[non_exhaustive]` enum. Zero existing `match` sites broken.
  - **Tests added (net +10 on this Windows host → 385 total)**:
    - Unit (`predicate.rs`): `parse_ps_version_spec_captures_minor`, `parse_ps_stdout_strips_bom`, `parse_ps_stdout_skips_banner_lines`, `parse_ps_stdout_empty_returns_none`, `split_hive_accepts_forward_slash`, `split_hive_accepts_backslash`, `split_hive_unknown_returns_none` (platform-agnostic); `reg_key_forward_slash_matches_backslash`, `ps_version_rejects_unreachable_future_minor`, `ps_version_boundary_51_against_real_host` (Windows-gated). Existing `reg_key_returns_not_supported_on_non_windows` / `ps_version_returns_not_supported_on_non_windows` extended to also assert the `platform` field matches `std::env::consts::OS`.
    - Integration (`tests/executor_plan.rs`): `predicate_any_of_tolerates_unsupported_leg_on_non_windows`, `predicate_top_level_require_bubbles_unsupported`, `when_gate_any_of_tolerates_unsupported_leg_on_non_windows` (all non-Windows-gated — encode the F5 semantics).
  - **Verification**: `cargo fmt --check` clean, `cargo clippy --all-targets --workspace -- -D warnings` clean, `cargo check --workspace` clean, `cargo test --workspace` **385 passed / 0 failed** (30 binaries, up from 375 on this Windows host).
  - **Zero-drift audit**: (a) spec §M4 req 5 ("non-Windows returns `PredicateNotSupported`") unchanged — leaf-level semantics intact; combiner tolerance is a behavioral refinement consistent with the M3 precedent (stubs → false) per the review brief's explicit zero-drift note. (b) `PATH`-hijack risk closed (absolute `%SystemRoot%` path tried first). (c) `io::ErrorKind::NotFound` no longer misclassified as `PredicateNotSupported`. (d) `winreg::HKEY` no longer leaks into the parser layer (hive mapping now Windows-internal). (e) ACL-denied reg reads no longer silently report `false`. (f) BOM / banner noise no longer defeats psversion parse. (g) 2 KiB stderr cap applied uniformly (matches M3 `ExecNonZero` precedent). (h) `PredicateProbeFailed` is never swallowed by combiner tolerance — a broken probe halts loud.
  - **Deferred (explicitly per review brief)**:
    - **HKCC / HKPD hive variants** — low-priority hive coverage; open carry-forward.
    - **WOW64 redirection non-determinism** — architectural; affects grex bitness, out of scope.
    - **`name: ""` vs `null` semantic distinction** — parse-layer concern; revisit only if a pack hits it.
    - **`probe_ps_major` memoization across a sync run** — perf; deferred.
    - **Migrate `predicate.rs` to its own `PredicateError` type decoupled from `ExecError`** — codex M4-D refactor recommendation.
    - **F2 timeout unit test** — requires process-spawn mocking infra not present; 5 s timeout is documented in code comment (see `spawn_powershell_version` / `wait_with_timeout`).
    - **F3 Windows-gated PATH-strip test** — relies on per-test PATH mutation which conflicts with the parallel test runner; intent documented in code comment.
    - **F7 ACL-denied HKLM\SECURITY test** — requires admin-denied hive that differs across Windows SKUs + AV policies; variant-assertion left to manual probe rather than flaky CI.
    - **F9 non-zero PS exit unit test** — same mock-spawn-infra gap as F2; codepath is covered by the `truncate_stderr` / `PredicateProbeFailed` wiring.
  - **DEFERRED to M4-D** — commit-SHA plumbing and `--force` flag unchanged from prior endpoint.
  - **DEFERRED to M5** — closed-enum `Action` hardening unchanged from prior endpoint.

## Prior endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-C shipped)
- **M4-C shipped (2026-04-20)** on `feat/m4-a-plugin-trait`: real predicate probes replace the M3 conservative-false stubs flagged in spec §M4 requirement 5.
  - **`reg_key` (Windows)**: `eval_reg_key` uses `winreg::RegKey::predef(hive).open_subkey(subpath)` and, when a value name is supplied, `get_raw_value`. Hive prefix parser (`split_hive`) accepts `HKCU` / `HKEY_CURRENT_USER` / `HKLM` / `HKEY_LOCAL_MACHINE` / `HKCR` / `HKEY_CLASSES_ROOT` / `HKU` / `HKEY_USERS` (case-insensitive). Unknown hive, empty subpath, or closed-subkey → `Ok(false)` (same conservative leaf shape as the other predicates).
  - **`reg_key` (non-Windows)**: returns `ExecError::PredicateNotSupported { predicate: "reg_key", platform: std::env::consts::OS }` — new variant added to the `#[non_exhaustive]` error enum.
  - **`psversion` (Windows)**: `probe_ps_major` spawns `powershell.exe -NoProfile -Command $PSVersionTable.PSVersion.Major` via `std::process::Command`; parses the numeric major. `parse_ps_version_spec` accepts `">=N"`, `">=N.m"`, `"N"`, `"N.m"` and returns the minimum major; comparison is `major >= target`. Unparsable spec → `Ok(false)` (avoid loud parse-error regression vs. M3 stub); child failure to launch → `PredicateNotSupported`.
  - **`psversion` (non-Windows)**: returns `ExecError::PredicateNotSupported { predicate: "psversion", ... }`.
  - **Evaluator signature change**: `predicate::evaluate` / `predicate::evaluate_when_gate` / the `evaluate_combiner` helpers in `plan.rs` + `fs_executor.rs` all return `Result<bool, ExecError>` (was `bool`). Error propagates through `fs_require` / `plan_require` / `fs_when` / `plan_when` via `?`. Non-predicate leaves (`path_exists`, `cmd_available`, `os`, `symlink_ok`) stay infallible — wrapped in `Ok(..)` at the match site.
  - **Error enum**: new `ExecError::PredicateNotSupported { predicate: &'static str, platform: &'static str }` variant (non-exhaustive enum, zero existing `match` sites broken).
  - **Cross-platform gating**: `winreg` dep already declared as `[target.'cfg(windows)'.dependencies]`; comment updated to list `reg_key` predicate alongside `env`-action persistence as consumers. All Windows-only helpers (`eval_reg_key`, `split_hive`, `probe_ps_major`) live behind `#[cfg(windows)]`; non-Windows twins under `#[cfg(not(windows))]`.
  - **Tests added (net +6 on this Windows host)**:
    - Unit (in `predicate.rs`): `parse_ps_version_spec_accepts_common_shapes`, `parse_ps_version_spec_rejects_garbage` (platform-agnostic); `reg_key_finds_well_known_hklm_software`, `reg_key_missing_path_returns_false`, `reg_key_rejects_unknown_hive`, `ps_version_returns_plausible_major` (Windows-gated); `reg_key_returns_not_supported_on_non_windows`, `ps_version_returns_not_supported_on_non_windows` (non-Windows-gated).
    - Integration (`tests/executor_plan.rs`): retired `predicate_reg_key_defaults_false_stage5a` / `predicate_ps_version_defaults_false_stage5a`; replaced by `predicate_reg_key_errors_on_non_windows` + `predicate_ps_version_errors_on_non_windows` + `predicate_reg_key_probes_real_registry_on_windows` + `predicate_ps_version_probes_powershell_on_windows` (cfg-gated).
  - **Doc drift fixed**: `src/execute/mod.rs` module comment (was "conservatively stubbed to `false`") and `Cargo.toml` `winreg` usage comment both updated. `.omne/cfg/actions.md` required no change — its predicate table already described intended semantics without referring to stubs.
  - **Verification**: `cargo fmt --check` clean, `cargo clippy --all-targets --workspace -- -D warnings` clean, `cargo check --workspace` clean, `cargo test --workspace` **375 passed / 0 failed** (30 binaries, up from 369 on this Windows host).
  - **Zero-drift audit**: (a) `eval_reg_key_stub` / `eval_ps_version_stub` removed (0 occurrences in `crates/`); (b) evaluator stub TODO comments (`TODO(slice-5b)`) removed (0 occurrences); (c) `winreg` still a `[target.'cfg(windows)']` dep, no cross-platform pollution; (d) `evaluate(...) -> bool` signature gone (0 occurrences outside docs); (e) tests file retains `predicate_reg_key_defaults_false_stage5a` name count: 0.
  - **DEFERRED to M4-D** — commit-SHA plumbing and `--force` flag unchanged from prior endpoint.
  - **DEFERRED to M5** — closed-enum `Action` hardening unchanged from prior endpoint.

## Prior endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-B post-review fix bundle shipped)
- **M4-B post-review fix bundle shipped (2026-04-20)** on `feat/m4-a-plugin-trait`: 6 fix streams close P1/P2 blockers surfaced by 8-persona `ce:review` + `codex:rescue`.
  - **W1 — registry propagation (P1 triply-flagged bypass)**: `ExecCtx` now carries `Arc<Registry>`; `WhenPlugin` + `plan_nested` honor the caller's custom registry instead of silently reconstructing builtins. Zero `FsExecutor::new()` call-sites in plugin module.
  - **W2 — hash stability (P1 silent hash instability)**: derived `Serialize` on `RequireSpec` / `Combiner` / `Predicate`; manual canonical `Serialize` for `WhenSpec`; removed `Debug` fallback in `lockfile::hash` (no more `format!("{:?}", …)`); fixed latent `Predicate` untagged bug; pinned golden digest v1 test so future drift breaks CI.
  - **W3 — sync error + halt gating + PackSkipped (P1 halt+skip cascade)**: added `SyncError::Lockfile { path, source }` variant (lockfile I/O was previously misrouted to `Validation`); halt-state gating drops halted-pack entry from prior lock so next run re-executes; emit dedicated `StepKind::PackSkipped` (replaces prior `StepKind::Require` proxy with `action_name: "pack"`).
  - **W4 — step variant hardening (P2)**: `#[non_exhaustive]` on `StepKind::Skipped` variant (in addition to enum-level); `StepKind::PackSkipped { actions_hash }` added to dedicated variant list.
  - **W5 — API surface hygiene (P2)**: `#[doc(hidden)]` on `ActionLogger` / `EnvResolver` / `LogLevel` / `TracingLogger` until M5 wires them into `ExecCtx`; `grex-plugins-builtin` empty stub removed (crate rustdoc notes it as v2-reserved).
  - **W6 — spec normative drift (P2)**: `openspec/feat-grex/spec.md` §1 + `.omne/cfg/architecture.md` L121 trait sketch corrected — async `&Value` changed to sync `&Action` / `ExecStep` to match shipped code. Zero `async fn execute` references remain in normative spec.
  - Verification: `cargo fmt --check` clean, `cargo clippy --all-targets -D warnings` clean, `cargo check --workspace` clean, `cargo test --workspace` **369 passed / 0 failed** (30 binaries).
  - Zero-drift audit (all 10 checks PASS): W1 `FsExecutor::new()` in plugin: 0; W2 `format!("{:?}"` in hash.rs: 0; W3 `StepKind::Require` in sync.rs: 0; W6 `async fn execute` in spec.md: 0; W5 `pub mod pack_types` in plugins-builtin: 0; W4 `#[non_exhaustive]` in step.rs: 6; W5 `#[doc(hidden)]` in log.rs+env.rs: 4; W3 `SyncError::Lockfile`: 2; W3 `StepKind::PackSkipped`: 1.
  - **DEFERRED to M5** — closed-enum `Action` hardening: plugin API can only *shadow* the 7 builtins (ActionPlugin.name() matches an existing kind), not introduce new kinds. Fixing requires opening the enum with an `Action::Extension { name: String, args: Value }` variant + parser update. Architectural, not M4 scope.
  - **DEFERRED to M4-D** — real commit-SHA plumbing: `sync::run_actions` still passes `""` to `compute_actions_hash` with TODO(M4) marker. Needs `PackNode::commit_sha` wired from `GixBackend`.
  - **DEFERRED to M4-D** — force-flag for bypass-skip: `--force` CLI flag to re-execute on hash match is not yet wired.

## Prior-prior endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-B shipped)
- **M4-B shipped (2026-04-20)** on `feat/m4-a-plugin-trait`: Stage B closes executor dispatch swap + lockfile idempotency + trait surface (S1–S5 streams).
  - S1 dispatch refactor: `FsExecutor` / `PlanExecutor` carry `Arc<Registry>`; `execute` body swapped from `match action` to `registry.get(action.name()).ok_or(UnknownAction)`; `ExecError::UnknownAction(String)` variant added; `sync::run` bootstraps one `Arc<Registry>` and shares across both executors via `with_registry`.
  - S2 hash + Skipped reshape: `lockfile::hash::compute_actions_hash` (sha256 of `b"grex-actions-v1\0" || canonical_json(actions) || b"\0" || commit_sha`, lowercase hex); `ExecResult::Skipped { pack_path, actions_hash }` variant; per-pack hash compare in `sync::run_actions` short-circuits when prior lock hash == freshly-computed hash (dry-run always re-plans); `PlanSkipped` reuses `StepKind::Require` shape with `action_name: "pack"` — dedicated variant deferred to M4-D audit-schema work.
  - S3 logger + resolver traits: `grex-core::log::ActionLogger` + `TracingLogger` (default impl over `tracing` crate) + `LogLevel`; `grex-core::env::EnvResolver` with blanket impl for `VarEnv`; both trait-object-safe; `ExecCtx` field wiring deferred to M5 per plugin-api.md reconciliation.
  - S5 doc reconciliation (.omne): `plugin-api.md` + `architecture.md` + `actions.md` aligned to shipped code — uniform `&str` across all three traits, `ExecStep` supersedes `ActionOutcome`, `log.rs` / `env.rs` added to architecture layout, `ExecCtx` pack_id/dry_run/logger deferral documented, builtins-in-`grex-core::plugin` acknowledged.
  - Verification: fmt check clean, `clippy --all-targets -D warnings` clean, `cargo check --workspace` clean, `cargo test --workspace` 361 passed / 0 failed (30 binaries), zero `match action { Action::` in `crates/grex-core/src/execute/`, zero `ExecResult::Skipped { reason` anywhere in workspace.
  - Documented-deferred (NOT drift): (a) `PlanExecutor` uses registry as name-oracle only — Tier-1 plugins are wet-run; planner keeps its own `plan_*` dry-run helpers. (b) Commit SHA wired as `""` in `sync::run_actions` with TODO(M4) — real SHA plumbing through `PackNode` is M4-D follow-up. (c) `StepKind::PackSkipped` dedicated variant not added; reused `StepKind::Require` with `action_name: "pack"` — spec does not mandate a dedicated variant. (d) `ExecCtx` field additions (pack_id, dry_run, logger wiring) deferred to M5; `ActionLogger` + `EnvResolver` traits defined and usable directly by plugins.
  - Drift fixed: `plugin-api.md` ActionPlugin signature block now documents the v1 shipped shape (sync, `&Action`) alongside the v2-facing async + `&Value` target; prior wording described only the v2 form and contradicted code.
- **M4-A audit complete (2026-04-20)**: docs reconciled across `spec.md`, `plugin-api.md`, `architecture.md` (trait signature, registration canonicality, `PackCtx.os` enum, `PackCtx.logger` field, rollback wording). Ready to commit M4-A WIP.
- **M4-A scope relaxed (2026-04-20)**: executor dispatch swap (enum match → `registry.get(name)`) moved from M4-A to M4-B. Threading `Registry` through `FsExecutor` / `PlanExecutor` cascades into >50 test-constructor changes; shipping trait + registry + builtins first, dispatch refactor as its own unit. WIP `crates/grex-core/src/plugin/mod.rs` carries inline deferral note (~lines 20–31). Scope docs (`milestone.md`, `openspec/feat-grex/spec.md`, `.omne/cfg/plugin-api.md`) updated to match.
- **Prior plan/M4 endpoint (2026-04-20)**: M4 Stage A-E scope locked, `milestone.md` M4 rewritten (plugin system), `openspec/feat-grex/spec.md` M4 section appended, `.omne/cfg/plugin-api.md` gaps filled (`Registry`, `register_builtins`, idempotency, `plugin-inventory` flag). Branch `plan/m4-plugin-system`.

## Prior endpoint (2026-04-20, post-M3-review)
- Main head: `7ce186e` (post review series; all 5 fix PRs merged).
- Workspace tests: **316 → 344** (+28 across fix PRs).
- Review series: 8 parallel reviews (4 codex adversarial + 4 analytical subagent); 7 returned, security stalled twice.
- **Fix PRs landed (this session):**
  - **PR #14 — semver hygiene**: `#[non_exhaustive]` on all public enums + arg structs (forward-compat for plugins); `ExecResult::Skipped` variant reserved for M4 lockfile idempotency; Action names switched to `Cow<'static, str>` to allow plugin heap names.
  - **PR #15 — data integrity**: Manifest event stream bracketed by `ActionStarted` / `ActionCompleted` / `ActionHalted` (pre-existing `Sync` event remains readable); `ManifestLock` wraps every sync-path append (per-action scope); `SyncError::Halted(Box<HaltedContext>)` for partial-apply surfacing.
  - **PR #16 — concurrency**: workspace-level fd-lock at `<workspace>/.grex.sync.lock` (non-blocking, fail-fast); per-repo fd-lock at `<dest>.grex-backend.lock` (sibling, not inside dest); dirty-check revalidated after lock acquire + immediately before `materialise_tree`.
  - **PR #17 — cross-platform**: `VarEnv` two-map (inner + Windows `lookup_index` for ASCII-lowercase lookup); `HOME -> USERPROFILE` fallback only in `from_os` / `from_map` (not `insert`); `DupSymlinkValidator` case-folds `dst` on Windows/macOS (ASCII only); `kind: auto` errors when src missing (new `ExecError::SymlinkAutoKindUnresolvable`).
  - **PR #18 — recovery**: Symlink backup rollback on create failure (rename `dst -> .grex.bak` succeeds but create fails → rename back; new `SymlinkCreateAfterBackupFailed` if rollback also fails); startup recovery scan (informational only; auto-cleanup deferred to `grex doctor` M4+); `ExecNonZero` carries truncated stderr (2 KB cap).

## Prior milestone endpoint (pre-review)
- PR #1 merged — M1 scaffold: cargo workspace + clap skeleton + 78 tests + CI.
- PR #2 merged — M2 manifest + lockfile JSONL + atomic fs + fd-lock; 174 tests; adversarial review applied.
- PR #3 merged — M2 hardening: 4 src fixes + 10 CI quality gates; 180 tests, 119 in grex-core.
- PR #6 merged — M3 Stage A: pack manifest parser + 7 Tier 1 actions.
- PR #7 merged — m3-b1: variable expansion module (`$VAR` / `${VAR}` / `%VAR%`, `$$`/`%%` escape).
- PR #8 merged — m3-b2: pluggable plan-phase validator framework + duplicate symlink check.
- PR #9 merged — m3-b3: git backend (GitBackend trait + GixBackend impl via gix 0.70).
- PR #10 merged — m3-b4: pack tree walker + cycle + depends_on validators (GraphValidator sibling trait).
- PR #11 merged — m3-b5a: action executor framework + PlanExecutor (dry-run).
- PR #12 merged — m3-b5b: FsExecutor (real side effects, 7 Tier 1 actions).
- PR #13 merged — m3-b6: `grex sync` verb — end-to-end pipeline.
- PRs #4, #5 merged — dependabot: checkout 4→6, upload-artifact 4→7.
- Workspace tests: 180 → 316 (+136). Main head commit `d160c7c feat(m3-b6): grex sync verb`.
- **.omne main** (ahead 2 earlier session) — 8 MUST-FIX spec gap closures: `when` precedence, empty-list validity, duplicate-symlink policy, variable escape `$$`/`%%`, YAML anchors/aliases rejected, type authority, lockfile hash scope, `children` vs `depends_on` semantics; plus name-regex letter-led tighten.

## Architecture state (post-M3 + post-review)
- `grex-core` modules: `pack`, `vars`, `git`, `tree`, `execute`, `pack::validate`, `sync`.
- 2 executor impls (`PlanExecutor`, `FsExecutor`) share `ActionExecutor` trait — interchangeable by value.
- 2 validator traits: `Validator` (per-manifest) + `GraphValidator` (per-graph).
- `Walker` + `FsPackLoader` + `GixBackend` + validators + executors composed in `sync::run()`.
- DFS post-order traversal (children installed before parent).
- **New modules (review series):** `tests/concurrency.rs`, `tests/sync_recovery.rs`, `tests/sync_concurrent_append.rs`.
- **`VarEnv`** is now a two-map (inner + Windows `lookup_index` for ASCII case-insensitive lookup).
- **Workspace + repo fd-locks**: `<workspace>/.grex.sync.lock` (non-blocking, fail-fast) and `<dest>.grex-backend.lock` (sibling, not inside dest).
- **Event stream**: `ActionStarted` / `ActionCompleted` / `ActionHalted` bracket each action append; `Sync` event retained for reader compat.
- **Error surface**: `SyncError::Halted(Box<HaltedContext>)` carries partial-apply context; `ExecNonZero` truncates stderr at 2 KB.
- **Recovery scan**: pre-run informational scan of stale locks + incomplete event brackets; auto-cleanup deferred to `grex doctor` (M4+).

## Test status
**399 tests default / 402 with `--features grex-core/plugin-inventory`** all green on `main` (post PR #21 squash-merge) on Windows (399 → 402 from M4-E's 3 inventory module tests: `inventory_collects_all_seven_builtins`, `bootstrap_from_inventory_registers_all_builtins`, `registry_register_from_inventory_is_idempotent`; the 399 baseline is preserved because the inventory module + tests are fully feature-gated out when the feature is off). Prior baseline (399 tests on `feat/m4-a-plugin-trait`) (398 → +1 net from the M4-D post-review fix bundle: retired 4 `build_only_globset` unit tests from the CLI crate after moving glob compilation into `grex-core::sync::compile_only_globset`; added 1 `cli_non_empty_string_rejects_whitespace` unit test + 4 new e2e tests — `e2e_only_absolute_path_glob_does_not_match`, `e2e_only_filter_preserves_prior_lock_entries_for_filtered_packs`, `e2e_force_plus_dry_run_plans_but_does_not_write_lockfile`, `e2e_upsert_lock_entry_sha_refreshes_on_commit_sha_change`; rewrote 3 existing e2e tests for workspace-relative semantics without changing their count). On non-Windows runners the 3 Windows-gated M4-C probe tests are replaced by 3 combiner-tolerance integration tests + the `platform` field assertion on the 2 `PredicateNotSupported` tests, so the total count stays equivalent across platforms.

## CI gates active
1. `fmt --check`
2. `clippy -D warnings` (workspace lints: `too_many_lines = "deny"` ≤50 LOC, `cognitive_complexity = "deny"` ≤25)
3. `cargo test --workspace`
4. coverage (cargo-llvm-cov, threshold 60% — TODO M5: raise to 80%)
5. `rustdoc -D warnings`
6. msrv (Rust 1.75)
7. cargo-machete (unused deps)
8. cargo-deny (advisories + licenses + bans + sources)
9. cargo-audit (RUSTSEC, `.cargo/audit.toml` ignores)
10. code-metrics (CBO ≤10/module, cyclomatic ≤15/fn via rust-code-analysis)
11. typos (`.typos.toml` allowlist)

Supplementary:
- semver-checks (skipped pre-v0.1.0, runs on release)
- Dependabot weekly (cargo + github-actions)
- CodeRabbit AI review

## Decisions locked
- Pack = git repo + `.grex/` contract dir; uniform meta-pack model (zero-children = leaf).
- 3 built-in pack-types: `meta`, `declarative`, `scripted`.
- 7 Tier 1 actions: `symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`.
- Manifest = append-only JSONL; lockfile = separate JSONL; both atomic temp+rename.
- Scheduler = tokio runtime + bounded semaphore.
- Embedded MCP stdio JSON-RPC server (not subprocess wrapper).
- Lean4 v1 invariant scope: `Grex.Scheduler.no_double_lock` only.
- Plugin traits: `ActionPlugin`, `PackTypePlugin`, `Fetcher`. In-process registry v1.
- v1 excludes: TUI (ratatui), external plugin loading, additional pack-types/actions.
- Git backend: `gix` 0.70 (pure-Rust).
- License: MIT.
- Crate name: `grex` (binary `grex`).
- Workspace: nested `crates/` w/ `grex` bin + `grex-core` lib + `grex-plugins-builtin` lib.
- **M3 Stage A parse-layer decisions:**
  - Key-dispatch action parsing (not serde untagged enum).
  - Separate `RequireOnFail` vs `ExecOnFail` enums (distinct semantics: require `skip` vs exec `ignore`).
  - Exec `cmd` XOR `cmd_shell` enforced via post-parse mutex check.
  - YAML anchors/aliases rejected at parse (tag-safe pre-pass).
  - Unknown top-level keys accepted only with `x-` prefix.
  - Name regex tightened to `^[a-z][a-z0-9-]*$` (letter-led).
  - `schema_version` must be quoted string `"1"`.
  - Predicate recursion max depth = 32.
  - `ChildRef.path` is `Option`; `effective_path()` strips `.git`.
  - `teardown: Option<Vec<Action>>` preserves omitted-vs-empty distinction.

## Decisions locked during M3 Stage B
- Pluggable validator framework (slice 2 pattern re-used for graph validators).
- GitBackend trait decouples gix from walker (mockable in tests).
- PlanExecutor + FsExecutor share ActionExecutor trait surface — interchangeable by value.
- Variable expansion at execute time (not parse time); escape `$$`/`%%`.
- Cycle identity: `url@ref` (children) / `path:<display>` (root) — diamond-at-different-tags NOT a cycle.
- Env persistence: session scope on all platforms; Windows user/machine via winreg; Unix user/machine returns NotSupported.
- Symlink backup via `<dst>.grex.bak` rename.

## Decisions locked during M3 review series (2026-04-20)
- `#[non_exhaustive]` policy applied to all public enums + arg structs (forward-compat for plugin crates; full list in PR #14 description).
- `ExecResult::Skipped` reserved for M4 lockfile idempotency; not emitted in M3.
- Action names carried as `Cow<'static, str>` to allow plugin heap-allocated names (stays free for built-ins).
- Manifest events bracketed by `ActionStarted` / `ActionCompleted` / `ActionHalted`; existing `Sync` event stays readable.
- `ManifestLock` wraps every sync-path append (per-action scope, not per-sync).
- Workspace-level fd-lock at `<workspace>/.grex.sync.lock` (non-blocking, fail-fast — concurrent sync is a hard error).
- Per-repo fd-lock at `<dest>.grex-backend.lock` (sibling file, NOT inside dest so it survives dest wipe).
- Dirty-check revalidated after lock acquire AND immediately before `materialise_tree` (TOCTOU closure).
- `VarEnv` case-insensitive on Windows via two-map (inner preserves original case; `lookup_index` is ASCII-lowercase → inner key).
- `HOME` → `USERPROFILE` fallback only in `from_os` / `from_map` constructors, NOT in `insert` (insert stays literal).
- `DupSymlinkValidator` case-folds `dst` on Windows/macOS (ASCII only; full Unicode case-folding deferred).
- `kind: auto` errors when `src` is missing (new `ExecError::SymlinkAutoKindUnresolvable`) — previously silently defaulted to file.
- Symlink backup rollback on create failure: if `dst → .grex.bak` rename succeeds but create fails, rename back; new `SymlinkCreateAfterBackupFailed` if rollback also fails.
- Startup recovery scan is informational only (logs stale locks + incomplete brackets); auto-cleanup deferred to `grex doctor` M4+.
- `ExecNonZero` carries truncated stderr (2 KB cap) for diagnosis without unbounded event size.

## Open questions
- crates.io name `grex` likely taken (real package: regex tool). Fallbacks: `grex-cli`, `grex-rm`, scoped `@grex-org/cli`. Check at v0.1.0 publish.
- Windows mandatory `ManifestLock` — needs `append_event_on_fd` API refactor (deferred from M2 hardening).
- Coverage threshold raise 60→80% as M3+ adds tests.
- Semver baseline at v0.1.0 publish.
- Lockfile `actions_hash` field name kept (not renamed to `content_hash`) — revisit at M4 when plugins land.
- `on_fail: ignore` (exec) vs `skip` (require) — confirmed distinct; keep split.
- ~~`reg_key` / `psversion` predicates are conservative stubs~~ — resolved by M4-C (real probes) + M4-C post-review fix bundle (F1–F11 hardening).
- Lockfile idempotency skip (via `actions_hash` compare) deferred from m3-b6 — M4 concern.

## Carry-forwards from M3 review series (open)
- **Perf TODOs** (not blocking M4): `Arc<PackManifest>` to avoid clones; batched manifest appends under single lock; predicate cache on `ExecCtx`; `Cow<str>` hot path in `vars::expand`; `gix` shallow-clone option exposed via `SyncOptions`.
- **Docs TODOs**: README status line stale (claims M1 — actual: M3 complete); `CONTRIBUTING.md` missing; PR template missing; ~39% rustdoc gap concentrated in `grex` CLI crate; only 1 source file has rustdoc code examples.
- **Security review**: codex attempted twice, stalled at synthesis both times — separate retry warranted (not on critical path for M4 kickoff).
- **LOW / later**: Unicode NFC/NFD path equality on macOS; Windows `\\?\` long-path prefix for MAX_PATH; POSIX mode-on-Windows warning for `mkdir { mode: ... }`.

## Files to read for 0-state hop-in
1. `CLAUDE.md`
2. `progress.md` (this file)
3. `milestone.md`
4. `openspec/feat-grex/spec.md`
5. `.omne/cfg/README.md`

## Next action
**M7-1 + M7-2 shipped (2026-04-22).** Start `feat-m7-3` from `main` head — spec at `openspec/changes/feat-m7-3-mcp-ci-conformance/` (mcp-validator==0.3.1 SHA `d766d3ee94076b13d0b73253e5221bbc76b9edb2`, self-contained release build, PR-blocking required check). Or `feat-m7-4` first if preferred — spec at `openspec/changes/feat-m7-4-import-doctor-license/` (import + doctor + workspace license dual + 9 stub-verb fills). Carry-forwards owed to m7-3+: wire `PackLock::acquire_cancellable` in production; wire `init_state_error()` at rmcp dispatch layer. Non-blocking M6 carry-forwards still tracked in `memory/m6_scope.md`.

M4 stage order (shipped 2026-04-20): A → B → C → D → E. All 5 stages ✓ complete.
- A: `ActionPlugin` trait + `Registry` struct + `register_builtins()`; 7 built-ins behind trait; re-exports; plugin-layer unit tests. Dispatch unchanged. [PR #20, `2175a09`]
- B: Executor dispatch refactor (direct `match Action` → `registry.get(name)`) + lockfile `actions_hash` compute + compare → `ExecResult::Skipped` emission. [PR #20, `2175a09`]
- C: `reg_key` + `psversion` real probes (replace stubs). [PR #20, `2175a09`]
- D: CLI `--ref`, `--only <pattern>`, `--force`; lockfile read/write formalized; commit-SHA plumbing. [PR #20, `2175a09`]
- E: Discovery hook (`inventory::submit!` behind `plugin-inventory` feature; default OFF); v2 foundation. [PR #21, squash-merge commit `5206f02` on `main`]

See `.omne/cfg/m3-review-findings.md` for the M3 review-series master finding list and mapping table (finding → PR → resolution).
