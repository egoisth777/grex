# feat-m8-release

**Status**: draft
**Milestone**: M8
**Artifacts**: [`spec.md`](./spec.md) · [`tasks.md`](./tasks.md)

## One-line

Ship v1.0.0: cargo-dist cross-platform binaries + crates.io publish + mdBook docs site + `grex-pack-template` reference repo + CHANGELOG / SemVer policy. No runtime behaviour changes.

## Scope (5 stages, each PR-separable)

- **M8-1** — cargo-dist wiring: `[workspace.metadata.dist]`, `.github/workflows/release.yml`, Win/Linux/macOS × x86_64 + aarch64 matrix (6 cells), shell + powershell installer scripts.
- **M8-2** — crates.io publish: name audit (`grex` or fallback), `[workspace.package].version = "1.0.0"` inheritance across all 4 crates, per-crate dry-runs, publish order `grex-core` → `grex-mcp` → `grex`.
- **M8-3** — mdBook docs site: build from `.omne/cfg/*.md`, deploy to GitHub Pages via `.github/workflows/docs.yml`, `[package.metadata.docs.rs]` on lib crates for canonical API docs.
- **M8-4** — `grex-pack-template` reference repo: separate `grex-org/grex-pack-template` GitHub repo with minimal pack skeleton, linked from main README + docs, installable via `grex add <url>`.
- **M8-5** — CHANGELOG + SemVer policy: `CHANGELOG.md` (Keep-a-Changelog 1.1.0 format) rolling up M1-M7, `docs/semver.md` defining MAJOR/MINOR/PATCH discipline for manifest / pack.yaml / CLI / MCP surfaces.

## Non-goals (v1.0.1 parking lot)

The 4 M7 residual tech-debt issues are parked for v1.0.1, explicitly NOT blockers for v1.0.0:

- **#32** — doctor on-disk drift TOCTOU handling.
- **#33** — MCP `-32002` code overload disambiguation (split into distinct codes).
- **#34** — doctor `--fix` severity roll-up edge case.
- **#35** — MCP pre-init + double-init request gates (rmcp 1.5.0 limitation).

Other deferred items: `grex upgrade` self-update, pack registry (`grex.dev`), custom docs domain, Windows Authenticode signing, CHANGELOG automation.

## Dependencies

- **Prior**: M7 fully shipped 2026-04-23 (PRs #25 #26 #28 #29 #30 #31); specifically M7-4c's `[workspace.package]` block is the inheritance seat for `version`.
- **External**: `cargo-dist` 0.22+, `mdbook` latest stable, `actions/deploy-pages@v4`, crates.io publishing credentials.
- **SSOT**: [`milestone.md`](../../../milestone.md) §M8; [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md).

## Delivery plan

See [`tasks.md`](./tasks.md) for the per-stage checklist. Ordering:

1. **M8-2** (version bump) + **M8-5** (CHANGELOG) land first — cargo-dist needs the version + reads the CHANGELOG for release notes.
2. **M8-3** (mdBook) + **M8-4** (pack-template) run in parallel, any order.
3. **M8-1** (cargo-dist tag push) lands last — the actual `v1.0.0` tag that triggers the matrix build + crates.io publish + GitHub Release.

## Acceptance criteria (summary)

Full list in [`spec.md`](./spec.md) §Acceptance. Headline items:

- `cargo install grex-cli` works fresh on Win/Linux/macOS (installs binary `grex`).
- 6-cell cargo-dist matrix produces attached artefacts on `v1.0.0` tag push.
- All 4 workspace crates report `1.0.0`; all publish to crates.io (or documented fallback).
- mdBook site live on GitHub Pages; `docs.rs/grex-core` + `docs.rs/grex-mcp` render cleanly.
- `grex add https://github.com/grex-org/grex-pack-template` + `grex run` succeeds end-to-end on 3 OS.
- `CHANGELOG.md` `[1.0.0]` entry merged; `docs/semver.md` merged.
- No regressions on M7 test suites; `cargo test --workspace` + `cargo clippy -D warnings` + `cargo fmt --check` all green.
- The 4 M7 debt issues (#32-#35) labelled `v1.0.1`, closed out of the release tracker.

## Effort estimate

Per `milestone.md` §M8: **2-4 days**. Split: M8-1 ~1d, M8-2 ~0.5d, M8-3 ~0.5d, M8-4 ~1d, M8-5 ~0.5d → ~3.5d total with margin for arm64 cross-compile flakes and crates.io name audit surprises.

## Risks / open questions

- **`grex` name squatting on crates.io** — fallback candidates (`grex-cli`, `grex-rs`, `graphex`); audit result documented in the M8-2 PR.
- **Linux aarch64 cross-compile flakes** — if the `cross` docker step OOMs on GitHub runners, bump to 4-core runner or defer Linux arm64 to v1.0.1 (5-cell matrix).
- **mdBook Pages hosting** — requires repo Pages settings source = "GitHub Actions"; documented in `docs.yml` header.
- **Windows Authenticode signing absent** — users will see SmartScreen warnings on the `.exe`; documented in release notes; follow-up for v1.0.x once a cert is acquired.
- **`grex-pack-template` lives in a separate repo** — coordination via M8-4 PR checklist; first-commit SHA cross-linked from the main repo's `[1.0.0]` CHANGELOG entry.
