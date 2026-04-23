# feat-m8-release — tasks

**Status**: draft
**Spec**: [`spec.md`](./spec.md)
**SSOT**: [`milestone.md`](../../../milestone.md) §M8; [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md)

Five independent stages, each landing in its own PR if preferred. Cross-stage ordering: M8-2 (version bump) and M8-5 (CHANGELOG) should land before M8-1 (cargo-dist tag push); M8-3 (mdBook) and M8-4 (template repo) can run in parallel at any point.

---

## Stage M8-1 — cargo-dist wiring

Generate the release workflow and verify the 6-cell matrix.

- [ ] 1.1 Install cargo-dist locally (`cargo install cargo-dist --version 0.22.*`).
- [ ] 1.2 Run `cargo dist init` at repo root; accept defaults except:
  - [ ] installers = `["shell", "powershell"]`
  - [ ] targets = 6 cells listed in spec
  - [ ] `pr-run-mode = "plan"`
- [ ] 1.3 Verify `[workspace.metadata.dist]` block landed in root `Cargo.toml`.
- [ ] 1.4 Run `cargo dist generate`; commit `.github/workflows/release.yml` as-generated (no hand edits).
- [ ] 1.5 Add a CI drift check: a GitHub Action that fails if `cargo dist generate` would produce a different `release.yml` than committed.
- [ ] 1.6 Push a throwaway `v1.0.0-rc1` tag on a feature branch; observe all 6 matrix cells complete.

**Tests**:
- [ ] 1.T1 `cargo dist plan` exits 0 locally.
- [ ] 1.T2 CI drift check passes on main after initial commit.
- [ ] 1.T3 `v1.0.0-rc1` tag push produces a draft GitHub Release with artefacts from all 6 cells.
- [ ] 1.T4 Linux `aarch64` cell completes within the runner's memory / time budget.
- [ ] 1.T5 `sh` installer from the draft release installs `grex` on a fresh Ubuntu VM; `grex --version` prints `1.0.0`.
- [ ] 1.T6 `ps1` installer from the draft release installs `grex` on a fresh Windows PowerShell session; `grex --version` prints `1.0.0`.
- [ ] 1.T7 macOS `x86_64` + `aarch64` manual install smoke test on available hardware.

---

## Stage M8-2 — crates.io publish (version bump + dry-runs + publish)

Name audit first; everything else gates on its outcome.

- [ ] 2.1 **Name audit**: `curl -sI https://crates.io/api/v1/crates/grex` — if 404, name is free; if 200, inspect the owner / version / last-update. Document finding in the PR description.
- [ ] 2.2 If `grex` is squatted, choose fallback from the ordered list (`grex-cli`, `grex-rs`, `graphex`) and update all references: `Cargo.toml`, `README.md`, installer script names, docs.rs URLs.
- [ ] 2.3 Edit root `Cargo.toml` `[workspace.package]` block: add `version = "1.0.0"` alongside the existing `license` / `authors` / `edition` / `repository` from M7-4c.
- [ ] 2.4 Edit each crate's `Cargo.toml`: replace `version = "0.0.1"` with `version.workspace = true`. Affects: `grex-core`, `grex-mcp`, `grex` (+ any lib split present).
- [ ] 2.5 Add per-crate metadata: `description`, `keywords` (≤5), `categories`, `readme = "README.md"`, `repository`, `homepage`, `documentation = "https://docs.rs/<crate>"`.
- [ ] 2.6 Run `cargo publish --dry-run -p grex-core`; fix any warnings until clean.
- [ ] 2.7 Run `cargo publish --dry-run -p grex-mcp`.
- [ ] 2.8 Run `cargo publish --dry-run -p grex` (bin crate; may need to strip bin-only deps from packaged source — address if it surfaces).
- [ ] 2.9 **Real publish** (on a release tag checkout only): `cargo publish -p grex-core` → wait for crates.io index to update (~30s) → `cargo publish -p grex-mcp` → wait → `cargo publish -p grex`.

**Tests**:
- [ ] 2.T1 `cargo metadata --format-version 1 | jq '[.packages[] | select(.name | startswith("grex")) | .version] | unique'` returns `["1.0.0"]` (single element).
- [ ] 2.T2 All three dry-runs exit 0 in CI (a `cargo publish --dry-run` job covering each crate).
- [ ] 2.T3 Post-publish, `cargo install <crate-name>` from a fresh machine succeeds on all 3 OS.
- [ ] 2.T4 `crates.io/crates/<crate>` page renders README + metadata correctly.

---

## Stage M8-3 — mdBook docs site + docs.rs metadata

- [ ] 3.1 `cargo install mdbook` in a dev container / local env.
- [ ] 3.2 Create `docs/book/book.toml` with `[book]` pointing `src = "../../.omne/cfg"` and `title = "grex"`.
- [ ] 3.3 Add `.omne/cfg/SUMMARY.md` listing chapter order. Only net-new file under `.omne/cfg/`.
- [ ] 3.4 Run `mdbook build docs/book` locally; verify zero warnings and working internal links.
- [ ] 3.5 Create `.github/workflows/docs.yml` — checkout, install mdbook, build, deploy via `actions/deploy-pages@v4` to `gh-pages` branch. Trigger: push to `main`.
- [ ] 3.6 Enable GitHub Pages in repo settings: source = "GitHub Actions".
- [ ] 3.7 Add `[package.metadata.docs.rs]` block to `crates/grex-core/Cargo.toml` and `crates/grex-mcp/Cargo.toml`:
  ```toml
  [package.metadata.docs.rs]
  all-features = true
  rustdoc-args = ["--cfg", "docsrs"]
  ```
- [ ] 3.8 Cross-link: main mdBook index page links to `docs.rs`; each crate's top-level rustdoc comment links to the mdBook site.

**Tests**:
- [ ] 3.T1 `mdbook build docs/book` exits 0 with zero warnings.
- [ ] 3.T2 CI `docs.yml` run completes on main; Pages URL returns HTTP 200 on the site root.
- [ ] 3.T3 `cargo doc --no-deps -p grex-core --cfg docsrs` builds clean locally (simulates docs.rs).
- [ ] 3.T4 After first crates.io publish (M8-2), `docs.rs/grex-core` renders within ~10 min and contains the expected modules.

---

## Stage M8-4 — `grex-pack-template` reference repo

This stage spans two repos. Most work lives outside `grex` main.

- [ ] 4.1 Create new empty GitHub repo `grex-org/grex-pack-template` (public, dual MIT/Apache-2.0 licence).
- [ ] 4.2 Commit `.grex/pack.yaml` with one `file-write` action + one `git-clone` action (illustrative, not load-bearing).
- [ ] 4.3 Commit `.grex/hooks/pre-install.sh` + `.grex/hooks/post-install.sh` — commented-out skeletons showing the hook surface.
- [ ] 4.4 Commit `README.md` with (a) purpose statement, (b) `grex add https://github.com/grex-org/grex-pack-template` demo, (c) "How to fork this for your own pack" section.
- [ ] 4.5 Commit `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSE` (pointer) — copy from main grex repo.
- [ ] 4.6 Commit `.gitignore` respecting the M6 managed-block contract.
- [ ] 4.7 In **main grex repo**: add `.omne/cfg/pack-template.md` — narrative walkthrough chapter.
- [ ] 4.8 In **main grex repo**: append "Getting Started" section to `README.md` linking to the template repo, installer scripts (M8-1), and docs site (M8-3).
- [ ] 4.9 Record the template repo's first-commit SHA in the main repo's `CHANGELOG.md` `[1.0.0]` entry for traceability.

**Tests**:
- [ ] 4.T1 On a fresh temp workspace: `grex init && grex add https://github.com/grex-org/grex-pack-template` registers without error.
- [ ] 4.T2 `grex ls` post-add shows the template pack row in the manifest.
- [ ] 4.T3 `grex run <template-pack>` executes the demo actions; exit code 0.
- [ ] 4.T4 `grex doctor` reports all-OK after the add (3 rows default; 4 with `--lint-config`).
- [ ] 4.T5 README quick-start demo copy-pasted verbatim works on Windows + Linux + macOS.

---

## Stage M8-5 — CHANGELOG + SemVer policy

Prose-only stage; no code changes.

- [ ] 5.1 Create `CHANGELOG.md` at repo root, Keep-a-Changelog 1.1.0 format.
- [ ] 5.2 `[Unreleased]` section at the top — empty subsections (`Added`/`Changed`/`Fixed`/`Removed`).
- [ ] 5.3 `[1.0.0] - 2026-MM-DD` entry rolling up M1-M7 as `Added` bullets referencing merged PR numbers (no per-commit detail). Example bullets:
  - [ ] "Added: core CLI with 11 verbs (`init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `import`, `run`, `exec`) — M1-M5."
  - [ ] "Added: concurrency primitives — parallel scheduler (PR #X), per-pack lock (PR #Y), Lean4 proof (PR #24) — M6."
  - [ ] "Added: MCP stdio server `grex serve` with 11 tool handlers, 2025-06-18 conformance (PRs #25, #26, #28) — M7-1/2/3."
  - [ ] "Added: `grex doctor` with `--fix` + `--lint-config` (PR #29); `grex import --from-repos-json` (PR #31) — M7-4a/4b."
  - [ ] "Added: dual MIT OR Apache-2.0 licence (PR #30) — M7-4c."
- [ ] 5.4 Verify cargo-dist's release-note auto-extract picks up the `[1.0.0]` section (tested during M8-1 rc1).
- [ ] 5.5 Create `docs/semver.md` covering:
  - [ ] MAJOR: breaking manifest / pack.yaml / CLI / MCP surface changes.
  - [ ] MINOR: new verbs / tools / pack-types / manifest row kinds / CLI flags with safe defaults.
  - [ ] PATCH: bug fixes, perf, docs, tests.
  - [ ] Deprecation policy: warn for one MINOR cycle before MAJOR removal; `doctor` surfaces warnings.
  - [ ] Manifest wire invariant: `schema_version` on every row; writers never emit older; readers skip-don't-error on unknown future fields.
- [ ] 5.6 Link `docs/semver.md` from the mdBook `SUMMARY.md` (M8-3) and from `CHANGELOG.md` header.

**Tests**:
- [ ] 5.T1 `CHANGELOG.md` headings / subsections match Keep-a-Changelog 1.1.0 (manual inspection or linter).
- [ ] 5.T2 cargo-dist `v1.0.0-rc1` release body contains the bullet list from the `[1.0.0]` section.
- [ ] 5.T3 `docs/semver.md` covers all 4 surface areas (manifest, pack.yaml, CLI, MCP) + deprecation policy — checklist review.
- [ ] 5.T4 `mdbook build` after 5.6 renders the semver doc as a reachable chapter.

---

## Cross-stage exit gates

- [ ] G1 `cargo test --workspace` green across all stages.
- [ ] G2 `cargo clippy --all-targets --workspace -- -D warnings` clean.
- [ ] G3 `cargo fmt --check` clean.
- [ ] G4 `cargo publish --dry-run` green for all 3 lib/bin crates (CI job).
- [ ] G5 cargo-dist release.yml drift check green.
- [ ] G6 mdBook build green; Pages URL live.
- [ ] G7 `grex-pack-template` repo exists and `grex add` + `grex run` smoke passes on all 3 OS.
- [ ] G8 CHANGELOG `[1.0.0]` entry merged; `docs/semver.md` merged.
- [ ] G9 All 4 M7 debt issues (#32, #33, #34, #35) labelled `v1.0.1` and closed out of the release tracker before the `v1.0.0` tag push.
- [ ] G10 Spec acceptance criteria 1-10 all demonstrably met (cross-link each to its test in the release PR description).
