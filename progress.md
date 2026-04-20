# progress — grex

## Where we are
M0/M1/M2/M2-hardening/M3 Stage A + Stage B + **M3 review series** + **M4-A + M4-B** on `feat/m4-a-plugin-trait`. **M3 complete + hardened (2026-04-20)**: parse layer + variable expansion + validator framework + git backend + pack tree walker + dual executors (Plan/Fs) + `grex sync` verb + 5 review-driven fix PRs. Main head `7ce186e`. M4-B shipped on branch (2026-04-20): dispatch via `registry.get(action.name())`, `actions_hash` compute + compare, `ExecResult::Skipped { pack_path, actions_hash }` emission, `ActionLogger` + `EnvResolver` traits defined. 344 → **361 tests**, all green. M4-C (registry probes: `reg_key` Windows winreg, `psversion` PowerShell) next.

## Last endpoint (2026-04-20, feat/m4-a-plugin-trait — M4-B shipped)
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
**361 tests** all green on `feat/m4-a-plugin-trait` (344 post-review + 17 from M4-A/M4-B streams: plugin registry bootstrap, actions_hash, ActionLogger/EnvResolver traits, executor registry-dispatch paths, Skipped reshape).

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
- `reg_key` / `psversion` predicates are conservative stubs returning false — upgrade to real probes in M4.
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
**M4-C (registry probes: `reg_key` Windows winreg, `psversion` PowerShell)**. Replace the conservative-false stubs flagged in M3 open questions. Scope per `milestone.md` M4 Stage C: real `winreg` crate reads on Windows (`RegOpenKeyEx` + `RegQueryValueEx`); `powershell.exe -NoProfile -Command` for `$PSVersionTable.PSVersion` probe; non-Windows returns `PredicateNotSupported` error. Branch off M4-B merge once landed on `main`. Commit SHA plumbing from `PackNode` (to feed real commit_sha into `compute_actions_hash`) is M4-D — tracked as a carry-forward.

Stage order reminder (updated 2026-04-20): A → B → C → D → E.
- A: `ActionPlugin` trait + `Registry` struct + `register_builtins()`; 7 built-ins behind trait; re-exports; plugin-layer unit tests. Dispatch unchanged.
- B: Executor dispatch refactor (swap direct `match Action` in `FsExecutor` / `PlanExecutor` for `registry.get(name)`) + lockfile `actions_hash` compute + compare → `ExecResult::Skipped` emission.
- C: `reg_key` + `psversion` real probes (replace stubs).
- D: CLI `--ref`, `--only <pattern>`, lockfile read/write formalized.
- E: Discovery hook (`inventory` behind `plugin-inventory` feature); v2 foundation.

See `.omne/cfg/m3-review-findings.md` for the M3 review-series master finding list and mapping table (finding → PR → resolution).
