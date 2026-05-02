---
slug: feat-v1.3.0-cli-rename-freeze
type: spec
status: active
last_updated: 2026-05-02
---

# feat-v1.3.0 — CLI rename (`--workspace` → `--pack`) + behavior contract freeze + MINOR cut

**Status**: draft
**Milestone**: v1.3.0 (MINOR per maintainer Rule 6 — additive: clap alias adds `--pack` alongside existing `--workspace`; JSON envelopes dual-emit `workspace` AND new `pack` keys; MCP `SyncParams` adds optional `pack` field; SSOT freezes 13 STABLE behavior contracts; Plugin-API stays UNSTABLE; zero deprecated-symbol removal — all v1.4.0 carry-forwards)
**Depends on**: v1.2.6 (SHIPPED 2026-05-02 — main @ `5bb2520`, tag `v1.2.6`, all 4 crates @ 1.2.6 live on crates.io). v1.2.x stabilization complete; v1.3.0 takes the rename + contract-freeze cut.
**Branch**: feat-v1.3.0

## Why now

v1.2.x finished stabilization. The next cut is the long-planned `--workspace` → `--pack` ergonomic rename + the behavior contract freeze that v1.3.0 has been earmarked for since v1.2.4. Bundling them into one MINOR release is preferable to two: the rename touches the same surfaces the freeze documents, so doing them together keeps the SSOT freeze table accurate at first publication.

- **CLI `--workspace` → `--pack` rename (carry-forward since v1.0)** — The CLI exposes `--workspace <path>` on `sync`, `serve`, `migrate-lockfile`, `teardown`. The internal noun has shifted from "workspace" to "pack" everywhere except the operator surface (cfg/cli.md §rename-tracker, walker.md §pack-noun). Operator-side rename has been gated until a MINOR cut to preserve SemVer discipline. v1.3.0 ships the rename via clap's `alias` mechanism: `--pack` becomes the canonical flag; `--workspace` continues to work and emits a one-time deprecation warn (warn-once per process). Same dual-emit treatment for JSON envelopes (ls/doctor) and MCP `SyncParams`.

- **Behavior contract freeze (v1.3.0 readiness goal)** — 13 STABLE behavior contracts identified across Round 1-3 of the v1.3.0 arch review. Freezing them in `.omne/cfg/freeze-v1.3.0.md` (4-column table: contract, surface, semver-class, owner) gives downstream Rust consumers + plugin authors + operators a stable target to code against. Plugin-API stays UNSTABLE deliberately — v1.4.0 freezes it once cap-std `Dir` capability handles harden plugin sandboxing.

- **Migration doc** — `.omne/cfg/migration-v1.3.0.md` ships alongside the freeze table with two operator-facing sections (CLI flag rename + JSON envelope key migration) and one consumer-facing section (Rust API additive surface for `pack` field on `ExecCtx` + `SyncParams`).

Each item is small and additive at every layer (CLI, JSON wire, MCP wire, file format, behavior). Together they satisfy the v1.3.0 readiness AC defined in v1.2.4 and confirmed across v1.2.5/6.

## Scope

Locked Round 4 design (synthesizing R1+R2+R3):

1. **CLI rename via clap alias** — `--workspace` becomes a hidden alias of `--pack` on the 4 arg structs (`SyncArgs`, `ServeArgs`, `MigrateLockfileArgs`, `TeardownArgs`). Doc-strings rewritten to use `<pack>` as the noun. Manpages regenerated.
2. **Doc-noun rename in CLI surface** — every `<workspace>` placeholder in `--help` text + manpages becomes `<pack>`. Internal struct fields stay `workspace` (additive `pack` alias only at flag level).
3. **JSON dual-emit** — `grex ls --json` and `grex doctor --json` envelopes emit BOTH `workspace` and `pack` keys with identical values. Order: `workspace` first (preserves diff-friendly stability for existing consumers); `pack` second.
4. **MCP `pack` param** — `SyncParams` adds optional `pack: Option<PathBuf>` alongside existing `workspace: Option<PathBuf>`. Precedence: `pack.or(workspace)` — if both present, `pack` wins (caller responsibility to not send both); if only `workspace` present, value used (back-compat); if neither, current default.
5. **Behavior contract freeze** — 13 STABLE contracts frozen per Round 2 table:
   - C1: `pack.yaml` schema (additive only post-freeze)
   - C2: `grex.lock.jsonl` schema (additive only)
   - C3: `events.jsonl` event variants (additive only — new variants under existing pattern)
   - C4: `TreeError` variant set (additive only — `#[non_exhaustive]` since v1.2.0)
   - C5: `--workspace` / `--pack` CLI flag set on 4 verbs
   - C6: JSON envelope keys (`workspace` + `pack` dual-emit; additive new keys allowed)
   - C7: MCP `SyncParams` field set (additive only — new fields allowed)
   - C8: `ExecCtx::workspace` field (additive `pack` shadow allowed; rename gated to v2)
   - C9: ChildPath validation rules (no semantic change without MAJOR)
   - C10: lockfile sentinel files (`.grex/`, `pack-id.txt`, `head.txt`)
   - C11: `--retain-days` semantics on `grex doctor` (no behavior change without MAJOR)
   - C12: cap-std root capability semantics on walker FS surface (cap-std runtime contract = stable)
   - C13: Quarantine GC + restore semantics (v1.2.5 semantics frozen)
   - **Plugin-API explicitly stays UNSTABLE** — v1.4.0 freeze candidate.
6. **Deprecation deferrals to v1.4.0** — `PackLock::acquire` (sync variant), `Scheduler::permits`, `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const removal all DEFERRED. v1.3.0 lands warn-once deprecation diagnostics; v1.4.0 deletes.
7. **NEW SSOT files** — `.omne/cfg/freeze-v1.3.0.md` (4-column freeze table) + `.omne/cfg/migration-v1.3.0.md` (operator + Rust consumer migration doc). Ship through SSOT repo per Rule 7.
8. **Plugin-API UNSTABLE marker** — add UNSTABLE callout to `plugin/mod.rs` lib doc-comment, `Cargo.toml` description suffix on `grex-plugins-builtin`, and `.omne/cfg/plugin-api.md` WARNING callout.
9. **e2e smoke extension** — extend existing `e2e_v1_3_0_readiness_smoke` (added v1.2.4) with deprecation warn-once assertion: invoke `grex sync --workspace <path>` twice, assert deprecation diagnostic emitted exactly once.
10. **MSRV unchanged** — workspace MSRV stays 1.79 per Cargo.toml:14. No language-feature dependency added.

## Out of scope

The following items remain deferred to v1.4.0 or later:

- **Removing deprecated symbols** (PackLock::acquire sync, Scheduler::permits, DEFAULT_MANAGED_GITIGNORE_PATTERNS) → v1.4.0. Warn-once shipped in v1.3.0; deletion is the MAJOR-feeling MINOR (still PATCH-compatible if downstream uses public API only) work for the next cycle.
- **Plugin-API freeze** → v1.4.0. Plugin-API stays UNSTABLE in v1.3.0 to preserve refactoring runway as cap-std `Dir` capabilities harden the sandbox.
- **`workspace-sync` lock tier rename** → v2 (semantically a rename of an internal lock-name string, but it surfaces in lockfile diagnostics — gated to v2).
- **`ExecCtx::workspace` field rename** → v2. v1.3.0 ADDS `pack` as additive sibling field only; both coexist until v2 removes `workspace`.
- **ChildPath validator changes** — out of scope; v1.3.0 freezes existing semantics (C9), no behavior change.
- **New Lean theorem** — Rule 8 simple exemption applies: v1.3.0 has zero new algorithmic behavior; the rename is pure surface; the dual-emit is pure JSON serialization; the MCP precedence is `Option::or` with documented winner. No concurrency invariant. No new algorithmic obligation. Document the exemption explicitly in `tasks.md` Stage 1.

No public API removal. New CLI flag (`--pack`) is additive alias; new JSON keys (`pack` in envelopes) are additive; new MCP field (`pack` in SyncParams) is additive; new SSOT docs (freeze + migration) are net-add. All additive at API/wire/file-format/behavior layers.

## Acceptance bar

1. **Lean obligations green (no new theorem required).**
   - All existing theorems still green: `sync_meta_no_cycle_infinite_clone` (v1.2.2/3), `cancellation_terminates_promptly` (v1.2.4), `partial_clone_cleanup_idempotent` (v1.2.5), `pool_deadlock_guard_terminates` (v1.2.5), `walker_subpath_resolution_bounded_by_meta_dir` (v1.2.6).
   - Axiom counts unchanged: Bridge ≤ 12 (target ZERO new axioms — v1.3.0 has zero algorithmic change), Types = 4, Other = 0.
   - Rule 8 "simple" exemption documented in `tasks.md` Stage 1 with explicit per-item justification.
2. **CLI rename regression test.** `cli_workspace_pack_alias_parses_both` — assert `grex sync --workspace foo` and `grex sync --pack foo` produce identical clap-parsed `SyncArgs`. Run for all 4 verb arg structs.
3. **CLI deprecation warn-once test.** `cli_workspace_deprecation_warns_once_per_process` — invoke `grex sync --workspace foo` twice in a single process; assert deprecation diagnostic emitted exactly once.
4. **JSON dual-emit test.** `cli_ls_doctor_json_envelopes_dual_emit_workspace_pack` — assert `grex ls --json` and `grex doctor --json` envelopes contain BOTH `"workspace": "<v>"` and `"pack": "<v>"` with identical values; key order: `workspace` first, `pack` second.
5. **MCP precedence test.** `mcp_sync_pack_or_workspace_precedence` — for SyncParams payloads `{pack: Some, workspace: Some}` (pack wins), `{pack: Some, workspace: None}` (pack wins), `{pack: None, workspace: Some}` (workspace wins back-compat), `{pack: None, workspace: None}` (default), assert resolved path matches precedence rule.
6. **Plugin-API UNSTABLE marker visible.** `cargo doc --no-deps -p grex-plugins-builtin` produces docs containing the UNSTABLE WARNING in the lib-level doc-comment; `cargo metadata` shows the description suffix.
7. **v1.3.0 readiness smoke extended.** Existing `e2e_v1_3_0_readiness_smoke` (v1.2.4) extended with the warn-once assertion; meta-pack + 1 sub-pack acyclic sync continues to return `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`.
8. **No regression on the 380+ existing unit tests + the v1.2.4 cancellation test + the v1.2.3 e2e cycle test + the v1.2.5 quarantine GC/restore tests + the v1.2.6 cap-std + TreeError split tests.**
9. **Local gates clean:** `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cd proof && lake build`, axiom-policy check (Bridge ≤ 12 unchanged).
10. **CI gate.** Existing `#print axioms` smoke check covers the 5 headline theorems; v1.3.0 adds nothing new (no new theorem). Drift fails CI.
11. **Versioning.** Workspace version 1.2.6 → 1.3.0 in workspace `Cargo.toml`, `crates/xtask/Cargo.toml` path-dep pin, `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION`. Man pages regenerated via `cargo xtask gen-man` — expect `<workspace>` → `<pack>` doc-noun changes in all 4 affected manpages.
12. **Changelog/history.** `CHANGELOG.md [1.3.0]` entry + `.omne/cfg/history.md` v1.3.0 MILESTONE entry (separate SSOT repo per Rule 7) + `.omne/cfg/freeze-v1.3.0.md` + `.omne/cfg/migration-v1.3.0.md` new files (also SSOT repo).

## v1.3.0 readiness constraint (maintainer directive — now satisfied)

This release IS the v1.3.0 cut. The constraint codified in v1.2.4 and tracked through v1.2.5/6 — "sub-pack under meta-pack flow MUST remain functional, basic action commands MUST NOT be broken, e2e_v1_3_0_readiness_smoke continues to pass" — is satisfied by:

- Sub-pack under meta-pack flow: unchanged (no walker logic touched in v1.3.0).
- Basic action commands (`grex sync`, `grex ls`, `grex doctor`, `grex migrate-lockfile`): unchanged behavior; only flag-name surface changes (clap alias is back-compatible).
- `e2e_v1_3_0_readiness_smoke`: extended with warn-once assertion; baseline assertions preserved.

v1.2.x → v1.3.0 roadmap (final):

- v1.2.4: cancellation + polish + tests + axiom CI (SHIPPED 2026-05-02)
- v1.2.5: A2 partial-clone cleanup + A3 pool deadlock guard + quarantine GC/restore + retention policy (SHIPPED 2026-05-02)
- v1.2.6: TreeError variant split + cap-std snapshot hardening + stale manifest doc + working-tree drift root cause (SHIPPED 2026-05-02)
- **v1.3.0: `--workspace` → `--pack` CLI rename + behavior contract freeze + MINOR cut (THIS PR)**
- v1.4.0: deprecated-symbol removal + Plugin-API freeze candidate

## SemVer

MINOR (1.2.6 → 1.3.0) per maintainer Rule 6. All changes additive at API/wire/file-format/behavior layers:

- New CLI flag (`--pack`) added as alias; existing `--workspace` flag preserved with warn-once deprecation.
- New JSON envelope key (`pack`) added alongside existing `workspace` (dual-emit).
- New MCP `SyncParams` field (`pack`) added alongside existing `workspace`.
- New SSOT docs (freeze table + migration guide) are net-add, no removal.
- 13 behavior contracts FROZEN (locks future change to additive-only without MAJOR).
- Plugin-API explicitly UNSTABLE — no contract added or removed.
- Zero deprecated-symbol removal (deferred to v1.4.0).
- Zero algorithmic behavior change (Rule 8 simple exemption applies).

Net SemVer impact: PATCH-compatible runtime + MINOR-class API additions (new clap alias, new MCP field, new SSOT docs).

Reviewer note: if a reviewer flags any item as MAJOR (e.g. argues that the dual-emit adds JSON keys that violate strict-schema consumers), surface to maintainer per Rule 6 — `pack` key is additive (existing consumers reading `workspace` continue to work; `pack` is opt-in for new consumers). Maintainer decides label.

## Process gates (per cfg/workflow.md)

Order of operations is fixed:

1. Phase 1 — OpenSpec (this proposal triplet).
2. Phase 2 — NO Lean obligation (Rule 8 simple exemption documented in `tasks.md` Stage 1). Rust impl proceeds directly.
3. Phase 3 — review (parallel reviewers + Codex pass — smaller wave: fewer surfaces touched than v1.2.6).
4. Phase 4 — PR + merge.
5. Phase 5 — wrap-up (CHANGELOG, SSOT history MILESTONE entry, freeze + migration doc publish, crates.io publish, tag).

A code change that lands without the documented Rule 8 exemption is a process violation. Per discipline 13, commits MUST NOT carry a `Co-Authored-By` trailer.
