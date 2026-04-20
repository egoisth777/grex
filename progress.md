# progress — grex

## Where we are
M0/M1/M2/M2-hardening/M3 Stage A + Stage B all shipped to `main`. **M3 complete (2026-04-20)**: parse layer + variable expansion + validator framework + git backend + pack tree walker + dual executors (Plan/Fs) + `grex sync` verb. Main head `d160c7c feat(m3-b6): grex sync verb`. M4 (plugin system) next.

## Last endpoint (2026-04-20)
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

## Architecture state (post-M3)
- `grex-core` modules: `pack`, `vars`, `git`, `tree`, `execute`, `pack::validate`, `sync`.
- 2 executor impls (`PlanExecutor`, `FsExecutor`) share `ActionExecutor` trait — interchangeable by value.
- 2 validator traits: `Validator` (per-manifest) + `GraphValidator` (per-graph).
- `Walker` + `FsPackLoader` + `GixBackend` + validators + executors composed in `sync::run()`.
- DFS post-order traversal (children installed before parent).

## Test status
316 tests all green on `main` (up from 180 pre-M3; +136 across Stage A + Stage B).

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

## Open questions
- crates.io name `grex` likely taken (real package: regex tool). Fallbacks: `grex-cli`, `grex-rm`, scoped `@grex-org/cli`. Check at v0.1.0 publish.
- Windows mandatory `ManifestLock` — needs `append_event_on_fd` API refactor (deferred from M2 hardening).
- Coverage threshold raise 60→80% as M3+ adds tests.
- Semver baseline at v0.1.0 publish.
- Lockfile `actions_hash` field name kept (not renamed to `content_hash`) — revisit at M4 when plugins land.
- `on_fail: ignore` (exec) vs `skip` (require) — confirmed distinct; keep split.
- `reg_key` / `psversion` predicates are conservative stubs returning false — upgrade to real probes in M4.
- Lockfile idempotency skip (via `actions_hash` compare) deferred from m3-b6 — M4 concern.

## Files to read for 0-state hop-in
1. `CLAUDE.md`
2. `progress.md` (this file)
3. `milestone.md`
4. `openspec/feat-grex/spec.md`
5. `.omne/cfg/README.md`

## Next action
Start **M4 (plugin system)**. Scope to be refined against `.omne/cfg/` + `milestone.md`:
- Custom action plugins (Tier 2+ actions beyond the 7 Tier 1).
- Lockfile `actions_hash` idempotency skip.
- Plugin discovery + loading.
- Possibly: `reg_key` / `psversion` real probes (currently stubs).
- CLI: `--ref` override, `--only <pattern>`, lockfile read/write.
