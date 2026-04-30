# feat-v1.2.0-nested-children — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: `.omne/cfg/walker.md` (canonical algorithm) · `lean/Grex/Walker.lean` (proof) · [`rust-design-decisions.md`](./rust-design-decisions.md) (in-flight; mechanism choices)

Markdown-only openspec PR first; implementation lands on a separate branch off post-merge `main`.

---

## Stage 0 — openspec PR (this branch)

- [ ] 0.1 Land openspec triplet under `openspec/changes/feat-v1.2.0-nested-children/` (proposal + design + tasks).
- [ ] 0.2 Cross-link `.omne/cfg/walker.md` → this triplet, and confirm walker.md acceptance-criteria block points back to `proposal.md`.
- [ ] 0.3 Update `progress.md` with the v1.2.0 openspec endpoint + refreshed "Where we are" block.
- [ ] 0.4 Confirm `lean/Grex/Walker.lean` builds clean under `lake build` and the 8 theorems are present (snapshot: 368 lines, 4 bridge axioms).
- [ ] 0.5 PR description references the locked decisions (parent-relative resolution; distributed lockfile; cargo-parallel; synthesis retired; SemVer MINOR per maintainer override).
- [ ] 0.6 Required CI gates green (typos, build × 3, lake-build, etc.) — markdown-only, should pass trivially.

---

## Stage 1 — implementation branch (after openspec PR merges)

### 1a — branch baseline

- [ ] 1a.1 Branch `feat/v1.2.0-impl` off post-merge `main`.
- [ ] 1a.2 Confirm `cargo test --workspace` baseline green at HEAD before any code changes.
- [ ] 1a.3 Confirm `lake build` clean in `lean/`.
- [ ] 1a.4 Snapshot the test count for the post-impl regression check (expected: existing v1.1.1 count + v1.2.0 additions).

### 1b — `LockEntry.path` field + read-fallback

- [ ] 1b.1 Add `pub path: Option<String>` (with `#[serde(default)]`) to `crates/grex-core/src/lockfile/entry.rs :: LockEntry`. Field appended after `synthetic`.
- [ ] 1b.2 Update every in-workspace constructor of `LockEntry` to set `path: Some(...)` explicitly with the relative-to-meta path.
- [ ] 1b.3 Add read-fallback helper `LockEntry::effective_path(&self) -> &str` that returns `self.path.as_deref().unwrap_or(&self.id)`.
- [ ] 1b.4 Round-trip test: write entries with `path: Some(...)`, read back, assert preserved.
- [ ] 1b.5 Forward-compat test: parse a v1.1.x-shaped lockfile line (no `path` field) and assert `effective_path()` returns the id.
- [ ] 1b.6 Verify `synthetic: bool` reads cleanly from v1.1.1 lockfiles; confirm new writes always set `synthetic: false`.
- **Verification**: `cargo test -p grex-core lockfile`.
- **Depends on**: 1a.

### 1c — validator relaxation + new rejects

- [ ] 1c.1 Relax `child.path` regex to allow `/` as separator. Per-segment validation replaces the bare-name regex.
- [ ] 1c.2 Add `unicode-normalization` crate dep; NFC-normalise each segment before duplicate-check.
- [ ] 1c.3 Reject `..`, absolute paths, drive letters, `:`, `$`, `~<digit>`, Windows reserved names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`).
- [ ] 1c.4 Reject Windows junctions and NTFS reparse points (other than proper symlinks). Reject gitfile `.git` files.
- [ ] 1c.5 New `TreeError` variants: `PathEscapesParent`, `WindowsReparseRejected`, `GitfileRejected`, `UnicodeNfcDuplicate`, `WindowsSpecialSegment`, `DuplicateChildDest`. Display impls per design.
- [ ] 1c.6 Parameterised tests in `crates/grex-core/tests/validator.rs` covering each reject case (one test per variant).
- **Verification**: `cargo test -p grex-core validator`.
- **Depends on**: 1a.

### 1d — TOCTOU mitigation crate adoption

- [ ] 1d.1 Adopt the crate selected in [`rust-design-decisions.md`](./rust-design-decisions.md) (likely `cap-std`, possibly with `openat2` direct on Linux). Add to `crates/grex-core/Cargo.toml`.
- [ ] 1d.2 Replace any `Path::canonicalize` + `Path::starts_with` patterns in dest-resolution with the cap-std handle-based check.
- [ ] 1d.3 Symlink target check happens on the kernel-confirmed handle, not on the path string.
- [ ] 1d.4 New `TreeError::SymlinkCrossesBoundary { src, target }` variant.
- [ ] 1d.5 Test: a fixture with a symlink whose target escapes the parent meta is rejected with `SymlinkCrossesBoundary`. Test runs on POSIX + Windows (gated `cfg(unix)` / `cfg(windows)`).
- **Verification**: `cargo test -p grex-core symlink_boundary`.
- **Depends on**: 1c.

### 1e — walker Phase 1 — 5-way branch + untracked aggregation

- [ ] 1e.1 Refactor `Walker::handle_child` into a `classify_dest` step returning `DestClass { Missing | PresentDeclared | PresentDirty | PresentInProgress | PresentUndeclared }`.
- [ ] 1e.2 Untracked-children aggregation: scan `<meta>/` for `.git/`-bearing dirs not in `manifest.children`; push to a shared `Mutex<Vec<UntrackedPath>>` accumulator.
- [ ] 1e.3 At end of walk, if accumulator non-empty, raise `TreeError::UntrackedChildren { paths }` with each entry carrying the `grex add <path>` fix line.
- [ ] 1e.4 Drop the v1.1.1 `synthesize_plain_git_manifest` helper. Walker no longer synthesizes manifests.
- [ ] 1e.5 In-progress detection: scan `.git/rebase-merge/`, `.git/rebase-apply/`, `.git/MERGE_HEAD`, `.git/CHERRY_PICK_HEAD`, `.git/BISECT_LOG`, `.git/REVERT_HEAD`; detached-HEAD via `git symbolic-ref --quiet HEAD` exit-1. Surface as `GitInProgress { dest, kind }`.
- **Verification**: `cargo test -p grex-core walker_phase1`. New test `untracked_children_error.rs`.
- **Depends on**: 1c, 1d.

### 1f — walker Phase 2 — prune-safety + recursive consent

- [ ] 1f.1 Implement `recursive_consent_walk(dest: &Path) -> ConsentResult` walking the orphan dest read-only (never touches lockfiles).
- [ ] 1f.2 Consent refusal kinds: `DirtyTree`, `DirtyTreeWithIgnored`, `GitInProgress`, `SubMetaWithDirtyChildren`. Returned as `ConsentRefusal` enum.
- [ ] 1f.3 `git status --porcelain --ignored` (note the `--ignored` flag — required to surface gitignored build artefacts).
- [ ] 1f.4 Prune executes only on `Clean` consent; otherwise raises `TreeError::PruneBlocked { dest, reason }`.
- [ ] 1f.5 Prune mechanism: native `rmtree.ps1`/`.sh` shim (no `shutil.rmtree` equivalents in Rust — use the existing platform-native pattern).
- [ ] 1f.6 New `TreeError::DirtyTreeWithIgnored { dest, paths }` variant (separate from plain `DirtyTree`).
- **Verification**: `cargo test -p grex-core prune_consent`. New test `cleanup_consent.rs` end-to-end.
- **Depends on**: 1e.

### 1g — walker Phase 3 — parent-relative recursion + cargo-parallel

- [ ] 1g.1 Replace global-anchor resolution with `dest = current_meta.join(child.path)`.
- [ ] 1g.2 Recursion entry: cwd of the CLI verb invocation. Drop the `Workspace`-anchor concept from the walker module.
- [ ] 1g.3 Adopt the scheduler choice from [`rust-design-decisions.md`](./rust-design-decisions.md) (rayon vs tokio). Sibling tasks within one meta and sub-meta tasks across the recursion frontier share one pool.
- [ ] 1g.4 Lockfile write under per-meta fd-lock (sentinel file at `<meta>/.grex/.lock`).
- [ ] 1g.5 Unit test: 4 sibling sub-metas × 4 leaves each completes in ~slowest-chain time, not sum (timing-tolerant via `Barrier`).
- **Verification**: `cargo test -p grex-core parallel_scheduler`.
- **Depends on**: 1e, 1f.

### 1h — distributed lockfile (per-meta read/write/fold)

- [ ] 1h.1 Lockfile reader operates on `<meta>/.grex/grex.lock.jsonl` only — never reads sub-meta lockfiles transitively.
- [ ] 1h.2 Lockfile writer operates on `<meta>/.grex/grex.lock.jsonl` only — never writes sub-meta lockfiles.
- [ ] 1h.3 Fold operation for `grex ls`: depth-first walk reading each meta's lockfile in turn.
- [ ] 1h.4 v1.1.x → v1.2.0 auto-migration: detect single-flat-lockfile shape, split into per-meta lockfiles, rename legacy file to `grex.lock.jsonl.v1_1.bak`.
- [ ] 1h.5 `--no-auto-migrate-lockfile` flag opts out; without auto-migrate, v1.2.0 errors with a manual migration hint.
- [ ] 1h.6 Test: v1.1.1 fixture migrated cleanly; per-meta lockfiles correct; `.bak` present.
- **Verification**: `cargo test -p grex-core distributed_lockfile`. New test `lockfile_v1_1_compat.rs`.
- **Depends on**: 1b, 1g.

### 1i — `ls.rs` migrate to parent-relative + drop synthesis fallback

- [ ] 1i.1 `crates/grex/src/cli/verbs/ls.rs`: render the parent-relative tree by reading each meta's lockfile in sequence.
- [ ] 1i.2 Drop the v1.1.1 synthesis fallback path.
- [ ] 1i.3 `ls --json` output: nested children represented as nested JSON, one node per meta with its `path` (relative-to-parent) populated.
- [ ] 1i.4 `~` synthetic marker preserved for entries that have `synthetic: true` (from v1.1.x lockfiles read forward); newly written entries never carry the marker.
- [ ] 1i.5 Snapshot/golden tests updated for the nested rendering.
- **Verification**: `cargo test -p grex ls_nested`.
- **Depends on**: 1h.

### 1j — `doctor` recursive default + `--shallow` flag

- [ ] 1j.1 `doctor` walks the cwd's full subtree by default (uses the same walker as `sync`).
- [ ] 1j.2 `--shallow` flag walks one level only (the cwd meta + its direct children, no sub-meta recursion).
- [ ] 1j.3 Doctor surfaces all `TreeError` variants the walker produces — same error model as `sync`.
- [ ] 1j.4 Doctor reports `synthetic: true` entries with an upgrade-advisory: `note: synthetic pack <id> — synthesis is retired in v1.2.0; consider 'grex add <path>'`.
- [ ] 1j.5 Test: `doctor` recursive sees grandchild errors; `doctor --shallow` does not.
- **Verification**: `cargo test -p grex doctor_recursion`.
- **Depends on**: 1g, 1h.

### 1k — error variants + display impls

- [ ] 1k.1 Add all v1.2.0 `TreeError` variants listed in [`design.md`](./design.md) "Error variants" table.
- [ ] 1k.2 Implement `Display` for each (single-line user message + `--verbose` long form with fix hint).
- [ ] 1k.3 Add `TreeError::Multiple { errors: Vec<TreeError> }` aggregator for parallel-walk error collection.
- [ ] 1k.4 Test: each variant's display matches a golden string.
- **Verification**: `cargo test -p grex-core tree_error_display`.
- **Depends on**: 1c, 1e, 1f, 1g.

### 1l — `--force-prune` flag + audit-log entry

- [ ] 1l.1 New flag `--force-prune` on `grex sync` and `grex remove`. Bypasses the consent walk.
- [ ] 1l.2 Force-prune writes an audit-log entry (existing event-log mechanism) recording the bypass + the dest pruned.
- [ ] 1l.3 Test: `--force-prune` against a dirty sub-meta dest succeeds + audit-log entry present.
- **Verification**: `cargo test -p grex force_prune`.
- **Depends on**: 1f.

### 1m — `SyncOptions` + MCP envelope deprecation

- [ ] 1m.1 Add `SyncOptions::cwd_meta` field as alias for `workspace`. Both populate the same internal field.
- [ ] 1m.2 Add `SyncOptions::with_cwd_meta()` builder method.
- [ ] 1m.3 Mark `SyncOptions::workspace` and `with_workspace()` as `#[deprecated(since = "1.2.0", note = "use cwd_meta")]`.
- [ ] 1m.4 MCP `SyncParams.workspace` and `LsResponse.workspace` keep field names; document the value-semantics shift in `man/mcp/CHANGELOG.md`.
- [ ] 1m.5 Test: both `with_workspace()` and `with_cwd_meta()` produce identical `SyncOptions`.
- **Verification**: `cargo test -p grex-core sync_options_alias`.
- **Depends on**: 1g.

### 1n — v1.2.0 test fixtures (nested + edge cases)

- [ ] 1n.1 New fixture dir `crates/grex/tests/fixtures/nested-children/` with a 3-level tree (root → apps/web → apps/web/services/api).
- [ ] 1n.2 New e2e test `crates/grex/tests/nested_children_walk.rs` covering acceptance criterion #1 + #8 (idempotent_resync).
- [ ] 1n.3 New e2e test `crates/grex/tests/sub_meta_autonomy.rs` covering criterion #3.
- [ ] 1n.4 New e2e test `crates/grex/tests/cleanup_consent.rs` covering criterion #7.
- [ ] 1n.5 New unit tests in `crates/grex-core/tests/distributed_lockfile.rs`, `untracked_children_error.rs`, `validator.rs`, `parallel_scheduler.rs`, `lockfile_v1_1_compat.rs`, `cycle_detection.rs` (nested-cycle case added).
- [ ] 1n.6 Update `man/test-plan.md` with v1.2.0 scenarios (was v1.0-vintage).
- **Verification**: `cargo test --workspace` — all new tests pass; existing tests still pass.
- **Depends on**: 1b, 1c, 1d, 1e, 1f, 1g, 1h, 1i, 1j, 1k, 1l.

### 1o — gates + version bump + CHANGELOG

- [ ] 1o.1 `cargo fmt --all -- --check` clean.
- [ ] 1o.2 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] 1o.3 `cargo test --workspace` green (existing + v1.2.0 additions).
- [ ] 1o.4 `cargo run -p xtask -- gen-man` drift-free.
- [ ] 1o.5 `cargo run -p xtask -- doc-site-prep && mdbook build grex-doc/` zero warnings.
- [ ] 1o.6 `cargo deny check` clean.
- [ ] 1o.7 `typos` clean.
- [ ] 1o.8 `lake build` (lean/) clean — 8 theorems present, 4 bridge axioms documented.
- [ ] 1o.9 MCP conformance gate green.
- [ ] 1o.10 Workspace bump 1.1.1 → 1.2.0 across `Cargo.toml`, `[workspace.dependencies]`, `crates/xtask/Cargo.toml`.
- [ ] 1o.11 `crates/xtask/tests/version_test.rs` bump assertion.
- [ ] 1o.12 `CHANGELOG.md` `[1.2.0] - 2026-04-XX` section: parent-relative resolution; distributed lockfile; cargo-parallel; synthesis retired; new validator rules; v1.1.x lockfile auto-migration; deprecate `workspace` in favour of `cwd_meta`.
- [ ] 1o.13 `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns only `1.2.0`.
- [ ] 1o.14 `dist plan` (cargo-dist) green at v1.2.0.

### 1p — manual real-world verification

- [ ] 1p.1 `cargo install --path crates/grex --force`.
- [ ] 1p.2 `grex sync E:\repos\utils\grex-org\` — walks the nested grex-org tree end-to-end.
- [ ] 1p.3 Re-run sync immediately: idempotent, byte-identical lockfiles.
- [ ] 1p.4 `grex ls E:\repos\utils\grex-org\` — renders the nested tree correctly.
- [ ] 1p.5 `grex doctor E:\repos\utils\grex-org\` — recursive default; reports clean.
- [ ] 1p.6 v1.1.x fixture (e.g., `E:\repos\code` from v1.1.1 evidence) — auto-migrates cleanly; `.bak` written; new per-meta lockfiles correct.

### 1q — ship

- [ ] 1q.1 Squash-merge impl PR → `main`.
- [ ] 1q.2 Tag `v1.2.0` (annotated, on the squash commit). Push.
- [ ] 1q.3 Wait for `release.yml` (cargo-dist) to publish GitHub Release with archives + installers.
- [ ] 1q.4 Publish 4 crates topologically: `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`. Wait for index propagation between each.
- [ ] 1q.5 Verify `crates.io` `max_version: 1.2.0` for all 4.
- [ ] 1q.6 `cargo install grex-cli --force --version 1.2.0` then re-run real-world sync (1p) to confirm published binary matches local validation.
- [ ] 1q.7 Open the `pack-template` repo update PR demonstrating v1.2.0 nested-children layout.
- [ ] 1q.8 Update `progress.md` with v1.2.0 SHIPPED endpoint.
