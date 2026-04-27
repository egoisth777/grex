# Changelog

<!--
  Versioning policy: see ./man/semver.md.
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
See [`man/semver.md`](./man/semver.md) for what MAJOR / MINOR / PATCH mean in terms
of the grex manifest schema, CLI surface, MCP tool surface, and `pack.yaml` schema.

## [Unreleased]

### Added

### Changed

### Deprecated

### Removed

### Fixed

### Security

## [1.1.0] - 2026-04-26

Behaviour change at runtime + zero schema/API break. Brings the
default child-resolution path into alignment with the long-standing
pack-spec rule (`children[].path` is a bare name, children resolve as
flat siblings of the parent pack root). See
`openspec/changes/feat-v1.1.0-flat-children-layout/` for the full
rationale.

### Changed

- `grex sync` resolves bare-name `children[].path` as **flat siblings**
  of the parent pack root. Previous default appended `.grex/workspace/`
  between the parent and the child name; that prefix is removed. A
  parent pack at `~/code/.grex/pack.yaml` with `children: [{ path: foo }]`
  now materialises the child at `~/code/foo/.grex/pack.yaml`. Aligns
  with the locked positioning ("nested meta-repo manager") and the
  `import` → `sync` workflow described in
  [`man/guides/migration.md`](./man/guides/migration.md).
- `--workspace` CLI flag still accepts a manual override; only the
  default changes. Help text updated on `sync` and `teardown` to drop
  the `.grex/workspace` reference.

### Added

- **Auto-migration of legacy `.grex/workspace/<name>/` layout on first
  `grex sync` after upgrade.** Detects the old workspace layout, moves
  each child to its flat-sibling slot via atomic `fs::rename`, removes
  the orphan `.grex.sync.lock` left at the legacy location, and rmdir's
  the now-empty `.grex/workspace/`. Migration is idempotent (a fresh
  v1.1.0+ workspace sees no legacy directory and the pass no-ops) and
  refuses to clobber pre-existing user data at the flat-sibling slot.
  Per-child outcomes (`migrated`, `skipped_both_exist`,
  `skipped_dest_occupied`, `failed`) surface in the sync report on
  both text and `--json` output channels so operators see exactly what
  happened during the upgrade. **No user action required for default
  workspaces.**
- Plan-phase validator (`ChildPathValidator`, internal) enforces the
  bare-name rule on `children[].path`. Invalid values (`/`, `\`, `..`,
  `.`, empty, uppercase, digit-led, regex mismatch) — and URL-derived
  tails when `path:` is omitted — are rejected at sync time with a
  `ChildPathInvalid { child_name, path, reason }` error variant. The
  walker also runs the same predicate pre-clone so a malicious
  `path: ../escape` cannot materialise a directory outside the pack
  root before plan-phase validation fires.
- Plan-phase validator (`DupChildPathValidator`, internal) rejects
  any pack whose `children[]` contains two entries resolving to the
  same effective path. Surfaces as
  `PackValidationError::ChildPathDuplicate { path, urls }`.
- `grex import --from-repos-json` validates each row's `path` against
  the same bare-name rule before writing the manifest. Invalid rows
  land in `ImportPlan::failed` with a clear reason; no `Event::Add`
  is appended for them.

### Fixed

- `scan_recovery` now anchors at the resolved workspace (post
  `--workspace` override), not at `pack_root`. Previously every
  `.grex.bak` orphan under an override workspace was missed.
- `walk_for_backups_inner` uses `entry.file_type()` instead of
  `entry.metadata()` so the recursion guard truly does NOT follow
  symlinks (and skips them explicitly).

### Migration notes

- **Auto-migration handles the common case.** Workspaces with a
  legacy `.grex/workspace/<name>/` layout are relocated automatically
  on the first `grex sync` after upgrade. The migration step prints
  one log line per child (text mode) or a `workspace_migrations`
  array entry (`--json` mode) so the upgrade is auditable.
- **Concurrency caveat across the upgrade boundary.** Do not run two
  `grex` versions concurrently against the same workspace during
  upgrade. v1.0.x writes its lock at
  `<pack_root>/.grex/workspace/.grex.sync.lock`; v1.1.0 writes
  `<pack_root>/.grex.sync.lock`. The two paths are in different
  namespaces, so the cross-version overlap is **not** serialised by
  either lock. The auto-migration cleans the legacy lock as part of
  the first 1.1.0 sync — once that completes, every subsequent run
  is on the new lock path.
- Authors of `pack.yaml` files that used `children[].path: foo/bar`
  must convert to a bare name. The same regex as `pack.name`
  (`^[a-z][a-z0-9-]*$`) is enforced.

## [1.0.3] - 2026-04-26

Doc-site quality fix. No runtime / CLI / MCP / `pack.yaml` behaviour
changes — surface and metadata only.

### Fixed

- `grex-doc/book.toml` `title` no longer hardcodes `v1.0.1`. mdBook
  does not auto-inject the workspace `Cargo.toml` version, and a
  static title is the right call for a doc-site that gets republished
  on every tag. Title shortened to `"grex documentation"`. Live
  `<title>` no longer drifts behind the latest release. Commit
  `399a1b1`.

## [1.0.2] - 2026-04-25

Doc-site quality fix. No runtime / CLI / MCP / `pack.yaml` behaviour
changes — surface and metadata only.

### Fixed

- Five 404s on the live doc-site caused by markdown links into repo-only
  paths (`.omne/cfg/*`, `openspec/changes/...`) that mdBook rendered as
  `.html` siblings. Rewritten as `https://github.com/.../blob/main/...`
  source links so they resolve regardless of where the page is rendered.

### Changed

- Landing page (`introduction.md`, sync'd to `grex-doc/src/introduction.md`)
  rewritten to lead with the locked positioning ("nested meta-repo
  manager"), a 30-second quickstart, and a tighter "read next" index.
  Removed M1–M7 internal milestone changelog from the public landing.

## [1.0.1] - 2026-04-24

Documentation surface restructure plus a positioning rewrite. No runtime /
CLI / MCP / `pack.yaml` behaviour changes — surface and metadata only.

### Added

- **Documentation site** at <https://egoisth777.github.io/grex/>, built from
  [`man/`](./man/) by an mdBook site rooted at [`grex-doc/`](./grex-doc/).
  Deployed to GitHub Pages on every `v*.*.*` tag push by
  [`.github/workflows/doc-site.yml`](./.github/workflows/doc-site.yml).
- `xtask doc-site-prep` subcommand — copies `man/**/*.md` into
  `grex-doc/src/` so `mdbook build grex-doc/` can render the site without
  symlinks (Windows-friendly).
- `mdbook-linkcheck` preprocessor wired into `grex-doc/book.toml` — internal
  link rot fails the build.
- `crates/grex/tests/positioning_test.rs` — guards the v1 tagline ("nested
  meta-repo manager") on three surfaces (clap `about`, `man/grex.1`, README
  first 30 lines).
- `crates/xtask/tests/version_test.rs` — guards the workspace version
  (asserts `env!("CARGO_PKG_VERSION") == "1.0.1"`).
- New top-level [`man/README.md`](./man/README.md) — entry point for `man/`,
  indexes the 15 generated `.1` pages and the bucketed authored reference.

### Changed

- **Repositioning**: tagline reframed from "Cross-platform dev-environment
  orchestrator" to **"nested meta-repo manager. Pack-based, agent-native,
  Rust-fast."** across `README.md`, all four crate `Cargo.toml` `description`
  fields, `crates/grex/src/cli/args.rs` clap `about`, and the regenerated
  `man/grex.1` NAME line.
- Migrated `docs/` → `man/` (single human-readable doc home). Authored
  reference content is bucketed under `man/concepts/`, `man/reference/`,
  `man/guides/`, `man/internals/`, `man/ci/`. `release.md`, `semver.md`,
  and `introduction.md` stay at `man/` root for findability.
- Workspace version bumped `1.0.0` → `1.0.1`.

### Removed

- `docs/` directory deleted entirely (`book.toml`, `build.{sh,ps1}`, `src/`,
  `src-authored/`, `ci/`). Content migrated under `man/`.
- `.github/workflows/docs.yml` deleted (built the now-removed `docs/` source
  tree; superseded by `.github/workflows/doc-site.yml`).

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
  [`man/reference/cli-json.md`](./man/reference/cli-json.md). Resolves M8-6.

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

- Nothing deprecated in 1.0.0. See [`man/semver.md`](./man/semver.md) for the
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

[Unreleased]: https://github.com/egoisth777/grex/compare/v1.1.0...HEAD
[1.1.0]: https://github.com/egoisth777/grex/releases/tag/v1.1.0
[1.0.3]: https://github.com/egoisth777/grex/releases/tag/v1.0.3
[1.0.2]: https://github.com/egoisth777/grex/releases/tag/v1.0.2
[1.0.1]: https://github.com/egoisth777/grex/releases/tag/v1.0.1
[1.0.0]: https://github.com/egoisth777/grex/releases/tag/v1.0.0
