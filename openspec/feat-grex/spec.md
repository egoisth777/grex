# feat-grex — Pack-based cross-platform dev-env orchestrator

## Problem

Dev-environment setup across machines is fragmented: a handful of config repos, per-tool install scripts, hand-written PowerShell/bash for symlinks + env vars, no uniform sync story, no lockfile, no agent-native control surface. Git submodules are brittle. Package managers install tools but don't orchestrate config placement, declarative env state, or multi-repo sync. No existing OSS tool combines a **pack abstraction** (git repo + contract dir) with declarative actions, a lockfile, and embedded MCP for agents.

## Goal

Ship a Rust CLI `grex` that orchestrates **packs** — git repos bearing a `.grex/` contract directory — across Windows, Linux, and macOS. Deliver unified repo sync, three built-in pack-types, seven built-in cross-platform actions, a Lean4-verified scheduler invariant, and an embedded MCP JSON-RPC server. Keep the core extensible: new action types and pack-types plug in via Rust traits with zero grex-core recompile for authors writing their own plugin modules, and (v2) zero recompile for third-party dylib/WASM plugins.

## Non-goals

- Monorepo conversion tooling.
- Full git submodule replacement (only covers "sub-repo fetch/sync"; does not reproduce `.gitmodules` semantics end-to-end).
- Non-git fetchers (rclone, S3, OCI) in v1 — deferred to v2.
- Retro-futurist TUI dashboard in v1 — deferred to v2.
- Language-specific build orchestration.
- Generic CI runner.

## Success criteria

1. `grex init`, `add`, `rm`, `sync`, `update`, `doctor`, `import` produce expected manifest + lockfile + on-disk state across Windows, Linux, and macOS (CI matrix integration tests).
2. `grex serve --mcp` responds to JSON-RPC `grex.init`, `grex.add`, `grex.sync`, `grex.status`, `grex.ls`, `grex.rm`, `grex.update`, `grex.doctor`, `grex.import`, `grex.run`, `grex.exec` methods 1:1.
3. A `declarative` pack exercising all 7 action types installs correctly on each of the three OSes.
4. A `scripted` pack runs `.grex/hooks/setup.{sh,ps1}` on the matching OS and no-ops on the others.
5. A `meta` pack with nested children syncs the tree recursively in parallel under the `--parallel N` bound.
6. Manifest + lockfile round-trip survives crash injection (SIGKILL / TerminateProcess mid-write) and torn lines are discarded on next load.
7. Lean4 proof `Grex.Scheduler.no_double_lock` compiles cleanly under `lake build` with zero `sorry` / zero unresolved `axiom` in deliverable scope.
8. `pack.yaml` has `schema_version: "1"`; v1 packs remain readable by future v2 without breaking.
9. `ActionPlugin`, `PackTypePlugin`, `Fetcher` traits are documented and used internally by every built-in plugin (proof of extensibility by construction).

## Architecture summary

Single crate `grex` (bin + lib). CLI verbs dispatch into the library. Data flow: CLI parse → manifest load (fold JSONL events) → pack tree walk (parse `.grex/pack.yaml` under each registered path, recurse through children) → pack-type plugin dispatch (`install`/`update`/`teardown`/`sync`) → action plugin registry execution → lockfile write → gitignore managed-block sync. Concurrency: tokio multi-thread runtime with a bounded semaphore, per-pack `.grex-lock` file via `fd-lock`, global manifest lock. Extensibility: in-process trait-object registries for `ActionPlugin` + `PackTypePlugin` + `Fetcher`; v2 adds external loading. Full module layout in [../../.omne/cfg/architecture.md](../../.omne/cfg/architecture.md).

## Out of scope v1

- External plugin loading (dylib / WASM).
- Retro-futurist TUI.
- Non-git fetchers (rclone, S3, OCI, HTTP).
- Pack-types beyond the 3 built-ins.
- Actions beyond the 7 Tier 1 primitives.
- Hosted pack registry (`grex.dev`).
- Self-update command.

## Dependencies

**Rust crates:**

| Crate | Purpose |
|---|---|
| `tokio` | async runtime |
| `clap` | CLI parsing |
| `serde`, `serde_yaml`, `serde_json` | schema I/O |
| `simd-json` (optional feature) | fast manifest fold |
| `gix` or `git2` | git operations (choice at M3) |
| `fd-lock` | cross-platform file locking |
| `anyhow` | binary error propagation |
| `thiserror` | typed library errors |
| `tracing`, `tracing-subscriber` | structured logs |
| `comfy-table` | `ls`/`status` tables |
| `owo-colors` | ANSI color w/ TTY detect |
| `async-trait` | async trait objects |
| `inventory` | plugin registration |

**Dev dependencies:**

| Crate | Purpose |
|---|---|
| `proptest` | property tests |
| `assert_cmd` | CLI integration tests |
| `tempfile` | test fixtures |

**External binaries:**

- `git` CLI (fallback for operations not covered by chosen Rust backend).
- OS symlink APIs (via `std::os::{unix,windows}::fs`).
- `lake` + `lean` (CI-only, for proof job).

## Acceptance

All success-criteria items PASS in the GitHub Actions matrix (Windows + Ubuntu + macOS × stable + beta toolchains). Lean4 `.olean` builds clean. `cargo install grex` works from crates.io on all three OSes. At least one reference pack repo (`grex-inst` or successor) is published as an installable example.
