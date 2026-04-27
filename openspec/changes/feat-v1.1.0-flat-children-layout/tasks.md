# feat-v1.1.0-flat-children-layout — tasks

**Status**: implemented (PR #50 — stacked impl + post-review fixes on a single branch)
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: [`grex-doc/src/semver.md`](../../../grex-doc/src/semver.md) (MINOR justification) · [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) §"Validation rules" line 176 (bare-name rule)

Two PRs: this one (openspec only, no code) + the implementation PR that lands after openspec review.

---

## PR-1 — openspec only (this branch)

- [x] P1.1 Branch `feat/v1.1.0-flat-children-layout` cut off `main` at `78f1c38`.
- [x] P1.2 `openspec/changes/feat-v1.1.0-flat-children-layout/proposal.md` authored.
- [x] P1.3 `openspec/changes/feat-v1.1.0-flat-children-layout/design.md` authored.
- [x] P1.4 `openspec/changes/feat-v1.1.0-flat-children-layout/tasks.md` authored (this file).
- [x] P1.5 Commit + push branch (`c220fb2`).
- [x] P1.6 PR #50 opened vs `main` (originally titled `docs(openspec): draft v1.1.0 — flat-sibling child layout`; retitled to `feat(v1.1.0): flat-sibling child layout (openspec + impl)` once the impl landed on the same branch).
- [x] P1.7 Reviewers: 4 parallel codex personas surfaced 2 BLOCKERS, 7 CONCERNs, 5 OTHERs, 10 NITs — all addressed in the post-review fix sweep.
- [ ] P1.8 ~~Merge after openspec review (no code merged).~~ Superseded — openspec, impl, and post-review fixes collapsed onto a single branch + single PR.

---

## PR-2 — implementation (cut off post-merge `main`)

### Sub-change 4a — Code: drop `.grex/workspace/` default + fix backup-scan anchor

- [x] 4a.1 Branch `feat/v1.1.0-impl` off post-merge `main`.
- [x] 4a.2 [`crates/grex-core/src/sync.rs:643-649`](../../../crates/grex-core/src/sync.rs) — `resolve_workspace()` default returns `pack_root_dir(pack_root)` directly; remove `.join(".grex").join("workspace")`.
- [x] 4a.3 [`crates/grex-core/src/sync.rs:1654-1660`](../../../crates/grex-core/src/sync.rs) — `scan_recovery()` workspace anchor changes to `pack_root_dir(pack_root)`; collapse the now-redundant "also walk pack_root" fallback.
- [x] 4a.4 [`crates/grex-core/src/tree/walker.rs:184`](../../../crates/grex-core/src/tree/walker.rs) — verify no change needed (walker is anchor-agnostic).
- [x] 4a.5 Update unit tests in `sync.rs` that asserted on the old default path.

### Sub-change 4b — Validator: bare-name `children[].path`

- [x] 4b.1 Add `crates/grex-core/src/pack/validate/child_path.rs` — new `ChildPathValidator` matching the `^[a-z][a-z0-9-]*$` regex used by `name`.
- [x] 4b.2 Add `PackValidationError::ChildPathInvalid { child_name, path, reason }` variant in [`crates/grex-core/src/pack/validate/mod.rs`](../../../crates/grex-core/src/pack/validate/mod.rs).
- [x] 4b.3 Wire `ChildPathValidator` into `run_all` alongside the existing 3 validators.
- [x] 4b.4 [`crates/grex-core/src/pack/mod.rs:165-172`](../../../crates/grex-core/src/pack/mod.rs) `effective_path()` keeps current shape; document the precondition that validation has run.
- [x] 4b.5 Add table-driven tests for the validator: rejects `../escape`, `foo/bar`, `foo\bar`, `.`, `..`, `""`, `/abs`; accepts `algo-leet`, `child-a`, `a`, `a1-b2`.

### Sub-change 4c — Doc updates

- [x] 4c.1 [`grex-doc/src/concepts/pack-spec.md`](../../../grex-doc/src/concepts/pack-spec.md) — add explicit "children resolve as flat siblings of the parent pack root" sentence near the `children[].path` rule.
- [x] 4c.2 [`grex-doc/src/guides/migration.md`](../../../grex-doc/src/guides/migration.md) — verify end-to-end after the refactor; no copy edits expected (it already describes the intended workflow).
- [x] 4c.3 [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) — mirror update for 4c.1.
- [x] 4c.4 [`.omne/cfg/pack-spec.md`](../../../.omne/cfg/pack-spec.md) — mirror update for 4c.1.
- [x] 4c.5 [`crates/grex/src/cli/args.rs`](../../../crates/grex/src/cli/args.rs) — `--workspace` help text drops `.grex/workspace` reference; new copy: "Override the workspace root. Defaults to the parent pack's root directory; children resolve as flat siblings."
- [x] 4c.6 `cargo run -p xtask -- gen-man` to regenerate man pages reflecting the new help text.

### Sub-change 4d — Workspace bump 1.0.3 → 1.1.0 + version-test bump + CHANGELOG

- [x] 4d.1 `Cargo.toml` `[workspace.package].version` `"1.0.3"` → `"1.1.0"`.
- [x] 4d.2 `[workspace.dependencies]` internal `version = "1.0.3"` → `"1.1.0"` for `grex-core`, `grex-mcp`, `grex-plugins-builtin`.
- [x] 4d.3 `crates/xtask/Cargo.toml` `grex-cli = { version = "1.1.0", ... }`.
- [x] 4d.4 [`crates/xtask/tests/version_test.rs`](../../../crates/xtask/tests/version_test.rs) — `EXPECTED_WORKSPACE_VERSION` `"1.0.3"` → `"1.1.0"`.
- [x] 4d.5 `CHANGELOG.md` — add `[1.1.0] - YYYY-MM-DD` section under `[Unreleased]`. Bullets:
  - **Changed (breaking-by-correctness)**: child packs now resolve as flat siblings of the parent pack root. Previous default `.grex/workspace/<child>` is removed.
  - **Added**: validator enforces `children[].path` is a bare name (regex `^[a-z][a-z0-9-]*$`).
  - **Fixed**: `grex sync` against a real-world meta-repo layout (children as siblings of `.grex/`) now succeeds without `--workspace` override.
  - **Upgrade note**: any orphaned lock at `<root>/.grex/workspace/.grex.sync.lock` left by the old version is harmless; remove with `grex doctor` or by hand.
- [x] 4d.6 Move `[Unreleased]` link target to `v1.1.0...HEAD`; add `[1.1.0]` link footer.

### New e2e test

- [x] T.1 `crates/grex/tests/import_then_sync.rs` — full happy path:
  1. `tempdir/REPOS.json` lists 2 child sub-repos at flat-sibling layout.
  2. Pre-populate `tempdir/<child-a>/.grex/pack.yaml` and `tempdir/<child-b>/.grex/pack.yaml`.
  3. Run `grex import` — produces `tempdir/.grex/pack.yaml` with `children: [child-a, child-b]`.
  4. Run `grex sync .` from `tempdir`.
  5. Assert: zero "manifest not found" errors; both children walked; lockfile at `tempdir/.grex.sync.lock` (NOT `tempdir/.grex/workspace/.grex.sync.lock`).

### Cross-stage exit gates

- [x] G1 `cargo fmt --all -- --check` clean.
- [x] G2 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] G3 `cargo test --workspace` green (incl. new e2e + new validator tests).
- [x] G4 `cargo doc --workspace --no-deps --all-features` clean.
- [x] G5 `cargo machete` clean.
- [x] G6 `cargo run -p xtask -- gen-man` drift-free after the help-text update.
- [x] G7 `cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/` exits 0; `mdbook-linkcheck` zero broken.
- [x] G8 **Grep gate**: `rg -n '\.grex[/\\]workspace' crates/grex-core/src/` returns zero matches.
- [x] G9 `dist plan` (cargo-dist sanity) green at `1.1.0`.
- [x] G10 `man-drift` job green.
- [x] G11 `mcp-conformance` job green.
- [x] G12 **Manual gate**: `grex sync E:\repos\code` (user's real workspace, 14 children) walks the entire tree end-to-end with zero errors.

### Post-review fix sweep (added after the impl-PR review)

- [x] PR.B1 walker pre-clone gate against malicious `children[].path: ../escape`.
- [x] PR.B2 auto-migration of legacy `.grex/workspace/<name>/` layout on first sync; idempotent; refuses to clobber user data; orphan lock + empty workspace dir cleaned up.
- [x] PR.C3 `scan_recovery` anchored at resolved workspace (post `--workspace` override).
- [x] PR.C4 URL-derived path tail validated when `children[].path` is omitted.
- [x] PR.C5 `import_from_repos_json` validates `path` per-row before manifest write; bad rows route to `ImportPlan::failed`.
- [x] PR.C7 `walk_for_backups_inner` uses `entry.file_type()` so the symlink-skip is honoured.
- [x] PR.O1 `DupChildPathValidator` rejects two children resolving to the same effective path within a parent.
- [x] PR.O2 e2e test exercising `--workspace` CLI override end-to-end.
- [x] PR.O3 `ChildPathValidator` + `DupChildPathValidator` demoted to `pub(crate)` (no external consumer).
- [x] PR.O4 MCP doc reference updated to `<workspace-version>` (server already used `env!("CARGO_PKG_VERSION")`).
- [x] PR.O5 e2e test exercising auto-clone-into-flat-sibling on first sync (children NOT pre-cloned).
- [x] PR.C1 CHANGELOG `grex doctor` lock-cleanup line removed; superseded by B2 auto-migration note.
- [x] PR.C2 CHANGELOG `[1.0.3]` section added.
- [x] PR.C6 CHANGELOG documents the upgrade-window concurrency caveat.
- [x] PR.N1+N2+N3 openspec proposal + tasks updated to reflect implemented status + collapsed two-PR plan + accurate field name.
- [x] PR.N5 README "M1 scaffold" + stale v1.0.0 references updated.
- [x] PR.N6 `derive_child_label` collapsed into `check_one`'s attribution split.
- [x] PR.N7 child_path validator tests collapsed into `rejection_table` / `accept_table`.
- [x] PR.N8 `file_url` helper duplication in two test files replaced by `gix_backend::file_url_from_path`.
- [x] PR.N9 pack-spec.md `children` paragraph shortened — points at Validation rules anchor instead of duplicating regex/error text.
- [x] PR.N10 `sync.rs` `resolve_workspace` rustdoc inlined the rationale instead of linking to the openspec change directory.
- [ ] PR.N4 man/grex.1 trailing whitespace — clap_mangen output artefact (`.TH ... ""` macro emits trailing space). `git diff --check` does NOT flag it; fixing manually creates drift with the gen-man drift gate. Deferred to a clap_mangen-side fix or a sed post-processing hook in `xtask gen-man` — out of scope for this PR.

### Release

- [x] R.1 PR-2 reviewers: 4 parallel codex personas + this fix sweep.
- [ ] R.2 Merge PR #50 to `main`.
- [ ] R.3 Tag `v1.1.0` on the merge commit; push tag.
- [ ] R.4 `cargo publish` 4 crates in topological order: `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`.
- [ ] R.5 Verify cargo-dist release artefacts populate the GitHub Release.
- [ ] R.6 Verify doc-site rebuilds on `main` push (post-merge auto-trigger via `doc-site.yml`).
