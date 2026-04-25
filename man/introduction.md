# Introduction

`grex` is a pack-based, agent-native, Rust-fast orchestrator for cross-platform
dev-environment setup. It turns machine bootstrap into a declarative,
reproducible graph of git-backed **packs** that can be synced, updated, and
torn down with a single uniform command surface — from a shell, from CI, or
from an LLM-driven agent speaking MCP.

## What `grex` gives you

- A **single CLI surface** (12 frozen verbs) covering `init`, `add`, `rm`,
  `ls`, `status`, `sync`, `update`, `doctor`, `serve`, `import`, `run`, and
  `exec`. Universal flags `--json`, `--plain`, `--dry-run`, `--parallel <N>`,
  and `--filter <EXPR>` apply on every verb — see
  [CLI reference](./reference/cli.md).
- A **declarative pack contract** — any git repo plus a `.grex/` directory
  with a `pack.yaml` becomes a grex pack. Three built-in pack-types cover
  the common cases; a plugin trait lets you add more without patching the
  core. See [pack model](./concepts/pack-spec.md) and [plugin API](./reference/plugin-api.md).
- A **reproducible manifest** — `grex.jsonl` captures intent, `grex.lock.jsonl`
  captures resolved state, both newline-delimited JSON with a `schema_version`
  field on every row. Full details in [manifest](./concepts/manifest.md).
- An **embedded MCP server** — `grex serve` speaks native MCP 2025-06-18 over
  stdio. Every non-`serve` verb is a tool call. No custom JSON-RPC dialect,
  no `grex.*` namespace, no batching. See [MCP server](./reference/mcp.md).
- **Parallel execution with a Lean4-verified invariant** — the scheduler
  holds a bounded semaphore, a per-pack `.grex-lock`, and an `fd-lock`
  manifest guard. Invariant I1 (no double lock) is mechanised in Lean4.
  See [concurrency](./concepts/concurrency.md).
- A **migration path** from legacy `REPOS.json` + `.scripts/` meta-repos via
  `grex import --from-repos-json` — both systems can coexist during the
  transition. See [migration](./guides/migration.md).

## Delivery milestones (M1–M7 shipped)

| Milestone | Shipped scope |
|---|---|
| M1 | Cargo workspace, CLI skeleton, CI matrix (Linux / macOS / Windows). |
| M2 | `init` / `add` / `rm` / `ls`, `grex.jsonl` intent log, `gix`-backed git. |
| M3 | `status` / `sync` / `update` / `exec`, lockfile, crash recovery. |
| M4 | `run` action engine (7 Tier-1 primitives), pack-type plugins, `doctor` v0. |
| M5 | Plugin API stabilised, `grex-plugins-builtin` crate, action registration. |
| M6 | Parallel scheduler, per-pack `.grex-lock`, Lean4 invariant proof (PR #24). |
| M7 | MCP stdio server (`grex serve`), 11-tool surface, 2025-06-18 conformance, `doctor --fix --lint-config`, `import --from-repos-json`, dual MIT/Apache-2.0 licence (PRs #25, #26, #28, #29, #30, #31). |
| M8 | Release machinery: cargo-dist, crates.io publish, this mdBook site, reference pack template, CHANGELOG + SemVer policy. |

The narrative design docs linked from the sidebar are the **normative v1
contract** — they hold independently of any commit SHA on `main`.

## Install

```sh
cargo install grex-cli
grex init
grex add https://github.com/egoisth777/grex-inst dev/grex-inst
grex sync
grex doctor
```

> **Note:** the crate is published as `grex-cli`; the installed binary is
> `grex`. If pemistahl's unrelated `grex` (regex-from-test-cases tool) is
> already on your PATH, pass `--force` to `cargo install grex-cli` or
> rename the existing binary first to avoid a silent overwrite.

Or use one of the pre-built installer scripts attached to each GitHub
Release (M8-1 cargo-dist wiring).

## Where to go next

- **New to grex?** Start at [goals](./concepts/goals.md), then
  [architecture](./concepts/architecture.md), then
  [pack model](./concepts/pack-spec.md).
- **Integrating grex into an agent?** Jump to [MCP server](./reference/mcp.md)
  and the [CLI reference](./reference/cli.md).
- **Working on grex itself?** See the
  [engineering handbook](./guides/engineering.md),
  [linter rules](./internals/linter.md), and
  [test plan](./guides/test-plan.md).
- **API reference?** Canonical rustdoc lives at
  [docs.rs/grex-core](https://docs.rs/grex-core) and
  [docs.rs/grex-mcp](https://docs.rs/grex-mcp).
