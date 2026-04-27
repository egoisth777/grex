# feat-v1.1.0-flat-children-layout — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: [`grex-doc/src/semver.md`](../../../grex-doc/src/semver.md) (MINOR justification) · [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) §"Validation rules" line 176 (bare-name rule)

Two PRs: this one (openspec only, no code) + the implementation PR that lands after openspec review.

---

## PR-1 — openspec only (this branch)

- [x] P1.1 Branch `feat/v1.1.0-flat-children-layout` cut off `main` at `78f1c38`.
- [x] P1.2 `openspec/changes/feat-v1.1.0-flat-children-layout/proposal.md` authored.
- [x] P1.3 `openspec/changes/feat-v1.1.0-flat-children-layout/design.md` authored.
- [x] P1.4 `openspec/changes/feat-v1.1.0-flat-children-layout/tasks.md` authored (this file).
- [ ] P1.5 Commit + push branch.
- [ ] P1.6 Open PR vs `main` titled `docs(openspec): draft v1.1.0 — flat-sibling child layout`.
- [ ] P1.7 Reviewers: parallel personas (correctness, simplicity, maintainability, architecture).
- [ ] P1.8 Merge after openspec review (no code merged).

---

## PR-2 — implementation (cut off post-merge `main`)

### Sub-change 4a — Code: drop `.grex/workspace/` default + fix backup-scan anchor

- [ ] 4a.1 Branch `feat/v1.1.0-impl` off post-merge `main`.
- [ ] 4a.2 [`crates/grex-core/src/sync.rs:643-649`](../../../crates/grex-core/src/sync.rs) — `resolve_workspace()` default returns `pack_root_dir(pack_root)` directly; remove `.join(".grex").join("workspace")`.
- [ ] 4a.3 [`crates/grex-core/src/sync.rs:1654-1660`](../../../crates/grex-core/src/sync.rs) — `scan_recovery()` workspace anchor changes to `pack_root_dir(pack_root)`; collapse the now-redundant "also walk pack_root" fallback.
- [ ] 4a.4 [`crates/grex-core/src/tree/walker.rs:184`](../../../crates/grex-core/src/tree/walker.rs) — verify no change needed (walker is anchor-agnostic).
- [ ] 4a.5 Update unit tests in `sync.rs` that asserted on the old default path.

### Sub-change 4b — Validator: bare-name `children[].path`

- [ ] 4b.1 Add `crates/grex-core/src/pack/validate/child_path.rs` — new `ChildPathValidator` matching the `^[a-z][a-z0-9-]*$` regex used by `name`.
- [ ] 4b.2 Add `PackValidationError::ChildPathInvalid { child_name, path, reason }` variant in [`crates/grex-core/src/pack/validate/mod.rs`](../../../crates/grex-core/src/pack/validate/mod.rs).
- [ ] 4b.3 Wire `ChildPathValidator` into `run_all` alongside the existing 3 validators.
- [ ] 4b.4 [`crates/grex-core/src/pack/mod.rs:165-172`](../../../crates/grex-core/src/pack/mod.rs) `effective_path()` keeps current shape; document the precondition that validation has run.
- [ ] 4b.5 Add table-driven tests for the validator: rejects `../escape`, `foo/bar`, `foo\bar`, `.`, `..`, `""`, `/abs`; accepts `algo-leet`, `child-a`, `a`, `a1-b2`.

### Sub-change 4c — Doc updates

- [ ] 4c.1 [`grex-doc/src/concepts/pack-spec.md`](../../../grex-doc/src/concepts/pack-spec.md) — add explicit "children resolve as flat siblings of the parent pack root" sentence near the `children[].path` rule.
- [ ] 4c.2 [`grex-doc/src/guides/migration.md`](../../../grex-doc/src/guides/migration.md) — verify end-to-end after the refactor; no copy edits expected (it already describes the intended workflow).
- [ ] 4c.3 [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) — mirror update for 4c.1.
- [ ] 4c.4 [`.omne/cfg/pack-spec.md`](../../../.omne/cfg/pack-spec.md) — mirror update for 4c.1.
- [ ] 4c.5 [`crates/grex/src/cli/args.rs`](../../../crates/grex/src/cli/args.rs) — `--workspace` help text drops `.grex/workspace` reference; new copy: "Override the workspace root. Defaults to the parent pack's root directory; children resolve as flat siblings."
- [ ] 4c.6 `cargo run -p xtask -- gen-man` to regenerate man pages reflecting the new help text.

### Sub-change 4d — Workspace bump 1.0.3 → 1.1.0 + version-test bump + CHANGELOG

- [ ] 4d.1 `Cargo.toml` `[workspace.package].version` `"1.0.3"` → `"1.1.0"`.
- [ ] 4d.2 `[workspace.dependencies]` internal `version = "1.0.3"` → `"1.1.0"` for `grex-core`, `grex-mcp`, `grex-plugins-builtin`.
- [ ] 4d.3 `crates/xtask/Cargo.toml` `grex-cli = { version = "1.1.0", ... }`.
- [ ] 4d.4 [`crates/xtask/tests/version_test.rs`](../../../crates/xtask/tests/version_test.rs) — `EXPECTED_WORKSPACE_VERSION` `"1.0.3"` → `"1.1.0"`.
- [ ] 4d.5 `CHANGELOG.md` — add `[1.1.0] - YYYY-MM-DD` section under `[Unreleased]`. Bullets:
  - **Changed (breaking-by-correctness)**: child packs now resolve as flat siblings of the parent pack root. Previous default `.grex/workspace/<child>` is removed.
  - **Added**: validator enforces `children[].path` is a bare name (regex `^[a-z][a-z0-9-]*$`).
  - **Fixed**: `grex sync` against a real-world meta-repo layout (children as siblings of `.grex/`) now succeeds without `--workspace` override.
  - **Upgrade note**: any orphaned lock at `<root>/.grex/workspace/.grex.sync.lock` left by the old version is harmless; remove with `grex doctor` or by hand.
- [ ] 4d.6 Move `[Unreleased]` link target to `v1.1.0...HEAD`; add `[1.1.0]` link footer.

### New e2e test

- [ ] T.1 `crates/grex/tests/import_then_sync.rs` — full happy path:
  1. `tempdir/REPOS.json` lists 2 child sub-repos at flat-sibling layout.
  2. Pre-populate `tempdir/<child-a>/.grex/pack.yaml` and `tempdir/<child-b>/.grex/pack.yaml`.
  3. Run `grex import` — produces `tempdir/.grex/pack.yaml` with `children: [child-a, child-b]`.
  4. Run `grex sync .` from `tempdir`.
  5. Assert: zero "manifest not found" errors; both children walked; lockfile at `tempdir/.grex.sync.lock` (NOT `tempdir/.grex/workspace/.grex.sync.lock`).

### Cross-stage exit gates

- [ ] G1 `cargo fmt --all -- --check` clean.
- [ ] G2 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] G3 `cargo test --workspace` green (incl. new e2e + new validator tests).
- [ ] G4 `cargo doc --workspace --no-deps --all-features` clean.
- [ ] G5 `cargo machete` clean.
- [ ] G6 `cargo run -p xtask -- gen-man` drift-free after the help-text update.
- [ ] G7 `cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/` exits 0; `mdbook-linkcheck` zero broken.
- [ ] G8 **Grep gate**: `rg -n '\.grex[/\\]workspace' crates/grex-core/src/` returns zero matches.
- [ ] G9 `dist plan` (cargo-dist sanity) green at `1.1.0`.
- [ ] G10 `man-drift` job green.
- [ ] G11 `mcp-conformance` job green.
- [ ] G12 **Manual gate**: `grex sync E:\repos\code` (user's real workspace, 14 children) walks the entire tree end-to-end with zero errors.

### Release

- [ ] R.1 PR-2 reviewers: parallel personas (correctness, simplicity, maintainability, architecture).
- [ ] R.2 Merge PR-2 to `main`.
- [ ] R.3 Tag `v1.1.0` on the merge commit; push tag.
- [ ] R.4 `cargo publish` 4 crates in topological order: `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`.
- [ ] R.5 Verify cargo-dist release artefacts populate the GitHub Release.
- [ ] R.6 Verify doc-site rebuilds on `main` push (post-merge auto-trigger via `doc-site.yml`).
