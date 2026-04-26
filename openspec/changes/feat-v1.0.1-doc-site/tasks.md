# feat-v1.0.1-doc-site — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: [`milestone.md`](../../../milestone.md) §M8 (v1.0.1 follow-on)

Three sub-changes, one PR.

---

## Sub-change 3a — Migrate `docs/` → `man/`

- [x] 3a.1 Create bucket dirs: `man/concepts/`, `man/reference/`, `man/guides/`, `man/internals/`, `man/ci/`.
- [x] 3a.2 Move (`git mv`) `docs/src-authored/*.md` into the four buckets per the layout in [`design.md`](./design.md).
- [x] 3a.3 Move `docs/release.md` → `man/release.md`; `docs/semver.md` → `man/semver.md`; `docs/ci/mcp-conformance.md` → `man/ci/mcp-conformance.md`.
- [x] 3a.4 Delete `docs/book/`, `docs/book.toml`, `docs/build.sh`, `docs/build.ps1`, `docs/src/`, `docs/src-authored/`, `docs/ci/` (after move).
- [x] 3a.5 Delete `docs/` directory entirely.
- [x] 3a.6 Update `README.md` — repoint `docs/release.md` → `man/release.md`, `docs/semver.md` → `man/semver.md`, drop `bash docs/build.sh` line, point at `cargo xtask doc-site-prep && mdbook build grex-doc/`.
- [x] 3a.7 Update `CHANGELOG.md` — header pointer `./docs/semver.md` → `./man/semver.md`.
- [x] 3a.8 Update Rust source docs that reference `docs/src/cli-json.md` → `man/reference/cli-json.md` (4 files: `crates/grex-mcp/src/tools/{doctor,import}.rs`, `crates/grex/src/cli/verbs/{doctor,import}.rs`, `crates/grex/tests/common/mod.rs`).
- [x] 3a.9 Update `_typos.toml` comment `docs/src-authored/mcp.md` → `man/reference/mcp.md`.
- [x] 3a.10 Update `examples/pack-template/README.md` `../../docs/src/pack-template.md` → `../../man/reference/pack-template.md` (move that file too if present).
- [x] 3a.11 Update `.github/workflows/ci.yml` `docs/ci/mcp-conformance.md` → `man/ci/mcp-conformance.md` (3 references).
- [x] 3a.12 Update `.github/workflows/release.yml` comment `docs/release.md` → `man/release.md`.
- [x] 3a.13 Update `progress.md` — best-effort; historical entries can keep `docs/` paths since they describe past state.

**Tests**:
- [x] 3a.T1 `find /e/repos/utils/grex-org/grex/docs -type f 2>/dev/null` returns empty.
- [x] 3a.T2 No live (non-CHANGELOG, non-progress) reference to `docs/` left in repo.

---

## Sub-change 3b — Scaffold `grex-doc/` mdBook site + xtask `doc-site-prep`

- [x] 3b.1 Create `grex-doc/book.toml` — mdBook config, `src = "src"`, title `"grex documentation v1.0.1"`, `mdbook-linkcheck` preprocessor block, repo URL set to `https://github.com/egoisth777/grex`.
- [x] 3b.2 Create `grex-doc/src/SUMMARY.md` — hand-authored TOC mapping the four `man/` buckets to mdBook chapters.
- [x] 3b.3 Create `grex-doc/src/introduction.md` — landing page (mirror of `man/README.md` if PR-A has shipped it; placeholder otherwise).
- [x] 3b.4 Add `grex-doc/book/` to `.gitignore`.
- [x] 3b.5 Add `Cmd::DocSitePrep` variant to `crates/xtask/src/main.rs` with a `doc_site_prep()` function that walks `man/**/*.md` and copies into `grex-doc/src/` preserving subdir structure.
- [x] 3b.6 `doc-site-prep` skips `*.1` files (man-page binaries), `README.md` (lives only at root), and pre-existing authored files in `grex-doc/src/` (`SUMMARY.md`, `introduction.md`).
- [x] 3b.7 `doc-site-prep` is idempotent — running twice yields identical tree (overwrite, no append).

**Tests**:
- [x] 3b.T1 `cargo run -p xtask -- doc-site-prep` exits 0 (tested locally during PR prep).
- [x] 3b.T2 `mdbook build grex-doc/` exits 0 with zero warnings (gated locally if mdBook installed; CI gates mandatorily).
- [x] 3b.T3 `mdbook-linkcheck` reports zero broken links.

---

## Sub-change 3c — Workspace bump 1.0.0 → 1.0.1 + version regression test

- [x] 3c.1 `Cargo.toml` `[workspace.package].version` bumped `"1.0.0"` → `"1.0.1"`.
- [x] 3c.2 `[workspace.dependencies]` internal `version = "1.0.0"` → `"1.0.1"` for `grex-core`, `grex-mcp`, `grex-plugins-builtin`.
- [x] 3c.3 `crates/xtask/Cargo.toml` `grex-cli = { ..., version = "1.0.1" }`.
- [x] 3c.4 `CHANGELOG.md` — add `[1.0.1] - 2026-04-24` section under `[Unreleased]` with 3 bullets (docs migration, doc-site, version bump). Move `[Unreleased]` link target to `v1.0.1...HEAD`; add `[1.0.1]: .../releases/tag/v1.0.1` link footer.
- [x] 3c.5 `README.md` — add `## Documentation site` section linking to `https://egoisth777.github.io/grex/`. Do NOT touch tagline / "What is grex?".
- [x] 3c.6 Add `crates/xtask/tests/version_test.rs` — `#[test] fn workspace_version_is_1_0_1()` asserts `env!("CARGO_PKG_VERSION") == "1.0.1"`.

**Tests**:
- [x] 3c.T1 `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns only `1.0.1`.
- [x] 3c.T2 `cargo test -p xtask --test version_test` green.
- [x] 3c.T3 `dist plan` (release-plan CI job) still green at v1.0.1.

---

## Workflow rewiring

- [x] W.1 DELETE `.github/workflows/docs.yml`.
- [x] W.2 ADD `.github/workflows/doc-site.yml` per spec.
- [x] W.3 PR run is build-only on `man/**` / `grex-doc/**` / `crates/xtask/**` paths.
- [x] W.4 Tag-push (`v*.*.*`) deploys via `actions/deploy-pages@v4`.

---

## Cross-stage exit gates

- [x] G1 `cargo fmt --all -- --check` clean.
- [x] G2 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] G3 `cargo test --workspace` green (incl. new `version_test`).
- [x] G4 `cargo doc --workspace --no-deps --all-features` clean.
- [x] G5 `cargo machete` clean (no new unused deps).
- [x] G6 `cargo run -p xtask -- gen-man` drift-free (no CLI changes).
- [x] G7 `cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/` exits 0 (smoke gate; CI mandatory).
- [x] G8 `find docs -type f` empty.
- [x] G9 No live `docs/` references in source / workflows / non-historical docs.
- [x] G10 `dist plan` (cargo-dist sanity) still green at v1.0.1.
