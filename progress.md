# progress — grex

## Where we are
M0/M1/M2/M2-hardening/M3 Stage A + Stage B + **M3 review series** all shipped to `main`. **M3 complete + hardened (2026-04-20)**: parse layer + variable expansion + validator framework + git backend + pack tree walker + dual executors (Plan/Fs) + `grex sync` verb + 5 review-driven fix PRs. Main head `7ce186e`. M4 (plugin system) next.

## Last endpoint (2026-04-20, plan/M4)
- **Plan/M4 endpoint**: M4 Stage A-E scope locked, `milestone.md` M4 rewritten (plugin system), `openspec/feat-grex/spec.md` M4 section appended, `.omne/cfg/plugin-api.md` gaps filled (`Registry`, `register_builtins`, idempotency, `plugin-inventory` flag). Branch `plan/m4-plugin-system`. Next: M4-A implementation PR.

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
**344 tests** all green on `main` (316 pre-review + 28 from fix PRs).

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
Implement **M4-A** (ActionPlugin trait + Registry struct): move 7 built-ins behind trait; executor dispatch via registry lookup. Branch off `main` after plan PR merges.

Stage order reminder: A → B → C → D → E.
- A: `ActionPlugin` trait + `Registry` struct + `register_builtins()`; 7 built-ins behind trait.
- B: Lockfile `actions_hash` compute + compare → `ExecResult::Skipped` emission.
- C: `reg_key` + `psversion` real probes (replace stubs).
- D: CLI `--ref`, `--only <pattern>`, lockfile read/write formalized.
- E: Discovery hook (`inventory` behind `plugin-inventory` feature); v2 foundation.

See `.omne/cfg/m3-review-findings.md` for the M3 review-series master finding list and mapping table (finding → PR → resolution).
