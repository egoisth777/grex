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

- Routed `grex import --from-repos-json` manifest writes through the shared
  add registration path, keeping import and `grex add` on one event
  construction flow without changing the manifest schema.

### Deprecated

### Removed

### Fixed

- `grex doctor` now compares and repairs managed `.gitignore` blocks against
  the patterns emitted by built-in pack types, including the default
  `.grex-lock` entry and authored `x-gitignore` patterns.

### Security

## [1.3.0] - 2026-05-02

### Added

- `--pack` flag as primary alias for `--workspace` on sync, serve,
  migrate-lockfile, teardown verbs (clap alias).
- `pack` JSON envelope key on ls + doctor output (additive sibling of
  `workspace`).
- `pack` field on MCP `sync` tool SyncParams (precedence
  `pack.or(workspace)`).
- `pack: &'a Path` additive field on `ExecCtx<'a>` (mirrors `workspace`).
- Plugin-API UNSTABLE marker in plugin/mod.rs lib doc-comment.
- Behavior contract freeze table at `.omne/cfg/freeze-v1.3.0.md`
  (13 STABLE contracts).
- Operator + Rust-consumer migration guide at
  `.omne/cfg/migration-v1.3.0.md`.
- Deprecation warn-once helper
  `crate::cli::deprecation::warn_workspace_alias_used()` (tracing target
  `grex::cli::deprecation`).
- Tests: cli_alias deprecation warn, cli_json dual-emit, MCP sync pack
  precedence, e2e_v1_3_0_readiness_smoke extended with warn-once +
  dual-emit asserts.

### Changed

- CLI doc-noun rewrite: `<workspace>` → `<pack>` in doc-strings +
  manpages.
- Manpages regenerated via `cargo xtask gen-man`.

### Deprecated (carry-forward; deferral note)

- `--workspace` flag on CLI: deprecated since v1.3.0; warn-once via
  tracing on stderr; removal scheduled for v2.0.0.
- `workspace` JSON envelope key on ls/doctor: deprecated since v1.3.0;
  removal v2.0.0.
- `SyncParams::workspace` MCP field: deprecated since v1.3.0; removal
  v2.0.0.
- `PackLock::acquire` (sync), `Scheduler::permits`,
  `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const: previously planned for
  v1.3.0 removal per CHANGELOG entries v1.2.4/v1.2.5/v1.2.6 — DEFERRED
  to v1.4.0 to keep v1.3.0 strictly additive (CLI rename + freeze only).

## [1.2.6] - 2026-05-02

### Added

- Three new `TreeError` variants for finer-grained manifest-read failure
  routing: `ManifestPermissionDenied { path }`, `ManifestNotADir { path }`,
  `ManifestIo { path, source }`. Existing `ManifestRead(String)` retained
  as the catch-all fallback for unmatched `io::ErrorKind` cases.
  Additive under existing `#[non_exhaustive]` (no downstream impact).
- New unit tests `tree_error_routing_per_io_error_kind`,
  `walker_resolves_under_meta_root_capability`.
- New integration test `working_tree_drift_no_recur` in
  `crates/grex-core/tests/drift_norec.rs`.
- New Lean theorem `walker_subpath_resolution_bounded_by_meta_dir` in
  `proof/Grex/Walker.lean` formalising the cap-std capability-bounded
  resolution invariant. `#print axioms` reports `[propext]` only —
  no new bridge axioms required.

### Changed

- Walker filesystem surface (`walker.rs::remove_dir_all_symlink_aware`,
  `quarantine.rs::snapshot_recursive_copy`,
  `consent.rs::read_dir`+`remove_dir_all`) now routes through cap-std
  `Dir` capability handles instead of ambient `std::fs::*` calls.
  Eliminates the per-step path-reopen TOCTOU window. No public API change;
  the cap-std root is opened internally at the meta boundary.

### Internal

- `clippy::disallowed_methods` lint added at the `tree::walker`,
  `tree::quarantine`, `tree::consent` module level to block future
  ambient `std::fs::*` regression.

### Repo hygiene

- `.gitignore`: added `**/.grex/`, `claude-statusline-probe.txt`,
  `CUsers*claude-statusline-probe*` patterns to catch v1.2.0 distributed
  event-log runtime artifacts and the cc-cfg statusline-probe fossil.
- `.gitattributes` (new): pins `eol=lf` for all tracked files to prevent
  CRLF/NUL recurrence on cross-tool edits.
- `scripts/cleanup-drift.ps1` (new): one-shot fossil cleanup script.

### Notes

- No public API change. New `TreeError` variants are additive under
  `#[non_exhaustive]`. cap-std migration is implementation-internal.
- MSRV unchanged at 1.79 (carry-forward from v1.2.5; required for
  symlink-secure `remove_dir_all` + cap-std v3 compat).
- `.omne/cfg/manifest.md` rewritten in the SSOT repo (separate from grex
  per Rule 7) — events catalog now lists v1.0.x action-bracket events
  + v1.2.x quarantine events; lockfile schema clarifies `commit_sha`
  semantics.

## [1.2.5] - 2026-05-02

### Added

- A2 partial-clone cleanup: failed / skipped / cancelled clone
  outcomes now atomically remove the destination directory before
  surfacing the error, so a half-cloned `<dest>/.git/` cannot
  poison subsequent `grex sync` runs. New helper
  `cleanup_partial_clone` centralises the dest-removal logic and
  is invoked from the `Skipped`, `Cancelled`, and `Failed` arms of
  the walker clone outcome match.
- A3 pool deadlock guard: debug-only `PoolInstallDepthGuard` plus a
  thread-local `HELD_PACK_LOCKS` counter. A debug-assert fires if
  `pool.install` re-entry is attempted while a pack lock is held on
  the same thread, catching the re-entrancy class of deadlock at
  test time. Release builds compile the guard out (zero overhead).
- Quarantine GC + restore + retention: new `prune` and `restore`
  functions in `grex_core::quarantine`; `--retain-days N` CLI flag
  on `grex sync`; GC sweep exposed via
  `grex doctor --prune-quarantine [--retain-days N]`; restore
  exposed via
  `grex doctor --restore-quarantine TS[:BASENAME] [--force]`.
  Two new audit `Event` variants `QuarantineRestored` and
  `QuarantineGCSwept` written to the per-meta
  `.grex/events.jsonl`.
- New public types: `RetentionConfig` (retention policy carrier),
  `PruneReport` (GC sweep result envelope), and `RestoreReport`
  (restore operation result envelope) in `grex_core::quarantine`.
- New `Event::Unknown` forward-compat variant — silently dropped on
  read so older binaries tolerate future audit-log variants;
  refused on write to keep the writer surface authoritative.
- New `CheckKind::QuarantineGc` and `CheckKind::QuarantineRestore`
  variants surfacing the new `doctor` flags as first-class checks
  in the `DoctorReport`.
- `#[non_exhaustive]` retrofitted onto `QuarantineError`,
  `SyncMetaOptions`, `CheckKind`, and `DoctorOpts` so future PATCH
  releases can add variants / fields without a SemVer break.
- New tests: T-A2 (cleanup-on-fail invariant), T-A3
  (deadlock-guard debug-panic), T-Q1 / T-Q2 / T-Q3 / T-Q4
  (quarantine restore / gc / retention / audit-log), T-R1
  (`sync --retain-days` end-to-end retention sweep).

### Changed

- Lean axiom budget unchanged at 9 bridge / 4 types / 0 model.
  v1.2.5 added 2 new theorems whose kernel dependencies are
  `[propext]` only — no new axiom introduced; CI axiom-set gate
  asserts unchanged counts.

### Deprecated

- (No new deprecations in v1.2.5.) Carry-forward from v1.2.4,
  still slated for removal in v1.3.0: `PackLock::acquire` (sync
  variant), `Scheduler::permits`, `DEFAULT_MANAGED_GITIGNORE_PATTERNS`
  const.

## [1.2.4] - 2026-05-02

### Added

- Cooperative cancellation token in the parallel walker
  (`Arc<AtomicBool>`): on first cycle detection inside a `rayon`
  sibling iteration, in-flight siblings observe the flag and abort
  promptly instead of running to completion. Cancellation scope is
  per-Phase-3-fan-out: a cycle in one sub-pack cancels its siblings
  at the same fan-out level only; disjoint sub-trees continue
  independently. Recursive sub-fan-outs construct their own
  cancellation flag.
- Lean theorem `cancellation_terminates_promptly` — extends
  `sync_meta_inner_model` with a `cancelled : Bool` parameter and
  proves termination once the flag is set. `lake build` green; zero
  `sorry`; zero `admit`.
- New cancellation behavior test asserting prompt sibling abort on
  first `CycleDetected`.
- New T1 diamond-DAG spot-check test (shared descendant, no cycle —
  guards the cancellation path against false positives on legal
  diamonds).
- New proptest cycle generator producing arbitrary cyclic manifest
  graphs; asserts walker always returns `CycleDetected` (never loops,
  never panics).
- CI axiom-set gate: `.github/workflows/ci.yml` now asserts the
  exact axiom counts (substantive / bridge / model) and fails on any
  drift, closing the manual-counter trap surfaced in v1.2.1.
- e2e v1.3.0-readiness smoke test exercising sub-pack-under-meta-pack
  flow + basic action commands end-to-end (the v1.3.0 release-readiness
  AC).

### Changed

- Internal rename `visited` → `ancestors` in walker. The set was
  always a path-prefix (parent chain), never a global visit set; the
  new name matches the semantics already proven in Lean.
- Internal rename `OwnCycleGuard` → `VisitedInsertGuard` (carry-forward
  from M6 cleanup; symbol is internal — no public API impact).
- Doc cleanup on `sync_meta`: rustdoc now states the cancellation
  contract and links the Lean theorem.

### Deprecated

- `PackLock::acquire` (sync variant) — use `acquire_async` or
  `try_acquire`. Will be removed in v1.3.0.
- `Scheduler::permits` — internal handle no longer required by
  callers. Will be removed in v1.3.0.
- `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const — use
  `default_managed_gitignore_patterns()` accessor. Will be removed in
  v1.3.0.

## [1.2.3] - pending

### Fixed

- `sync_meta` cycle check now fires BEFORE depth-cap early-return in
  Walker Phase 3. Previously, a cyclic manifest with cycle length
  exceeding `max_depth` would silently truncate without surfacing
  `CycleDetected`. Closes B1.
- `pack_identity_for_child` no longer emits trailing `@` when ref is
  empty/None. Identities like `url:https://x.git@` are now
  `url:https://x.git`. Lean model `ChildRef.identity` updated to match.
  Closes B2.
- Cycle chain now includes root pack identity (`path:<root_dir>`).
  Operators see where in the on-disk tree a cycle started, not just
  child→child→child. Closes B4.

### Added

- 3 new unit tests covering diamond (shared descendant, no cycle),
  4-node cycle (`A→B→C→D→A`), and nested-prefix cycle (cycle inside
  acyclic outer arm).

### Migration note (v1.2.2 → v1.2.3)

**If you pattern-match on `TreeError::CycleDetected { chain }`:**

- v1.2.2 chain shape: `["url:<a>@<ref_a>", "url:<b>@<ref_b>", "url:<a>@<ref_a>"]` (children only).
- v1.2.3 chain shape: `["path:<root_dir>", "url:<a>@<ref_a>", "url:<b>", "url:<a>@<ref_a>"]` (root prefixed; trailing `@` omitted on empty/None ref per B2).

If your code parses chain elements:

- Skip the first element if it starts with `"path:"` (root identity).
- Do not assume `"@"` separator is always present in `"url:<url>"` elements.

## [1.2.2] - 2026-05-02

### Fixed

- Cycle detection now prevents infinite clone on cyclic manifests
  (`sync_meta` walker). Previously, a manifest declaring a cyclic pack
  graph (e.g. A→B→A or self-loop) would loop forever in Walker Phase 1,
  filling disk. v1.2.2 detects the cycle at Walker Phase 3 recurse edge
  and returns `TreeError::CycleDetected` with the ancestor chain. Closes
  the v1.2.1 BLOCKER tracked by the `#[ignore]`'d `e2e_cycle_aborts` test
  (now re-enabled).
- Algorithm proven by Lean4 theorem
  `Grex.Walker.sync_meta_no_cycle_infinite_clone`
  (`proof/Grex/Walker.lean`). Bridge: Lean `List.contains` ≡ Rust
  `HashSet.contains` for membership semantics; HashSet is
  perf-optimization only.

## v1.2.0 — 2026-04-30

### Added — Nested-Children Walker

- Distributed lockfile: each meta owns `<meta>/.grex/grex.lock.jsonl` with entries for ITS direct children only (Lean theorem W2).
- Walker Phase 1: 5-way DestClass classifier (Missing/PresentDeclared/PresentDirty/PresentInProgress/PresentUndeclared) per child + UntrackedGitRepos error aggregation.
- Walker Phase 2: prune-safety with recursive consent walk (Clean/DirtyTree/DirtyTreeWithIgnored/GitInProgress/SubMetaWithDirtyChildren).
- Walker Phase 3: parent-relative recursion into nested metas (sequential; rayon deferred to v1.2.x).
- Validator: rejects Unicode-NFC duplicates, colon/dollar/tilde-digit segments, Windows reserved names, NTFS reparse points, .git-as-file references.
- TOCTOU: `BoundedDir` primitive via cap-std (Linux openat2(RESOLVE_BENEATH) under the hood; Win/Mac via cap-std handles).
- `LockEntry.path` field with v1.1.1 read-fallback (path derived from id).
- SyncOptions: force_prune, force_prune_with_ignored, migrate_lockfile, recurse, max_depth.
- `--force-prune` / `--force-prune-with-ignored` CLI flags + audit-log entry on override.
- `grex ls` walks ManifestTree via read_lockfile_tree; nested rendering preserved.
- `grex doctor` walks recursively by default; new `--shallow N` flag for depth bounds.

### Lean4 Proof

- 4 new theorems: validator_strengthens_W1 (V1), classify_dest_total (C1), prune_only_on_clean_consent (C2), fold_tree_lockfile_partition (F1).
- 3 new bridge axioms in proof/Grex/Bridge.lean: git_in_progress_decidable, consent_walk_reflects_fs_state, sync_lock_partition.
- Bridge.lean now houses 9 propositional bridge axioms (extracted from Walker.lean / Scheduler.lean).
- New Types.lean module with shared model + 3 helper lemmas (descends_refl/trans/join).
- lake build green, zero sorry, zero admit. CI gates: theorem count ≥17, axiom counts (=9 in Bridge.lean, =3 in Types.lean), no axioms outside.
- SSOT documentation at `.omne/proof/impl-axiom-bridge.md` (separate repo).

### Migration (v1.1.x → v1.2.0)

- **Lockfile auto-migrate is OFF by default.** Walker errors with `LegacyLockfileDetected` when v1.1.1 single-flat lockfile is detected.
- Migrator module `lockfile::migrate_v1_1_1` is an isolated unit with no inbound callers from steady-state code paths. Designed for clean removal in a future minor release.
- Opt-in via `--migrate-lockfile` flag (TODO: CLI dispatcher to be wired in v1.2.1+ when subcommand surface is finalized).

### Renamed

- Top-level `lean/` → `proof/` directory.

### Internal

- 874 tests pass (~120 new since v1.1.1).
- Stage 0 LOCKED decisions: TOCTOU=hybrid, scheduler=rayon (deferred to v1.2.x), glyph=keep-legacy, Lean4=mandatory-gate, auto-migrate=default-off.

## [1.1.1] - 2026-04-27

### Added
- Walker synthesizes a scripted-no-hooks pack manifest in-memory when a child dir contains `.git/` but no `.grex/pack.yaml`. Plain-git children now walk end-to-end on `grex sync` without per-child `pack.yaml` authoring (the bootstrap pattern: `REPOS.json` + flat-sibling git repos).
- `LockEntry.synthetic: bool` field (default `false`, `#[serde(default)]` for forward compatibility) — true iff the pack manifest was synthesized.
- `grex doctor` reports synthetic packs as `OK (synthetic)`; JSON output gains `"synthetic": true` per entry.
- `grex ls` prefixes synthetic entries with `~` (tree mode) and adds `"synthetic": true` (JSON mode).
- New e2e test `crates/grex/tests/plain_git_children_sync.rs` covering plain-git child walk + idempotent re-sync + mixed-tree.

### Changed
- Pack-spec doc gains a "Plain-git children" section.
- Migration guide updated: `grex import --from-repos-json` + `grex sync` works end-to-end on bootstrap-pattern repos.
- `grex ls` is no longer a stub. From v1.1.1+, `grex ls [<pack_root>]`
  walks the workspace read-only and renders the pack tree (human and
  JSON modes). Previously `grex ls --json` returned
  `{"status": "unimplemented", "verb": "ls"}` and exit 0; now it
  returns `{"workspace", "tree": [...]}` and may exit 2 if the
  workspace is invalid (no root manifest). The MCP `ls` tool wires
  through the same `grex_core::build_ls_tree` helper, so CLI and
  MCP `ls` are field-aligned. Wrappers that polled `grex ls` as a
  "binary available?" probe should switch to `grex --version`.

### Migration notes

- **v1.1.0 lockfiles parse forward.** `LockEntry.synthetic` carries
  `#[serde(default)]`, so a pre-v1.1.1 lockfile decodes cleanly into
  the new struct (`synthetic` defaults to `false`). No on-disk
  migration is required for the lockfile.
- **PATCH semver justification.** v1.1.1 is additive: walker synthesis
  is a fallback that fires only when the legacy path errors, no
  `pack.yaml` schema break, and no public API break beyond a struct
  growing one field (now gated by `#[non_exhaustive]` for forward
  compatibility — see fix-sweep round 1 scope A).
- **`grex ls` exit-code change.** The stub→wired transition flips the
  `ls` exit code on a broken workspace (was always 0/stub, now 2/error
  when the root manifest cannot be loaded). Wrappers checking for
  binary presence via `grex ls`'s exit status should migrate to
  `grex --version`. Successful `ls` invocations still exit 0.

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

[Unreleased]: https://github.com/egoisth777/grex/compare/v1.3.0...HEAD
[1.3.0]: https://github.com/egoisth777/grex/releases/tag/v1.3.0
[1.2.5]: https://github.com/egoisth777/grex/releases/tag/v1.2.5
[1.2.4]: https://github.com/egoisth777/grex/releases/tag/v1.2.4
[1.2.3]: https://github.com/egoisth777/grex/releases/tag/v1.2.3
[1.2.2]: https://github.com/egoisth777/grex/releases/tag/v1.2.2
[1.2.0]: https://github.com/egoisth777/grex/releases/tag/v1.2.0
[1.1.1]: https://github.com/egoisth777/grex/releases/tag/v1.1.1
[1.1.0]: https://github.com/egoisth777/grex/releases/tag/v1.1.0
[1.0.3]: https://github.com/egoisth777/grex/releases/tag/v1.0.3
[1.0.2]: https://github.com/egoisth777/grex/releases/tag/v1.0.2
[1.0.1]: https://github.com/egoisth777/grex/releases/tag/v1.0.1
[1.0.0]: https://github.com/egoisth777/grex/releases/tag/v1.0.0
