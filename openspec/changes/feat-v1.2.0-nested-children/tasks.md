# feat-v1.2.0-nested-children — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: `.omne/walker.md` (canonical algorithm) · `proof/Grex/Walker.lean` (proof) · [`rust-design-decisions.md`](./rust-design-decisions.md) (in-flight; mechanism choices)

Markdown-only openspec PR first; implementation lands on a separate branch off post-merge `main`.

---

## Stage 0 — openspec PR (this branch)

- [ ] 0.1 Land openspec triplet under `openspec/changes/feat-v1.2.0-nested-children/` (proposal + design + tasks).
- [ ] 0.2 Cross-link `.omne/walker.md` → this triplet, and confirm walker.md acceptance-criteria block points back to `proposal.md`.
- [ ] 0.3 Update `progress.md` with the v1.2.0 openspec endpoint + refreshed "Where we are" block.
- [ ] 0.4 Confirm `proof/Grex/Walker.lean` builds clean under `lake build` and 14 theorems are present (W1–W8 + I1 + no_deadlock + V1/C1/C2/F1), 9 bridge axioms (Bridge.lean), zero sorry/admit.
- [ ] 0.5 PR description references the locked decisions: parent-relative resolution; distributed lockfile; rayon cargo-parallel (M6 reuse); synthesis retired with keep-legacy `~` glyph; hybrid `openat2(RESOLVE_BENEATH)` + `cap-std` TOCTOU; Lean4 hard-gate before any Rust impl; default-OFF lockfile auto-migrate; SemVer MINOR per maintainer override.
- [ ] 0.6 Required CI gates green (typos, build × 3, lake-build, etc.) — markdown-only, should pass trivially.
- [ ] 0.7 Stage 0.5 (Lean4 proof gate) is queued as a hard prerequisite for Stage 1; cannot start 1a until 0.5 is green.

---

## Stage 0.5 — Lean4 proof gate (HARD GATE, blocks Stage 1)

Stage 0 LOCKED decision #4: any new non-simple algorithm work — explicitly including concurrent algorithms beyond M6 reuse — requires the Lean4 proof to compile clean BEFORE any Rust code change lands. This stage is mandatory in v1.2.0; not deferred to v1.2.x.

The bridge-axiom proof at commit `cee83d7` covers walker invariants 1–8 (boundary preservation, distributed isolation, termination, idempotency, sub-meta autonomy, no-untracked, cleanup safety, concurrency safety). Reuse is fine where it covers; new obligations require new theorems.

- [x] 0.5.1 Audit Stage 1 algorithm surface (validator hybrid TOCTOU resolution, walker phases 1–3, prune-safety + recursive-consent, distributed-lockfile fold, scheduler dispatch, migrator isolation). Identify which obligations are covered by existing invariants W1–W8 + bridge axioms vs. which need new theorems.
- [x] 0.5.2 If any new obligation is identified beyond M6 reuse (e.g. rayon scheduler dispatch correctness, recursive-consent walk totality, distributed-lockfile fold isolation), add the theorem stub to `proof/Grex/Walker.lean` (or a sibling module under `proof/Grex/`) with a clear name and statement. List the new theorems explicitly in this task list before discharging them.
- [x] 0.5.3 Discharge each new theorem (no `sorry`). Run `lake build` and confirm zero warnings.
- [x] 0.5.4 Update bridge-axiom docs (now `.omne/proof/impl-axiom-bridge.md` in the SSOT, separate repo — superseding the in-tree `proof/Grex/Bridge.md`) for any new bridge axiom required to link a new theorem to the Rust impl. Document what each new bridge axiom assumes about the Rust side.
- [x] 0.5.5 CI gate: `lake build` step in `.github/workflows/ci.yml` (or equivalent) is mandatory and blocking — must already exist; confirm it covers any new files added under `proof/`.
- [x] 0.5.6 Snapshot the post-0.5 Lean state in this tasks file: theorem count, file count, `lake build` wall-time. Commit the snapshot before opening the Stage 1 PR.

**Stage 0.5.6 snapshot (post-discharge, branch `proof/v1.2.0-stage-0.5-lean-gate` @ `b44fdd9`):**

- **Theorems**: 14 total (W1–W8 walker invariants + I1 `no_double_lock` + `no_deadlock` scheduler corollary + 4 v1.2.0: `validator_strengthens_W1`, `classify_dest_total`, `prune_only_on_clean_consent`, `fold_tree_lockfile_partition`).
- **Bridge axioms**: 9 total (6 M6-era extracted to `Bridge.lean` in commit `9a8cd94` + 2 v1.2.0 added in commit `c23eba9` + 1 v1.2.0 added in D4 commit `b44fdd9`).
- **Proof files**: 7 (`.lean`): `Grex.lean` (root re-export), `Grex/Types.lean` (shared model), `Grex/Bridge.lean` (axioms), `Grex/Walker.lean` (W1–W8 + V1 + F1), `Grex/Scheduler.lean` (I1 + `no_deadlock`), `Grex/Phase1.lean` (C1), `Grex/Consent.lean` (C2).
- **`lake build` wall-time**: cold ≈ 1379 ms (post `lake clean`), warm ≈ 399 ms (steady-state CI cost estimate, measured locally on Windows). CI Linux runner expected within the same order of magnitude; tracked under Stage 0.5.F.
- **`sorry` / `admit` count**: 0 / 0 (verified by `Select-String` strict pattern `:= by sorry|:= sorry|:= by admit|:= admit` returning zero matches across `proof/Grex/`).
- **Stage 0.5 hard gate status**: SATISFIED at proof level. CI module (Stage 0.5.F) and SSOT docs (Stage 0.5.E) are concurrent commits closing the gate operationally.

- **HARD GATE**: Cannot proceed to Stage 1a until 0.5.1–0.5.6 are all checked and `lake build` is green with zero `sorry`.
- **Verification**: `cd proof && lake build` exits 0 with no warnings; `grep -r 'sorry' proof/Grex/` returns no matches.

---

## Stage 1 — implementation branch (after openspec PR merges and Stage 0.5 is green)

### 1a — branch baseline

- [ ] 1a.1 Branch `feat/v1.2.0-impl` off post-merge `main`.
- [ ] 1a.2 Confirm `cargo test --workspace` baseline green at HEAD before any code changes.
- [ ] 1a.3 Confirm `lake build` clean in `proof/`.
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

### 1d — TOCTOU mitigation: hybrid `openat2(RESOLVE_BENEATH)` + `cap-std` (Stage 0 LOCKED)

- [ ] 1d.1 Add `cap-std` to `crates/grex-core/Cargo.toml` for Windows/macOS dirfd handles. Add `openat2` crate (or wire raw `libc::syscall(SYS_openat2, ...)` — pick whichever has fewer transitive deps at impl time) gated `#[cfg(target_os = "linux")]`.
- [ ] 1d.2 On Linux: dest resolution opens the parent meta dir, then `openat2` with `RESOLVE_BENEATH | RESOLVE_NO_SYMLINKS` (or `RESOLVE_BENEATH` alone if proper symlinks must traverse) on the relative child path. Single syscall; kernel enforces the boundary.
- [ ] 1d.3 On Windows/macOS: dest resolution goes through `cap-std::Dir::open_ambient_dir(parent_meta)?.open_dir(child_relative)?` so the boundary check and the subsequent fs ops happen on the same dirfd handle.
- [ ] 1d.4 Replace every `Path::canonicalize` + `Path::starts_with` pattern in dest-resolution with the handle-based check. Add a clippy-friendly comment marking the naive pattern as TOCTOU-unsafe.
- [ ] 1d.5 Symlink target check happens on the kernel-confirmed handle (Linux) or the cap-std capability dirfd (Windows/macOS), never on the path string.
- [ ] 1d.6 New `TreeError::SymlinkCrossesBoundary { src, target }` variant.
- [ ] 1d.7 Test: a fixture with a symlink whose target escapes the parent meta is rejected with `SymlinkCrossesBoundary`. Test runs on POSIX + Windows (gated `cfg(unix)` / `cfg(windows)`); a separate Linux-only variant exercises the `openat2` path explicitly.
- **Verification**: `cargo test -p grex-core symlink_boundary`.
- **Depends on**: 0.5 (Lean gate green), 1c.

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

### 1g — walker Phase 3 — parent-relative recursion + rayon scheduler (Stage 0 LOCKED)

- [ ] 1g.1 Replace global-anchor resolution with `dest = current_meta.join(child.path)`.
- [ ] 1g.2 Recursion entry: cwd of the CLI verb invocation. Drop the `Workspace`-anchor concept from the walker module.
- [ ] 1g.3 Adopt **rayon** sync work-stealing pool (Stage 0 LOCKED). Sibling tasks within one meta and sub-meta tasks across the recursion frontier share one rayon pool. Tokio rejected: libgit2 is sync, no network-multiplexing payoff. Reuse the M6 concurrency primitives (bounded semaphore + per-pack `.grex-lock` + manifest fd-lock) — Lean4 `I1 no_double_lock` already proves these correct (commit `cee83d7`).
- [ ] 1g.4 Lockfile write under per-meta fd-lock (sentinel file at `<meta>/.grex/.lock`). M6 fd-lock primitive reused verbatim — no new locking primitive.
- [ ] 1g.5 If Stage 0.5 identified any new scheduler-correctness theorem (e.g. rayon dispatch totality, cross-meta isolation under work-stealing), confirm it is discharged and `lake build` is green before this task ships. Cite the theorem name in the impl PR body.
- [ ] 1g.6 Unit test: 4 sibling sub-metas × 4 leaves each completes in ~slowest-chain time, not sum (timing-tolerant via `Barrier`).
- **Verification**: `cargo test -p grex-core parallel_scheduler`.
- **Depends on**: 0.5 (Lean gate green), 1e, 1f.

### 1h — distributed lockfile (per-meta read/write/fold) + isolated migrator (Stage 0 LOCKED — default-OFF)

- [ ] 1h.1 Lockfile reader operates on `<meta>/.grex/grex.lock.jsonl` only — never reads sub-meta lockfiles transitively.
- [ ] 1h.2 Lockfile writer operates on `<meta>/.grex/grex.lock.jsonl` only — never writes sub-meta lockfiles.
- [ ] 1h.3 Fold operation for `grex ls`: depth-first walk reading each meta's lockfile in turn.
- [ ] 1h.4 **No silent rewrites.** When a v1.2.0 binary detects a v1.1.1 lockfile shape (single flat file at the cwd), it errors with a typed `TreeError::LegacyLockfileDetected { path }` whose Display is `v1.1.1 lockfile detected, run grex migrate-lockfile`.
- [ ] 1h.5 Add explicit opt-in surface: `--migrate-lockfile` flag on `grex sync` AND a standalone `grex migrate-lockfile` subcommand. Both routes invoke the same migrator entry point.
- [ ] 1h.6 **Migrator is an isolated module** (Rule 9 modular-removability). Place at `crates/grex-core/src/lockfile/migrate_v1_1_1.rs` (or equivalent single-file module). Inbound callers: only the `--migrate-lockfile` flag dispatcher and the `grex migrate-lockfile` subcommand. Walker, sync, ls, doctor, remove, add, init must NOT reach into the migrator. Outbound deps: read v1.1.x shape, write v1.2.0 per-meta lockfiles, rename legacy to `.bak`. No coupling back to walker types or scheduler primitives.
- [ ] 1h.7 With `--migrate-lockfile`: split v1.1.x lockfile into per-meta lockfiles, rename legacy file to `grex.lock.jsonl.v1_1.bak`. Without the flag: error path only; no on-disk mutation.
- [ ] 1h.8 Module-isolation test (lint-style): a CI check (or dedicated test) confirms no file outside `crates/grex-core/src/lockfile/migrate_v1_1_1.rs` and the CLI dispatcher imports the migrator's public functions. Documents the removability constraint.
- [ ] 1h.9 Test: v1.1.1 fixture errors out without `--migrate-lockfile`; with the flag, migrates cleanly; per-meta lockfiles correct; `.bak` present.
- **Verification**: `cargo test -p grex-core distributed_lockfile`. New test `lockfile_v1_1_compat.rs` (migrator-only, no walker calls).
- **Depends on**: 1b, 1g.

### 1i — `ls.rs` migrate to parent-relative + keep-legacy `~` glyph (Stage 0 LOCKED)

- [ ] 1i.1 `crates/grex/src/cli/verbs/ls.rs`: render the parent-relative tree by reading each meta's lockfile in sequence.
- [ ] 1i.2 Drop the v1.1.1 synthesis *fallback* path (the code that synthesized lockentries from on-disk `.git/` directories at render time). `ls` reads strictly from each meta's lockfile.
- [ ] 1i.3 `ls --json` output: nested children represented as nested JSON, one node per meta with its `path` (relative-to-parent) populated.
- [ ] 1i.4 **Keep-legacy `~` glyph**: preserve the `~` marker for any lockentry whose `synthetic` field is `true` (forward-read from v1.1.1 lockfiles). Newly written v1.2.0 entries never set `synthetic: true`, so the glyph self-extincts as users re-sync. Migration-window UX preserved.
- [ ] 1i.5 `LockEntry.synthetic: bool` is read-only deprecated. Confirm the serializer omits the field when `false` (clean v1.2.0 lockfiles); confirm v1.2.0 code never writes `true`.
- [ ] 1i.6 Snapshot/golden tests updated for the nested rendering. Add a fixture that mixes a v1.1.x lockentry (`synthetic: true`, `~` glyph rendered) with v1.2.0 entries (no glyph).
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
- [ ] 1o.8 `lake build` (proof/) clean — 14 theorems present, 9 bridge axioms documented in `proof/Grex/Bridge.lean` + `.omne/proof/impl-axiom-bridge.md`, zero sorry/admit (CI-gated).
- [ ] 1o.9 MCP conformance gate green.
- [ ] 1o.10 Workspace bump 1.1.1 → 1.2.0 across `Cargo.toml`, `[workspace.dependencies]`, `crates/xtask/Cargo.toml`.
- [ ] 1o.11 `crates/xtask/tests/version_test.rs` bump assertion.
- [ ] 1o.12 `CHANGELOG.md` `[1.2.0] - 2026-04-XX` section: parent-relative resolution; distributed lockfile; rayon cargo-parallel scheduler (M6 reuse); hybrid `openat2(RESOLVE_BENEATH)` + `cap-std` TOCTOU mitigation; synthesis retired (`~` glyph kept-legacy for v1.1.x reads); new validator rules; explicit `grex migrate-lockfile` opt-in for v1.1.x → v1.2.0 lockfile migration (default-OFF, no silent rewrites); deprecate `workspace` in favour of `cwd_meta`.
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
