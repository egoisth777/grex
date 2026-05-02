---
slug: feat-v1.3.0-cli-rename-freeze-tasks
type: spec
status: active
last_updated: 2026-05-02
---

## Stage 0 — branch + dirs (DONE)
- [x] Cut feat-v1.3.0 from main @ 5bb2520
- [x] mkdir openspec/changes/feat-v1.3.0-cli-rename-freeze

## Stage 1 — Lean obligations (NONE — Rule 8 simple exemption)

**Rule 8 simple exemption applies to all 10 scope items.** v1.3.0 has ZERO new algorithmic behavior. Per-item justification:

- [x] **CLI rename via clap alias** — pure surface rename. clap's alias mechanism is documented + tested upstream. No new algorithm; same clap parser produces same field. Exempt.
- [x] **Doc-noun rename** — doc text only; zero code path change. Exempt.
- [x] **JSON dual-emit** — pure additive field on a serde-derived struct. Trivially satisfied by `let pack = workspace.clone();` at construction. Exempt.
- [x] **MCP `pack` param + precedence** — `Option::or` is a stdlib function with documented semantics. Pure routing. Exempt.
- [x] **Behavior contract freeze** — documentation only. No code behavior change. Exempt.
- [x] **Deprecation deferrals** — symbols remain in place with `#[deprecated]` attribute; no removal until v1.4.0. Exempt.
- [x] **NEW SSOT files** (freeze + migration docs) — net-add documentation; no code touched. Exempt.
- [x] **Plugin-API UNSTABLE marker** — doc + Cargo.toml metadata; no code behavior change. Exempt.
- [x] **e2e smoke extension** — extends an existing test (`e2e_v1_3_0_readiness_smoke`) with one warn-once assertion. The walker exercised by the test is already proven via `sync_meta_no_cycle_infinite_clone` + `cancellation_terminates_promptly`. No new algorithmic obligation. Exempt.
- [x] **MSRV unchanged** — no language feature dependency added. Exempt.

**Verification gate (no proof work needed; verify pre-existing proofs still green):**
- [ ] `cd proof && lake build` exit 0 — verify all v1.2.x theorems still green (extensions in v1.3.0 must not break prior proofs since v1.3.0 doesn't touch the proof tree at all):
  - `sync_meta_no_cycle_infinite_clone` (v1.2.2/3)
  - `cancellation_terminates_promptly` (v1.2.4)
  - `partial_clone_cleanup_idempotent` (v1.2.5)
  - `pool_deadlock_guard_terminates` (v1.2.5)
  - `walker_subpath_resolution_bounded_by_meta_dir` (v1.2.6)
- [ ] `#print axioms` for all 5 headline theorems matches v1.2.6 baseline (no new axioms expected; v1.3.0 has zero algorithmic change → zero bridge delta)
- [ ] Bridge axiom count unchanged from v1.2.6 (≤ 12)

## Stage 2 — Rust implementation (parallel workers, after Stage 1 verification gate)

### File scope partition (parallel-worker-safe — disjoint write-sets per worker)

| Worker | Files | Role |
|---|---|---|
| 2a | `crates/grex/src/verbs/sync.rs`, `crates/grex/src/verbs/serve.rs`, `crates/grex/src/verbs/migrate_lockfile.rs`, `crates/grex/src/verbs/teardown.rs`, `crates/grex/src/deprecation.rs` (NEW), `crates/grex/src/main.rs` | clap alias on 4 verbs + warn-once helper |
| 2b | `crates/grex/src/verbs/ls.rs`, `crates/grex/src/verbs/doctor.rs` | JSON dual-emit on 2 envelopes |
| 2c | `crates/grex-mcp/src/sync.rs` | MCP `pack` param + precedence |
| 2d | `crates/grex-core/src/exec.rs`, `crates/grex-plugins-builtin/src/plugin/mod.rs`, `crates/grex-plugins-builtin/Cargo.toml` | ExecCtx pack field add + Plugin-API UNSTABLE marker |
| 2e | `crates/grex/tests/cli_alias.rs` (NEW), `crates/grex/tests/cli_json.rs` (NEW or extend), `crates/grex-mcp/tests/sync_pack.rs` (NEW), `crates/grex/tests/sync_e2e.rs` (extend) | new test files + e2e smoke extension |
| 2f | `Cargo.toml`, `crates/xtask/Cargo.toml`, `crates/xtask/tests/version_test.rs`, manpages dir (regenerated) | version bump 1.2.6 → 1.3.0 + manpage regen |
| 2g | `CHANGELOG.md` | CHANGELOG entry + carry-forward deferral note |
| 2h | `.omne/cfg/freeze-v1.3.0.md` (NEW), `.omne/cfg/migration-v1.3.0.md` (NEW) | SSOT NEW docs (separate repo per Rule 7) |
| 2i | `.omne/cfg/cli.md`, `.omne/cfg/api-contract.md`, `.omne/cfg/mcp.md`, `.omne/cfg/walker.md`, `.omne/cfg/plugin-api.md` | SSOT existing doc updates per Round 3 gap list (separate repo) |

### 2a — clap alias + warn-once + verb deprecation

- [ ] In `crates/grex/src/verbs/sync.rs`: change `#[arg(long)]` to `#[arg(long = "pack", alias = "workspace")]` on the `workspace` field; rewrite doc-comment to use `<pack>` noun + mention `--workspace` deprecated alias
- [ ] In `crates/grex/src/verbs/serve.rs`: same pattern on `ServeArgs::workspace`
- [ ] In `crates/grex/src/verbs/migrate_lockfile.rs`: same pattern on `MigrateLockfileArgs::workspace`
- [ ] In `crates/grex/src/verbs/teardown.rs`: same pattern on `TeardownArgs::workspace`
- [ ] Create `crates/grex/src/deprecation.rs` (NEW): `warn_workspace_deprecation_if_used()` helper with `OnceLock<()>` guard. Detects `--workspace` (or `--workspace=`) in `std::env::args()` and emits one-time `eprintln!("warning: --workspace is deprecated; use --pack instead. The alias will be removed in a future major release.")`
- [ ] In `crates/grex/src/main.rs`: call `warn_workspace_deprecation_if_used()` after `Cli::parse()` (before any subcommand dispatch)
- [ ] Add `mod deprecation;` to `crates/grex/src/lib.rs` (or wherever the module tree is rooted)

### 2b — JSON dual-emit (ls + doctor envelopes)

- [ ] In `crates/grex/src/verbs/ls.rs`: add `pack: PathBuf` field to `LsEnvelope` struct (after `workspace` field for byte-stable JSON order); construction sets `pack: workspace.clone()`
- [ ] In `crates/grex/src/verbs/doctor.rs`: same pattern on `DoctorEnvelope` (or whatever the envelope struct is named)
- [ ] Verify serde JSON output preserves field order: `workspace` first, `pack` second (manual smoke run + assert in 2e tests)

### 2c — MCP `pack` param (sync.rs)

- [ ] In `crates/grex-mcp/src/sync.rs`: add `pub pack: Option<PathBuf>` to `SyncParams` (after existing `workspace` field)
- [ ] Add `impl SyncParams { pub fn resolved_pack_root(&self) -> Option<&PathBuf> { self.pack.as_ref().or(self.workspace.as_ref()) } }`
- [ ] Replace any existing `params.workspace.as_ref()` consumers in the same file with `params.resolved_pack_root()` (additive; preserves back-compat for callers sending only `workspace`)
- [ ] Document precedence rule inline (rustdoc on `pack` field + `resolved_pack_root` method)

### 2d — ExecCtx pack field add + Plugin-API UNSTABLE marker

- [ ] In `crates/grex-core/src/exec.rs` (or current ExecCtx location): add `pub pack: PathBuf` field after `pub workspace: PathBuf`; constructor (`ExecCtx::new` or struct literal sites) sets `pack: workspace.clone()` (or equivalent)
- [ ] In `crates/grex-plugins-builtin/src/plugin/mod.rs`: prepend crate-level rustdoc UNSTABLE WARNING block:
  ```rust
  //! # ⚠ UNSTABLE — Plugin API
  //!
  //! This crate's public API is **NOT frozen** in v1.3.0. The Plugin-API
  //! contract is a v1.4.0 freeze candidate. Downstream plugin authors
  //! should expect breaking changes between minor releases until v1.4.0.
  //!
  //! See `.omne/cfg/plugin-api.md` for the freeze roadmap.
  ```
- [ ] In `crates/grex-plugins-builtin/Cargo.toml`: append "(UNSTABLE — Plugin-API frozen in v1.4.0)" to the `description` field

### 2e — New tests + e2e smoke extension

- [ ] T-CLI-A1: `cli_workspace_pack_alias_parses_both` in `crates/grex/tests/cli_alias.rs` (NEW). Construct args `["grex", "sync", "--workspace", "/tmp/p"]` and `["grex", "sync", "--pack", "/tmp/p"]`; parse via `Cli::try_parse_from`; assert resolved `SyncArgs::workspace` is `Some(PathBuf::from("/tmp/p"))` for both. Repeat for `serve`, `migrate-lockfile`, `teardown` (4 sub-tests).
- [ ] T-CLI-A2: `cli_workspace_deprecation_warns_once_per_process` in `crates/grex/tests/cli_alias.rs`. In a single test, invoke `warn_workspace_deprecation_if_used()` twice with `--workspace` in env::args (use a `Mutex<Vec<String>>` capture for stderr); assert diagnostic emitted exactly once. (Note: real-process subprocess invocation may be cleaner — use `std::process::Command::new(env!("CARGO_BIN_EXE_grex"))` + capture stderr).
- [ ] T-CLI-J1: `cli_ls_doctor_json_envelopes_dual_emit_workspace_pack` in `crates/grex/tests/cli_json.rs` (NEW or extend existing JSON test file). Construct synthetic envelope; serialize via `serde_json::to_string`; parse back into `serde_json::Value`; assert `obj.contains_key("workspace") && obj.contains_key("pack")` AND `obj["workspace"] == obj["pack"]` AND key order in raw bytes is `workspace` first, `pack` second.
- [ ] T-MCP-P1: `mcp_sync_pack_or_workspace_precedence` in `crates/grex-mcp/tests/sync_pack.rs` (NEW). Four sub-cases: (a) `{pack: Some("/a"), workspace: Some("/b")}` → resolved = "/a"; (b) `{pack: Some("/a"), workspace: None}` → "/a"; (c) `{pack: None, workspace: Some("/b")}` → "/b"; (d) `{pack: None, workspace: None}` → None. Assert via `SyncParams::resolved_pack_root()`.
- [ ] T-E2E-EXT: extend `e2e_v1_3_0_readiness_smoke` in `crates/grex/tests/sync_e2e.rs` with deprecation warn-once assertion: invoke meta-pack + 1 sub-pack acyclic sync via subprocess with `--workspace` flag twice; capture stderr; assert deprecation diagnostic appears exactly once across both invocations (or exactly once per invocation if the test runs them in isolated processes — clarify in implementation).

### 2f — Version bump + manpage regen

- [ ] Workspace `Cargo.toml` version 1.2.6 → 1.3.0
- [ ] 3 internal path-deps (grex-core, grex-mcp, grex-plugins-builtin) bumped to 1.3.0 (workspace inheritance picks them up automatically if using `version.workspace = true`; otherwise edit each crate's Cargo.toml)
- [ ] `crates/xtask/Cargo.toml` grex-cli path-dep 1.2.6 → 1.3.0
- [ ] `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION` 1.2.6 → 1.3.0
- [ ] Regenerate man pages: `cargo xtask gen-man` — expected diff: 4 manpages (sync, serve, migrate-lockfile, teardown) get `<workspace>` → `<pack>` doc-noun rewrite + `--pack` listed as canonical flag with `--workspace` in alias position; other manpages unchanged

### 2g — CHANGELOG entry + carry-forward deferral note

- [ ] CHANGELOG.md: append `[1.3.0] - 2026-05-XX` MILESTONE entry per design.md "Migration note for changelog" template (Added / Changed / Deprecated / Frozen / Tests / Notes sections)
- [ ] Inline note: deferred-to-v1.4.0 carry-forward list:
  - `PackLock::acquire` synchronous variant removal
  - `Scheduler::permits()` removal
  - `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const removal
  - Plugin-API freeze
  - `--workspace` CLI flag removal (gated to v2 — alias stays through v1.x)
  - `ExecCtx::workspace` field removal (gated to v2)

### 2h — SSOT NEW docs (separate repo per Rule 7)

- [ ] In `.omne/cfg/freeze-v1.3.0.md` (NEW, SSOT working tree): G2 frontmatter (`type: design`, `status: active`, `topic: freeze`, `last_updated: 2026-05-02`); body = 4-column freeze table per design.md Table F1 (13 rows + Plugin-API exclusion row); cross-link to `cfg/api-contract.md` and `cfg/migration-v1.3.0.md`
- [ ] In `.omne/cfg/migration-v1.3.0.md` (NEW, SSOT working tree): G2 frontmatter (`type: migration`, `status: active`, `topic: migration`, `last_updated: 2026-05-02`); body = operator section (Tables M1, M2 from design.md) + Rust consumer section (Table M3); strict-schema consumer note for JSON envelope key addition; cross-link to `cfg/cli.md`, `cfg/mcp.md`, `cfg/freeze-v1.3.0.md`
- [ ] Add both new files to G1 routing table in `.omne/schemas/rules.md` (per discipline 11)
- [ ] Run `.omne/scripts/validate.py` — exit 0 against both new files (frontmatter intact, slug unique, kebab-case)
- [ ] Run `.omne/scripts/build_index.py` — auto-regenerates `.omne/INDEX.yaml` to include both new files (per discipline 12)
- [ ] Commit + push in SSOT repo (separate from grex per Rule 7); the grex feature branch never carries SSOT updates

### 2i — SSOT existing doc updates (per Round 3 gap list)

- [ ] In `.omne/cfg/cli.md` (SSOT): update flag descriptions for sync/serve/migrate-lockfile/teardown to use `<pack>` doc-noun; add note that `--workspace` is a deprecated alias (warn-once); cross-link to `cfg/migration-v1.3.0.md`; bump `last_updated`
- [ ] In `.omne/cfg/api-contract.md` (SSOT): add cross-link to `cfg/freeze-v1.3.0.md`; document the 13-contract freeze; bump `last_updated`
- [ ] In `.omne/cfg/mcp.md` (SSOT): document `SyncParams::pack` field + `resolved_pack_root()` precedence rule; bump `last_updated`
- [ ] In `.omne/cfg/walker.md` (SSOT): document `ExecCtx::pack` additive field; note that `workspace` rename is gated to v2; bump `last_updated`
- [ ] In `.omne/cfg/plugin-api.md` (SSOT): add UNSTABLE WARNING callout at the top of the doc; document v1.4.0 freeze candidate status; bump `last_updated`
- [ ] Run `.omne/scripts/validate.py` — exit 0 across all 5 updated docs
- [ ] Commit + push in SSOT repo (separate from grex per Rule 7)

## Stage 3 — local gates (full pre-push sequence per cfg/workflow.md Phase 2 Step 7)
- [ ] `cargo fmt --all -- --check` exit 0
- [ ] `cargo build --workspace` green
- [ ] `cargo test --workspace` (excluding `dispatch_parallel.rs` per pre-existing UAC issue)
- [ ] Verify NO regression: 380+ existing lib tests pass; v1.2.6 cap-std + TreeError split tests pass; v1.2.5 quarantine GC/restore tests pass; v1.2.4 cancellation tests pass; v1.2.3 `e2e_cycle_aborts` pass; v1.2.2 `same_repo_two_refs_no_cycle` pass; cycle_self_loop_aborts/three_node/four_node/nested_prefix/diamond all pass; v1.2.4 `e2e_v1_3_0_readiness_smoke` pass (with new warn-once assertion)
- [ ] `cargo doc --no-deps --workspace -D warnings` clean (verify Plugin-API UNSTABLE WARNING renders correctly in `grex-plugins-builtin` docs)
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cd proof && lake build` green (NO new theorem expected; verify pre-existing 5 headline theorems still pass)
- [ ] axiom counts unchanged from v1.2.6 baseline (Bridge ≤ 12, Types = 4, Other = 0)
- [ ] Release-build sanity: `cargo build --release --workspace` exit 0
- [ ] `git status` clean — no fossil drift visible (v1.2.6 .gitignore + .gitattributes + cleanup script holding)
- [ ] Manual smoke: `target/release/grex sync --workspace <p>` shows deprecation warn; `target/release/grex sync --pack <p>` does not; `target/release/grex ls --json` shows both `workspace` and `pack` keys

## Stage 4 — review pass (parallel + Codex per cfg/workflow.md Phase 3, discipline 14 dispatch)
- [ ] **Smaller wave than v1.2.6** — 4-5 parallel subagent reviewers (fewer surfaces touched; no concurrent algorithm; no Lean obligation):
  - CLI rename correctness reviewer (clap alias on all 4 verbs; warn-once `OnceLock` semantics; doc-comment doc-noun consistency; manpage diff is rename-only)
  - JSON dual-emit + MCP precedence reviewer (envelope key order stability; serde Serialize derive correctness; `pack.or(workspace)` precedence matches doc; back-compat preserved for `workspace`-only callers)
  - Behavior contract freeze table reviewer (13 contracts cover the surfaces they claim; SemVer-class column accurate; Plugin-API exclusion documented; cross-links from `cfg/api-contract.md` valid)
  - SSOT doc reviewer (freeze + migration NEW docs G2-frontmatter compliant; routing-table updated; INDEX.yaml regenerated; existing docs cross-link correctly; 5 existing docs updated per Round 3 gap list)
  - SemVer + BC reviewer (additive everywhere; no public API removal; deprecation deferrals documented for v1.4.0; MSRV unchanged; CHANGELOG accurate)
- [ ] Apply review fix-ups (separate workers — never the original writers per discipline 14)
- [ ] Codex rescue second pass; skip if no return
- [ ] Re-run gates after fix-ups

## Stage 5 — commit + PR + merge (per cfg/workflow.md Phase 4)
- [ ] Conventional Commit (NO Co-Authored-By per discipline 13): `feat(v1.3.0): --workspace → --pack rename + 13-contract freeze + MINOR cut`
- [ ] git push origin feat-v1.3.0
- [ ] gh pr create --base main --head feat-v1.3.0
- [ ] Watch CI: `gh pr checks <num> --watch --interval 30`
- [ ] After CI green: `gh pr merge <num> --squash --delete-branch`
- [ ] Local: git checkout main; git pull

## Stage 6 — ship (cargo publish + tag)
- [ ] cargo publish topo: grex-core → grex-mcp + grex-plugins-builtin (parallel) → grex-cli (use `--allow-dirty` if `crates/grex/.grex/events.jsonl` runtime artifact reappears post-cleanup-script — same pattern as v1.2.4/5/6)
- [ ] git tag -a v1.3.0 -m "v1.3.0 — --workspace → --pack CLI rename + 13-contract freeze (MINOR)" <merge-commit>
- [ ] git push origin v1.3.0

## Stage 7 — wrap-up (per cfg/workflow.md Phase 5)
- [ ] Append `## Endpoint (2026-05-XX, main — v1.3.0 SHIPPED MILESTONE)` to progress.md
- [ ] Update top "Where we are" block — promote v1.3.0 from IN FLIGHT to SHIPPED MILESTONE
- [ ] Promote draft entry in `.omne/cfg/history.md` to SHIPPED MILESTONE with date + commit SHA (separate SSOT repo per Rule 7)
- [ ] Verify SSOT freeze + migration docs published in SSOT repo (separate from grex commit + push)
- [ ] Commit progress.md (grex) + history.md (SSOT)
- [ ] Carry-forward list to v1.4.0:
  - `PackLock::acquire` synchronous variant REMOVAL (deprecation warn shipped v1.3.0)
  - `Scheduler::permits()` REMOVAL
  - `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const REMOVAL
  - Plugin-API freeze + Plugin-API UNSTABLE marker removal
- [ ] Carry-forward list to v2:
  - `--workspace` CLI flag removal (alias stays through v1.x)
  - `ExecCtx::workspace` field removal (sibling `pack` keeps working)
  - `workspace-sync` lock tier rename
- [ ] Verify v1.3.0 readiness goal met: existing `e2e_v1_3_0_readiness_smoke` pass + warn-once extension pass + freeze table published

## Out of scope (defer to v1.4.0+)
- Removing deprecated symbols (PackLock::acquire sync, Scheduler::permits, DEFAULT_MANAGED_GITIGNORE_PATTERNS) → v1.4.0
- Plugin-API freeze → v1.4.0
- `workspace-sync` lock tier rename → v2
- `ExecCtx::workspace` field rename (only ADD `pack` additive in v1.3.0) → v2
- ChildPath validator changes → out (v1.3.0 freezes existing semantics as C9)
- New Lean theorem → Rule 8 simple exemption (v1.3.0 has zero new algorithmic behavior)
- SSOT v2 (owners.yaml, topic-reorg cfg/, lib/cfg dedup, history.md aggregator) → SSOT roadmap, separate repo
