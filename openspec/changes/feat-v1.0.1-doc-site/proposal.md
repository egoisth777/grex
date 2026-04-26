# feat-v1.0.1-doc-site — migrate `docs/` → `man/`, scaffold `grex-doc/` mdBook site, bump v1.0.1

**Status**: draft
**Milestone**: v1.0.1 (post-M8)
**Depends on**: PR-A (`feat-positioning-rewrite`) merged first — this PR rebases onto post-PR-A `main`.

## Why

Three structural changes for v1.0.1, all aimed at separating *agent-readable* from *human-readable* documentation:

1. **`docs/` is reserved for agent-readable project descriptions** by org convention. Today it doubles as the human/CLI reference home (mdBook source + `release.md` + `semver.md`). That overload conflates audiences.
2. **`man/` is the canonical human/CLI reference home.** Auto-generated `*.1` man pages already live there; the migration brings the rest of the human-facing reference content alongside.
3. **A user-facing v1 documentation site** (mdBook → GitHub Pages) needs to ship with v1.0.1 so install instructions, CLI reference, and concepts are discoverable at a stable URL.

## What changes (3 sub-changes)

### 3a. Migrate `docs/` → `man/`

- Move every authored markdown chapter from `docs/src-authored/` (17 files) into bucketed subdirs under `man/`:
  - `man/concepts/` — pack-spec, manifest, architecture, goals, concurrency
  - `man/reference/` — cli, cli-json, actions, plugin-api, mcp
  - `man/guides/` — migration, engineering, test-plan
  - `man/internals/` — linter, m3-review-findings, roadmap, man-pages
- Move `docs/release.md` → `man/release.md`; `docs/semver.md` → `man/semver.md`; `docs/ci/mcp-conformance.md` → `man/ci/mcp-conformance.md`.
- Delete the entire `docs/` tree (`book.toml`, `build.{ps1,sh}`, `ci/`, `src/`, `src-authored/`).
- Update every code/doc reference to `docs/...` paths to point at the new `man/...` location.

### 3b. Scaffold `grex-doc/` mdBook site

- New top-level directory `grex-doc/` (sibling to `crates/`, `man/`, `examples/`, `lean/`).
- `grex-doc/book.toml` — mdBook config; title `"grex documentation v1.0.1"`; `mdbook-linkcheck` preprocessor.
- `grex-doc/src/SUMMARY.md` — table of contents derived from `man/` structure, hand-authored for v1 (no auto-gen this round).
- Build pipeline: `cargo run -p xtask -- doc-site-prep` copies `man/**/*.md` into `grex-doc/src/` (no symlinks — Windows-hostile), then `mdbook build grex-doc/`.
- `grex-doc/book/` (mdBook output) added to `.gitignore`.

### 3c. Bump workspace version 1.0.0 → 1.0.1

- `Cargo.toml` `[workspace.package].version = "1.0.1"`.
- Workspace-internal `[workspace.dependencies]` `grex-core` / `grex-mcp` / `grex-plugins-builtin` `version = "1.0.1"`.
- `crates/xtask/Cargo.toml` `grex-cli` dep bumped to `version = "1.0.1"`.
- `CHANGELOG.md` gains a `[1.0.1] - 2026-04-24` section under `[Unreleased]` with the three bullets:
  - Migrated `docs/` → `man/` (single human-doc home).
  - New `grex-doc/` mdBook site, deployed to GitHub Pages on tag push.
  - Workspace bumped to 1.0.1.
- `README.md` gains a `## Documentation site` section linking to `https://egoisth777.github.io/grex/`.

### Workflow rewiring

- DELETE `.github/workflows/docs.yml` (it builds the now-removed `docs/` source tree).
- ADD `.github/workflows/doc-site.yml`:
  - On `pull_request` touching `man/**` / `grex-doc/**` / `crates/xtask/**`: smoke build only (`cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/`).
  - On `push: tags: ['v*.*.*']`: full build + deploy to GitHub Pages via `actions/deploy-pages@v4` + `actions/upload-pages-artifact@v3`.
  - mdBook + `mdbook-linkcheck` installed via `peaceiris/actions-mdbook@v2`.

### xtask `doc-site-prep` subcommand

- New `crates/xtask/src/main.rs` `Cmd::DocSitePrep` variant.
- Walks `man/**/*.md`, copies into `grex-doc/src/` preserving subdir structure.
- Skips `*.1` man-page binaries — only markdown.
- SUMMARY.md is hand-authored and lives under version control; `doc-site-prep` does NOT regenerate it (avoids the chicken-and-egg of the linkcheck preprocessor needing a stable TOC).

### Regression test

- `crates/xtask/tests/version_test.rs` — asserts `cargo metadata` workspace version equals `"1.0.1"`. Fails build if any future bump to the workspace forgets to update one of the inheritance points.

## What does NOT change

- No runtime / CLI behaviour changes.
- No new verbs, flags, or MCP tools.
- No edits to `crates/grex/src/cli/args.rs` clap `about` (PR-A territory).
- No edits to Cargo `description` fields (PR-A territory).
- No edits to `README.md` tagline / "What is grex?" paragraph (PR-A territory).
- `man/README.md` is NOT touched here (PR-A creates it).
- Existing `*.1` man pages are NOT regenerated (no CLI surface change → no man-drift).
- No changes to crates.io publish flow (`man/release.md` is the same content as old `docs/release.md`).
- No new external dependencies beyond `mdbook-linkcheck` (CI-installed; not a Rust dep).

## Acceptance criteria

1. `docs/` directory does not exist; `find docs -type f` returns nothing.
2. Every former `docs/...` reference in the repo points to its `man/...` equivalent (excluding CHANGELOG entries from prior releases — those are historical record).
3. `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns `1.0.1` only (xtask is `version.workspace = true` so it ride-alongs).
4. `cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/` exits 0 with zero warnings; `grex-doc/book/` populated.
5. `cargo run -p xtask -- doc-site-prep` is idempotent — running twice produces the same `grex-doc/src/` tree.
6. `mdbook-linkcheck` reports zero broken links inside the rendered site.
7. `.github/workflows/docs.yml` is deleted; `.github/workflows/doc-site.yml` exists.
8. `cargo fmt --check` + `cargo clippy --workspace --all-targets -- -D warnings` + `cargo test --workspace` clean.
9. `cargo run -p xtask -- gen-man` man-drift gate stays green (no CLI changes).
10. `cargo-dist`'s `dist plan` stays green at v1.0.1 (the `release-plan` CI job still passes).

## Source-of-truth links

- [`milestone.md`](../../../milestone.md) §M8 — v1.0.0 release umbrella; v1.0.1 is the immediate follow-on.
- [`openspec/changes/feat-positioning-rewrite/proposal.md`](../feat-positioning-rewrite/proposal.md) — PR-A; this PR rebases onto post-PR-A `main`.
- [`openspec/changes/feat-m8-release/spec.md`](../feat-m8-release/spec.md) — M8 design baseline; this PR rewrites the doc-site stage on top.
- [Keep-a-Changelog 1.1.0](https://keepachangelog.com/en/1.1.0/) — CHANGELOG format.
- [mdbook-linkcheck](https://github.com/Michael-F-Bryan/mdbook-linkcheck) — link-validation preprocessor.
