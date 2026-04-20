# progress — grex

## Where we are
M0/M1/M2/M2-hardening shipped to `main`. On `feat/m3-pack-sync` (cut from main, no commits) ready to start M3 Stage A.

## Last endpoint (2026-04-19)
- PR #1 merged — M1 scaffold: cargo workspace + clap skeleton + 78 tests + CI.
- PR #2 merged — M2 manifest + lockfile JSONL + atomic fs + fd-lock; 174 tests; adversarial review applied.
- PR #3 merged — M2 hardening: 4 src fixes + 10 CI quality gates; 180 tests, 119 in grex-core.

## Test status
180 tests / 20 suites / ~30s, all green on `main`.

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

## Open questions
- crates.io name `grex` likely taken (real package: regex tool). Fallbacks: `grex-cli`, `grex-rm`, scoped `@grex-org/cli`. Check at v0.1.0 publish.
- Windows mandatory `ManifestLock` — needs `append_event_on_fd` API refactor (deferred from M2 hardening).
- Coverage threshold raise 60→80% as M3+ adds tests.
- Semver baseline at v0.1.0 publish.

## Files to read for 0-state hop-in
1. `CLAUDE.md`
2. `progress.md` (this file)
3. `milestone.md`
4. `openspec/feat-grex/spec.md`
5. `.omne/cfg/README.md`

## Next action
Start **M3 Stage A**: define `grex-core::pack` module — `PackManifest`, `PackType` (meta/declarative/scripted), `Action` enum (7 Tier 1 primitives), serde-yaml parse, round-trip tests. Pure logic, no git/I/O.

## After M3 Stage A
**M3 Stage B**: gix integration + sync engine + recursive pack tree walk + cycle detection + wire into `grex sync` verb.
