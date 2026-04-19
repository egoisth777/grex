# progress — grex

## Where we are
M1 scaffold underway; pending commit.

## Last endpoint (2026-04-19)
- 2026-04-19 M1 scaffold in progress: cargo workspace + clap stubs + CI + LICENSE
- Pack taxonomy locked: `meta`, `declarative`, `scripted` (3 built-in pack-types).
- Tier 1 action vocabulary locked at 7 primitives: `symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`.
- Action vocab grounded in real-world E:\repos scan (3 PowerShell scripts, 945 LOC) — frequencies drove the shortlist.
- Plugin trait APIs spec'd: `ActionPlugin`, `PackTypePlugin`, `Fetcher`. In-process registry v1; external loading v2.
- `grex-inst` fixture/reference repo created + pushed (first-party example consumer of the pack protocol).
- CLI verb surface frozen at 12: `init add rm ls status sync update doctor serve import run exec`.
- Manifest split: `grex.jsonl` intent log + `grex.lock.jsonl` resolved state, both JSONL + atomic temp+rename.
- Sync-as-universal-op principle adopted: every pack inherits `grex sync` for free.
- Lean4 scope for v1 narrowed to one invariant: `Grex.Scheduler.no_double_lock`.
- Embedded MCP stdio JSON-RPC confirmed (not subprocess wrapper).
- TUI, external plugins, non-git fetchers, additional action/pack-types pushed to v2.

## Decisions locked
- Pack = git repo + `.grex/` contract dir; outside `.grex/` is opaque.
- Every pack is a meta-pack (uniform model, zero-children = leaf).
- Repo sync is universal; install/update/teardown is per-pack-type.
- 3 built-in pack-types, 7 built-in actions, both extensible via in-process plugin registry.
- Manifest is append-only JSONL; lockfile separate JSONL; both atomic-rename on compact.
- Scheduler = tokio runtime + bounded semaphore + per-pack `.grex-lock` + `fd-lock` for manifest.
- Embedded MCP server; methods mirror CLI verbs 1:1.
- Lean4 proves exactly one scheduler invariant in v1.
- Plugins v1 are in-process Rust modules registered at startup; v2 opens dylib/WASM.
- v1 ships without TUI, external plugin loading, or non-git fetchers.

## Open questions
- crates.io name: `grex` likely taken. Fallbacks: `grex-cli`, `grex-rm`, scoped `@grex-org/cli`.
- Git backend: `git2` (libgit2 bindings, proven) vs `gix` (pure Rust, faster, less mature). Decision by M3.
- License: MIT vs Apache-2.0 vs dual. Decision by M7.
- Plugin trait ABI versioning strategy for v2 (semver on trait crate? ABI hash? inventory slot schema?).

## Files to read for 0-state hop-in
1. `CLAUDE.md`
2. `progress.md` (this file)
3. `milestone.md`
4. `openspec/feat-grex/spec.md`
5. `.omne/cfg/README.md`

## Next action
Review M1 scaffold, run cargo check, commit, then start M2 (manifest + lockfile).
