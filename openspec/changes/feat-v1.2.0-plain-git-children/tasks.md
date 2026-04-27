# feat-v1.2.0-plain-git-children — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: [`progress.md`](../../../progress.md) §"Endpoint (2026-04-27, v1.1.0 SHIPPED)" (the trigger evidence)

Markdown-only openspec PR first; implementation lands on a separate branch off post-merge `main`.

---

## Stage 0 — openspec PR (this branch)

- [ ] 0.1 Land openspec triplet under `openspec/changes/feat-v1.2.0-plain-git-children/` (proposal + design + tasks).
- [ ] 0.2 Add v1.2.0 placeholder callout to `grex-doc/src/concepts/pack-spec.md` and `man/concepts/pack-spec.md` (mirror).
- [ ] 0.3 Update `progress.md` with the v1.2.0 openspec endpoint + refreshed "Where we are" block.
- [ ] 0.4 PR description references the locked decisions (approach A; SemVer MINOR; openspec-then-impl staging).
- [ ] 0.5 Required CI gates green (typos, build × 3, etc.) — markdown-only, should pass trivially.

---

## Stage 1 — implementation branch (after openspec PR merges)

### 1a — branch + scaffold

- [ ] 1a.1 Branch `feat/v1.2.0-impl` off post-merge `main`.
- [ ] 1a.2 Confirm `cargo test --workspace` baseline green at HEAD before any code changes (sanity check).

### 1b — walker synthesis fallback

- [ ] 1b.1 Add `synthesize_plain_git_manifest(child: &ChildRef) -> PackManifest` helper to `crates/grex-core/src/tree/walker.rs` (or to `loader.rs` if cleaner — see [`design.md`](./design.md) "Decision: synthesize at the loader-fallback boundary, not in the loader"; helper lives wherever cyclomatic budget tolerates it).
- [ ] 1b.2 Replace `let child_manifest = self.loader.load(&dest)?;` in `Walker::handle_child` with the match-fallback shown in design.md, gated on `dest_has_git_repo(&dest)`.
- [ ] 1b.3 Tag the walker-recorded `PackNode` with a `synthetic: bool` flag (graph-side) so downstream consumers (lockfile writer, doctor, ls) can read it without re-deriving. Add to `crates/grex-core/src/tree/graph.rs :: PackNode`.
- [ ] 1b.4 Unit-test `synthesize_plain_git_manifest` directly: name = effective_path, type = Scripted, all lists empty.
- [ ] 1b.5 Unit-test the walker fallback with a mock `PackLoader` that returns `ManifestNotFound` + a fixture filesystem with a `.git/` marker.

### 1c — lockfile schema bump

- [ ] 1c.1 Add `pub synthetic: bool` (with `#[serde(default)]`) to `crates/grex-core/src/lockfile/entry.rs :: LockEntry`.
- [ ] 1c.2 Update every in-workspace constructor of `LockEntry` to set `synthetic: false` explicitly (search-and-replace; small blast radius).
- [ ] 1c.3 Lockfile-writer code path that runs after a walk emits `synthetic: true` for nodes the walker tagged as synthetic.
- [ ] 1c.4 Round-trip test: write synthetic entries, read back, assert field preserved.
- [ ] 1c.5 Forward-compat test: parse a v1.1.0-shaped lockfile line (no `synthetic` field) and assert deserialized `synthetic == false`.

### 1d — doctor

- [ ] 1d.1 `crates/grex-core/src/doctor.rs` (or successor): branch on `entry.synthetic`. Skip schema validation; still run gitignore drift + orphan-lock checks.
- [ ] 1d.2 Human output for synthetic packs: `OK (synthetic)`.
- [ ] 1d.3 JSON output: add `synthetic: true` to per-pack diagnostic entries.
- [ ] 1d.4 Doctor test: synthetic-pack workspace → `doctor` exits 0, no missing-manifest errors.

### 1e — `grex ls`

- [ ] 1e.1 `crates/grex/src/cli/verbs/ls.rs`: prepend `~` (final glyph TBD; `~` placeholder) to synthetic-pack tree lines.
- [ ] 1e.2 `--json` output: add `synthetic: true` field to entries.
- [ ] 1e.3 Snapshot/golden tests updated to cover the synthetic-marker rendering.

### 1f — e2e regression

- [ ] 1f.1 New file `crates/grex/tests/plain_git_children_sync.rs`. Fixture: workspace with a meta-pack root + N flat-sibling plain-git child dirs (each `git init`'d, each containing one tracked file).
- [ ] 1f.2 Test: `grex sync <fixture-workspace>` exits 0, walks every child, runs `git pull` (mocked / local-remote tested via tmp git repos).
- [ ] 1f.3 Test: re-run `grex sync` immediately, assert exit 0 and identical lockfile content (idempotency).
- [ ] 1f.4 Test: mixed tree — meta + one declarative pack-yaml-equipped child + one plain-git child — walks to completion.

### 1g — docs

- [ ] 1g.1 Replace v1.2.0 placeholder callout in `man/concepts/pack-spec.md` and `grex-doc/src/concepts/pack-spec.md` with the full "Plain-git children" subsection (no recursion, sync = git pull only, no hooks, `.grex/pack.yaml` not required, synthetic-marker visibility).
- [ ] 1g.2 Refresh `man/guides/migration.md`: `grex import --from-repos-json` + `grex sync` now works end-to-end on plain-git children. Drop any v1.1.0-era caveats about per-child pack.yaml being mandatory.
- [ ] 1g.3 Mirror migration.md changes to `grex-doc/src/guides/migration.md` (handled by `cargo xtask doc-site-prep` if the doc-site-prep flow is still copy-only; verify).

### 1h — version bump + CHANGELOG

- [ ] 1h.1 Workspace bump 1.1.0 → 1.2.0: `Cargo.toml` `[workspace.package].version`, `[workspace.dependencies]` (`grex-core`, `grex-mcp`, `grex-plugins-builtin`), `crates/xtask/Cargo.toml` `grex-cli` dep.
- [ ] 1h.2 `crates/xtask/tests/version_test.rs` bump assertion: `"1.1.0"` → `"1.2.0"`.
- [ ] 1h.3 `CHANGELOG.md` `[1.2.0] - 2026-04-2X` section under `[Unreleased]` with bullets: synthetic plain-git children walk; `LockEntry.synthetic` field; doctor/ls surface; e2e test added.

### 1i — gates (impl PR)

- [ ] 1i.1 `cargo fmt --all -- --check` clean.
- [ ] 1i.2 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] 1i.3 `cargo test --workspace` green (existing 703 + new tests from 1b/1c/1d/1e/1f).
- [ ] 1i.4 `cargo run -p xtask -- gen-man` drift-free (no CLI surface change beyond ls marker; verify man page diff is intentional or zero).
- [ ] 1i.5 `cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/` exits 0 with zero warnings.
- [ ] 1i.6 `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns only `1.2.0`.
- [ ] 1i.7 `dist plan` (cargo-dist) green at v1.2.0.
- [ ] 1i.8 MCP conformance gate green.

### 1j — manual real-world verification

- [ ] 1j.1 `cargo install --path crates/grex --force` (or local-built binary).
- [ ] 1j.2 `grex sync E:\repos\code` walks all 14 plain-git children, exits 0.
- [ ] 1j.3 Re-run `grex sync E:\repos\code` immediately: exit 0, no destructive changes, idempotent.
- [ ] 1j.4 `grex ls E:\repos\code` shows the 14 children with synthetic marker.
- [ ] 1j.5 `grex doctor E:\repos\code` reports `OK (synthetic)` for the 14 plain-git children.

### 1k — ship

- [ ] 1k.1 Squash-merge impl PR → `main`.
- [ ] 1k.2 Tag `v1.2.0` (annotated, on the squash commit). Push.
- [ ] 1k.3 Wait for `release.yml` (cargo-dist) to publish GitHub Release with archives + installers.
- [ ] 1k.4 Publish 4 crates topologically: `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`. Wait for index propagation between each.
- [ ] 1k.5 Verify `crates.io` `max_version: 1.2.0` for all 4.
- [ ] 1k.6 `cargo install grex-cli --force --version 1.2.0` then re-run real-world sync (1j) to confirm published binary matches local validation.
- [ ] 1k.7 Update `progress.md` with v1.2.0 SHIPPED endpoint.
