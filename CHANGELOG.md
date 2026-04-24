# Changelog

<!--
  Versioning policy: see ./docs/semver.md.
  Section meanings (Keep-a-Changelog 1.1.0):
    - Added       — new features / surfaces.
    - Changed     — changes to existing behaviour.
    - Deprecated  — soon-to-be-removed features (see deprecation policy).
    - Removed     — now-removed features.
    - Fixed       — bug fixes.
    - Security    — vulnerability fixes and hardening.
-->

All notable changes to `grex` are documented in this file.

The format is based on [Keep a Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).
See [`docs/semver.md`](./docs/semver.md) for what MAJOR / MINOR / PATCH mean in terms
of the grex manifest schema, CLI surface, MCP tool surface, and `pack.yaml` schema.

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

### Security

## [1.0.0] - 2026-04-23

First stable release. Rolls up milestones M1 through M7 as shipped to `main`,
plus the M8-6 / M8-7 completeness work. Section previously tracked as
`[Unreleased - 1.0.0]`.

### Changed

- **M8-7 — MCP `import` + `doctor` wired through `grex_core`**: the
  `import` tool now dispatches into
  `grex_core::import::import_from_repos_json` and the `doctor` tool into
  `grex_core::doctor::run_doctor`, mirroring the CLI surfaces shipped in
  M7-4a / M7-4b. Both tools return structured JSON envelopes (full
  `ImportPlan` / `DoctorReport`). The `parity_import` + `parity_doctor`
  integration tests (previously `#[ignore]` breadcrumbs) are now live
  and green, closing the CLI / MCP parity gap for these two verbs.

### Added

- **`--json` output wired for all 11 non-transport verbs** (was 2/12 —
  only `doctor` and `import` honoured the flag; `init`, `add`, `rm`,
  `ls`, `status`, `sync`, `update`, `run`, `exec`, `teardown` silently
  dropped it). Stub verbs now emit
  `{"status": "unimplemented", "verb": "<name>"}`; `sync` / `teardown`
  emit a `SyncReport`-shaped document. `serve` is excluded (it owns
  stdio for JSON-RPC). Schemas are documented in
  [`docs/src/cli-json.md`](./docs/src/cli-json.md). Resolves M8-6.

- **M1 — cargo workspace scaffold**: 4-crate cargo workspace (`grex-core`,
  `grex-mcp`, `grex`, test harness), `clap`-driven CLI skeleton with the full
  12-verb surface stubbed, 78-test smoke suite, GitHub Actions CI matrix across
  Linux + macOS + Windows. Shipped via PR
  [#1](https://github.com/egoisth777/grex/pull/1)
  ([`7fc52d0`](https://github.com/egoisth777/grex/commit/7fc52d0)).
- **M2 — manifest + lockfile foundation**: append-only `grex.jsonl` intent log
  and `grex.lock.jsonl` lockfile in JSONL with `schema_version` on every row,
  atomic filesystem primitives (write-temp-then-rename), `fd-lock`-backed
  single-writer manifest lock, and 10 CI quality gates (clippy, fmt, typos,
  cargo-deny, etc.). PRs
  [#2](https://github.com/egoisth777/grex/pull/2) +
  [#3](https://github.com/egoisth777/grex/pull/3)
  ([`1e9dad3`](https://github.com/egoisth777/grex/commit/1e9dad3),
  [`1a16e3d`](https://github.com/egoisth777/grex/commit/1a16e3d)).
- **M3 — pack manifest parser + 7 Tier-1 actions + sync verb**: `pack.yaml`
  parser, the seven built-in action primitives (`file-write`, `file-copy`,
  `symlink`, `git-clone`, `shell-run`, `template`, `download`), variable
  expansion, pluggable plan-phase validator with duplicate-symlink detection,
  `GitBackend` trait over a `gix`-backed implementation, pack-tree walker with
  cycle + `depends_on` validators, `FsExecutor` for real side effects, plan-mode
  (`--dry-run`) emission, and the `grex sync` verb wiring the whole stack
  together. PRs
  [#6](https://github.com/egoisth777/grex/pull/6) →
  [#13](https://github.com/egoisth777/grex/pull/13)
  (`afaa65d` through `d160c7c`).
- **M3 post-review hardening**: semver hardening (`#[non_exhaustive]` on public
  enums, `ExecResult::Skipped` addition), data-integrity fixes (`ManifestLock`
  held across sync, symlink backup rollback), concurrency locks (workspace +
  per-repo), cross-platform polish (case folding, `HOME` fallback, kind
  auto-error), and halt-state persistence + teardown recovery. PRs
  [#14](https://github.com/egoisth777/grex/pull/14) →
  [#18](https://github.com/egoisth777/grex/pull/18).
- **M4 — plugin system (action plugins)**: `ActionPlugin` trait, registry,
  dispatch wiring, trait probes, CLI integration, lockfile plugin metadata, and
  `inventory`-backed auto-registration so built-in plugins wire themselves at
  link time. PRs
  [#20](https://github.com/egoisth777/grex/pull/20) +
  [#21](https://github.com/egoisth777/grex/pull/21)
  ([`2175a09`](https://github.com/egoisth777/grex/commit/2175a09),
  [`5206f02`](https://github.com/egoisth777/grex/commit/5206f02)).
- **M5 — pack-type plugin system**: `PackTypePlugin` trait, three built-in
  pack-types (declarative, imperative, meta), trait-dispatch wiring, teardown
  semantics, `.gitignore` managed-block contract, and meta-pack recursion. PRs
  [#22](https://github.com/egoisth777/grex/pull/22) +
  [#23](https://github.com/egoisth777/grex/pull/23)
  ([`a2e313d`](https://github.com/egoisth777/grex/commit/a2e313d),
  [`20ee5fa`](https://github.com/egoisth777/grex/commit/20ee5fa)).
- **M6 — concurrency + parallel scheduler + Lean4 proof**: tokio-based parallel
  scheduler with bounded-semaphore admission control, per-pack `.grex-lock`
  file, manifest-level `fd-lock`, and a Lean4 proof of the core scheduling
  invariant (no two concurrent writers to the same pack; bounded concurrency is
  honoured). PR
  [#24](https://github.com/egoisth777/grex/pull/24)
  ([`fba0a39`](https://github.com/egoisth777/grex/commit/fba0a39)).
- **M7-1 — MCP stdio server**: `grex serve --mcp` launches an embedded stdio
  JSON-RPC 2.0 server via `rmcp` 1.5 with per-request cancellation, 11 tool
  handlers mapping one-to-one onto the CLI verb surface. PR
  [#25](https://github.com/egoisth777/grex/pull/25)
  ([`0b80a63`](https://github.com/egoisth777/grex/commit/0b80a63)).
- **M7-2 — MCP test harness**: L2-L5 conformance test harness + permit gate
  enforced at the MCP edge (every tool call holds a scheduler permit for the
  duration of the handler). PR
  [#26](https://github.com/egoisth777/grex/pull/26)
  ([`e98af8c`](https://github.com/egoisth777/grex/commit/e98af8c)).
- **M7-3 — MCP CI conformance**: `mcp-validator` 0.3.1 wired into CI against
  the 2025-06-18 MCP spec revision; protocol drift now fails the build. PR
  [#28](https://github.com/egoisth777/grex/pull/28)
  ([`ce01eb5`](https://github.com/egoisth777/grex/commit/ce01eb5)).
- **M7-4a — `grex import --from-repos-json`**: one-shot importer for legacy
  metarepo `REPOS.json` registries; idempotent, round-trips cleanly into the
  grex manifest. PR
  [#31](https://github.com/egoisth777/grex/pull/31)
  ([`aa8c7d1`](https://github.com/egoisth777/grex/commit/aa8c7d1)).
- **M7-4b — `grex doctor` + `--fix` + `--lint-config`**: integrity-check verb
  with optional automatic remediation (`--fix`) and opt-in pack-manifest lint
  pass (`--lint-config`). Three default `OK` rows; four with `--lint-config`.
  PR [#29](https://github.com/egoisth777/grex/pull/29)
  ([`5ce880e`](https://github.com/egoisth777/grex/commit/5ce880e)).
- **M7-4c — dual MIT OR Apache-2.0 licence**: `[workspace.package]` block with
  shared `license`, `authors`, `edition`, `repository`; matching `LICENSE-MIT`,
  `LICENSE-APACHE`, and combined `LICENSE` notice; README contribution clause.
  PR [#30](https://github.com/egoisth777/grex/pull/30)
  ([`262770a`](https://github.com/egoisth777/grex/commit/262770a)).

### Changed

- **Post-M7 cleanup**: archived completed openspec change directories, pruned
  stale worktrees, refreshed `progress.md` + `milestone.md` cross-links. PR
  [#36](https://github.com/egoisth777/grex/pull/36)
  ([`d5cd99c`](https://github.com/egoisth777/grex/commit/d5cd99c)).

### Deprecated

- Nothing deprecated in 1.0.0. See [`docs/semver.md`](./docs/semver.md) for the
  deprecation policy going forward (one MINOR cycle of warnings before removal
  in a MAJOR).

### Removed

- Nothing removed in 1.0.0.

### Fixed

- All M3 post-review fixes listed above (PRs
  [#14](https://github.com/egoisth777/grex/pull/14) →
  [#18](https://github.com/egoisth777/grex/pull/18)) are rolled into this
  stable cut rather than tracked as separate patch releases.

### Security

- No known security issues at 1.0.0. `cargo-deny` is enforced in CI across the
  workspace (advisories, bans, licences, sources).

### Known limitations (tracked for 1.0.1)

The following M7 residual tech-debt items are **not blockers** for 1.0.0 and
are parked for 1.0.1:

- [#32](https://github.com/egoisth777/grex/issues/32) — `doctor`: TOCTOU window
  between `symlink_metadata` and report emission in the on-disk drift check.
- [#33](https://github.com/egoisth777/grex/issues/33) — MCP: `-32002` code is
  overloaded across pack-op errors and init-state errors; needs disambiguation.
- [#34](https://github.com/egoisth777/grex/issues/34) — `doctor`: `--fix`
  severity roll-up edge case when a post-fix retry still surfaces warnings.
- [#35](https://github.com/egoisth777/grex/issues/35) — MCP: pre-init request
  gate + double-init gate (rmcp 1.5.0 limitation; documented in
  `openspec/archive/feat-m7-1-mcp-server/spec.md` §Known limitations).

[Unreleased]: https://github.com/egoisth777/grex/compare/v1.0.0...HEAD
[1.0.0]: https://github.com/egoisth777/grex/releases/tag/v1.0.0
