# feat-v1.2.1 — doc-debt + rayon scheduler + CLI migrate dispatcher + force-prune polish

**Status**: draft
**Milestone**: v1.2.1 (PATCH; additive only)
**Depends on**: v1.2.0 (SHIPPED 2026-04-30 — `main @ 2c1791d`, tag `v1.2.0`, all 4 crates live on crates.io)

## Why

v1.2.0 shipped the nested-children walker, distributed lockfile, 5-way DestClass, recursive consent, TOCTOU `BoundedDir`, and `--force-prune` flags. Five follow-up items were explicitly deferred at ship time (see `progress.md` "## Endpoint (2026-04-30, main — v1.2.0 SHIPPED)" → "Deferred to v1.2.1+"):

1. **mdbook doc-debt.** The user-facing concept docs (`grex-doc/src/concepts/*.md`) and the duplicated set under `man/concepts/*.md` were last touched only for the `lean/`→`proof/` rename. v1.2.0 walker semantics (nested-children, distributed lockfile, 5-way classifier, recursive consent, TOCTOU `BoundedDir`, `--force-prune`) are NOT documented in mdbook. Auto-generated CLI man pages (`man/*.1` via `cargo xtask gen-man`) and CHANGELOG are current. The gap is concept-level prose only.
2. **CLI `grex migrate-lockfile` dispatcher.** v1.2.0 Stage 1.h shipped the library-level migrator (`grex-core::lockfile::migrate_v1_1_1`) as an isolated module, but did NOT wire a CLI dispatcher. Users hitting the `v1.1.1 lockfile detected, run grex migrate-lockfile` error currently have no `grex migrate-lockfile` subcommand to run.
3. **Rayon parallel sibling sync.** v1.2.0 Stage 1.g shipped sequential walker (sound under the existing `sync_disjoint_commutes` axiom + single-permit semaphore). Cargo-style parallelism on siblings within one meta + sub-meta recursion in parallel was deferred — covered by the same `sync_disjoint_commutes` axiom, no new Lean4 theorem required.
4. **`grex doctor --scan-undeclared`.** v1.2.0's `grex doctor` walks the declared subtree recursively and reports validator errors per node, but does NOT scan for undeclared `.git/` directories beneath registered metas. The aggregated `TreeError::UntrackedChildren` only fires during `sync` walk; doctor needs a peer scan that surfaces the same paths without performing any sync.
5. **Optional `--quarantine` flag on `--force-prune`.** v1.2.0's `--force-prune` performs an irreversible `rm -rf` on dest. A `--quarantine` flag should snapshot the dest's full subtree into `<meta>/.grex/trash/<ISO8601>/<basename>/` before the delete fires. Snapshot failure aborts the prune (no delete). This is a non-simple algorithm with concurrency + ordering invariants → Lean4 proof-first per Rule 8.

All five are additive: existing manifests stay valid, existing lockfiles continue to load (the migrator is opt-in), the rayon scheduler reuses the M6 concurrency primitives + Lean4 invariants, the doctor flag is new, and the quarantine flag is new. SemVer = PATCH.

## What changes

A single coherent PATCH release covering the 5 deferred items. Delivery order is fixed by dependency + risk:

1. **mdbook doc-debt** (lowest risk, no code change).
2. **CLI `grex migrate-lockfile` dispatcher** (thin shim over shipped library).
3. **Rayon parallel sibling sync** (covered by existing axiom).
4. **`grex doctor --scan-undeclared`** (new walker mode, no schema change).
5. **`--quarantine` flag** (Lean4 proof-first, gated behind `quarantine_snapshot_precedes_delete` theorem).

## Stage-level decisions (LOCKED)

These were sealed at v1.2.1 kickoff. No deferral inside the PATCH cycle.

1. **SemVer = PATCH** (1.2.0 → 1.2.1). Every delta is additive: new flag, new subcommand, internal scheduler swap, new doc chapters. No existing behaviour changes; no field removal; no error-variant rename.
2. **Quarantine layout = `<meta>/.grex/trash/<ISO8601_timestamp>/<basename>/`.** Per-meta scope (each meta gets its own quarantine bucket inside its `.grex/`). Recursive snapshot — full subtree contents, not just the top dir. Audit-log entry to `<meta>/.grex/events.jsonl` BEFORE the copy fires (with `fsync`). Snapshot failure aborts the prune (no delete). The on-disk folder name is `trash/`; the conceptual feature name remains "quarantine" (Lean4 theorem, flag name, prose).
3. **Lean4 proof-first for item 5.** Theorem name: `Grex.Walker.quarantine_snapshot_precedes_delete`. The theorem must compile (`lake build` green, zero `sorry`, zero `admit`) BEFORE any `--quarantine` Rust code lands. Bridge axiom may be added if needed; documented in SSOT (`.omne/proof/impl-axiom-bridge.md`, separate `grex-inst` repo).
4. **Rayon scheduler = no new Lean4 theorem.** Covered by the existing `sync_disjoke_commutes` axiom (M6) + the single-permit semaphore + per-meta `.grex-lock` shipped in v1.2.0. The axiom proves disjoint-pack work commutes; rayon's work-stealing pool is a scheduling-strategy refinement under the same proof. No new bridge axiom required.
5. **mdbook scope split across two repos.** The `grex-doc/src/concepts/*.md` and `man/concepts/*.md` updates land in this `grex` branch (item 1). NEW SSOT files `.omne/cfg/force-prune.md` and `.omne/cfg/toctou.md` are authored in the SSOT (`grex-inst`/`grex-ssot`) repo and ship through that repo's separate commit channel — NOT in this `feat/v1.2.1` branch (per Rule 7: SSOT lives in a separate repo). Existing `.omne/cfg/walker.md`, `.omne/cfg/lockfile.md`, `.omne/cfg/concurrency.md` are canonical sources; mdbook content derives from them.

## Sub-features

### 1. mdbook doc-debt

**Scope (this branch — `grex` repo):**
- Update `grex-doc/src/concepts/walker.md` (or create) — nested-children walker, parent-relative resolution, distributed lockfile, 5-way `DestClass`, recursive consent.
- Update `grex-doc/src/concepts/lockfile.md` (or create) — distributed `<meta>/.grex/grex.lock.jsonl`, `LockEntry.path`, v1.1.1 read-fallback.
- Update `grex-doc/src/concepts/concurrency.md` — single-permit semaphore + per-meta fd-lock + manifest fd-lock; note the M6 invariant carries through v1.2.0.
- New `grex-doc/src/concepts/force-prune.md` — `--force-prune` / `--force-prune-with-ignored` semantics, audit log, future `--quarantine` flag preview.
- New `grex-doc/src/concepts/toctou.md` — TOCTOU `BoundedDir` primitive, hybrid cap-std + Linux openat2 internal, why this matters for `rm -rf`.
- Mirror each chapter to `man/concepts/*.md` (man-page concept variants).

**Scope (separate SSOT-repo commit, NOT this branch):**
- New `.omne/cfg/force-prune.md` — canonical SSOT prose; mdbook chapter derives from it.
- New `.omne/cfg/toctou.md` — canonical SSOT prose; mdbook chapter derives from it.

**Acceptance:**
- `mdbook build grex-doc/` exits 0; new chapters appear in nav.
- Each `grex-doc/src/concepts/*.md` chapter cites the corresponding `.omne/cfg/*.md` SSOT (link or "see SSOT" reference).
- `man/concepts/*.md` byte-equivalent (or path-rewritten copy) of `grex-doc/src/concepts/*.md`.

**Files touched (estimate):** 5 new + 3 updated in `grex-doc/src/concepts/`, parallel set in `man/concepts/`. ~600 lines of prose total. No code change.

**Verification:** `mdbook build grex-doc/` exit 0; `cargo xtask gen-man` exit 0 (no man-page drift since this is concept-only).

### 2. CLI `grex migrate-lockfile` dispatcher

**Scope:**
- Add `crates/grex/src/cli/verbs/migrate_lockfile.rs` — clap subcommand with `--dry-run` and `--workspace <path>` (defaults to cwd).
- Wire into `crates/grex/src/cli/mod.rs` verb dispatch.
- Subcommand calls `grex_core::lockfile::migrate_v1_1_1` (already shipped v1.2.0 Stage 1.h).
- `--dry-run` emits the diff (additions / removals / field-fills) without writing.
- Without `--dry-run`, atomically writes the migrated lockfile (existing temp+rename path).

**Acceptance:**
- `grex migrate-lockfile --help` lists `--dry-run` and `--workspace`.
- `grex migrate-lockfile --dry-run` on a v1.1.1 fixture prints the diff and exits 0; lockfile bytes unchanged.
- `grex migrate-lockfile` on a v1.1.1 fixture rewrites the lockfile to v1.2.0 schema; subsequent `grex sync` succeeds without the `v1.1.1 lockfile detected` error.
- `grex migrate-lockfile` on an already-v1.2.0 lockfile is a no-op (exit 0, lockfile bytes unchanged).
- Idempotent: running twice produces byte-identical output.

**Files touched (estimate):** 1 new `crates/grex/src/cli/verbs/migrate_lockfile.rs` (~80 LOC); 1-line update in `crates/grex/src/cli/mod.rs`; 1 integration test `crates/grex/tests/migrate_lockfile.rs`. No `grex-core` change (library already shipped).

**Verification:** `cargo test -p grex migrate_lockfile` green; manual run on a v1.1.1 fixture round-trips clean.

**Lean4 gate:** None (CLI-shim only, no algorithmic change).

### 3. Rayon parallel sibling sync

**Scope:**
- Replace the sequential `for child in children` loop in `sync_meta` Phase 1 (probe / lock) and Phase 3 (action exec) with `rayon::par_iter` (Phase 2 = consent walk stays sequential — single-pass tree decision).
- Single-permit semaphore at the manifest layer is preserved (the M6 invariant — only one writer to a given lockfile).
- Per-meta `.grex-lock` (fd-lock) is preserved.
- Sub-meta recursion enters its own rayon scope (work-stealing — sibling work in the parent scope can interleave with grandchild work in the child scope).
- New `SyncOptions::parallel: Option<usize>` field (default `None` = use rayon default = `num_cpus`).

**Acceptance:**
- A 4-sibling fixture, each sibling declaring 4 leaf children, completes in roughly the time of the slowest sibling chain (not the sum). Test uses `Barrier` for timing tolerance, asserts ≤ 1.5× slowest-chain time.
- Per-meta `.grex-lock` prevents two walkers from concurrently writing the same lockfile (regression test on contended writes).
- Manifest-layer single-permit semaphore prevents concurrent appends to the same `events.jsonl` (regression test).
- Idempotency: second sync exits 0 with byte-identical lockfiles at every level.
- Existing `sync_disjoint_commutes` axiom in `proof/Grex/Walker.lean` covers the disjoint-pack commute property; no new theorem.

**Files touched (estimate):** `crates/grex-core/src/sync/walker.rs` (Phase 1 + Phase 3 loops); `crates/grex-core/src/sync/options.rs` (new field); `crates/grex-core/tests/parallel_scheduler.rs` (new — ~150 LOC).

**Verification:** `cargo test -p grex-core parallel_scheduler` green; `cargo test --workspace` 874+ tests still green; `lake build` green (no new theorem; existing axiom suffices).

**Lean4 gate:** None new — `sync_disjoint_commutes` (M6 axiom, reused by v1.2.0 Stage 0.5) covers this. Reference cited in commit message + SSOT.

### 4. `grex doctor --scan-undeclared [--depth N]`

**Scope:**
- Add `--scan-undeclared` flag to `grex doctor`. When set, after the standard validator pass, scan each declared meta's filesystem for `.git/` directories that are NOT registered in the meta's `pack.yaml` `children:` list.
- Reuse the v1.2.0 `TreeError::UntrackedChildren { paths: Vec<PathBuf> }` aggregation logic from sync walker — same path normalisation, same `grex add <path>` fix-suggestion format.
- `--depth N` bounds the scan depth (default = unbounded); `0` = current meta only, `1` = direct children, etc.
- `--scan-undeclared` does NOT mutate state. Output is read-only diagnostic (printed to stdout in the same format as `sync`'s error variant).
- Exit code: 0 if no undeclared children; 1 if any found (so CI can gate on it).

**Acceptance:**
- A meta with 3 declared children + 2 undeclared `.git/` directories produces output listing both undeclared paths with `grex add <path>` suggestions; exit code 1.
- A clean meta (all `.git/` declared) produces no output; exit code 0.
- `--depth 0` only scans the current meta (skips child-meta filesystem walking).
- `--scan-undeclared` composes with the existing `--shallow` flag (which bounds the recursive validator pass) — both can be set independently.
- No filesystem mutation: a `--scan-undeclared` run on a clean working tree produces no `events.jsonl` append, no `.grex.lock` change, no lockfile change.

**Files touched (estimate):** `crates/grex/src/cli/verbs/doctor.rs` (flag wiring); `crates/grex-core/src/doctor/scan.rs` (new ~120 LOC); `crates/grex/tests/doctor_scan_undeclared.rs` (new ~100 LOC).

**Verification:** `cargo test -p grex doctor_scan_undeclared` green; manual probe on `E:\repos\code` (14 plain-git children) reports 0 undeclared (since they're all already declared via v1.2.0 sync).

**Lean4 gate:** None (read-only diagnostic, no state mutation, no concurrency).

### 5. `--quarantine` flag on `--force-prune` (Lean4 proof-first)

**Scope:**
- Add `--quarantine` flag to `--force-prune` and `--force-prune-with-ignored`.
- When set, before the `rm -rf` of `dest`, snapshot the entire subtree contents recursively to `<meta>/.grex/trash/<ISO8601_timestamp>/<dest_basename>/`.
- Audit-log entry to `<meta>/.grex/events.jsonl` BEFORE the copy fires (with `fsync`). Event schema: `{ "kind": "QuarantineStart", "ts": "<ISO8601>", "src": "<dest>", "snapshot": "<trash_path>" }`.
- Snapshot uses `cap-std` for cross-platform recursive copy with TOCTOU-safe boundary. Linux uses `cap-std`'s native syscalls (no separate openat2 path needed for copy — read side is a separate concern from `BoundedDir`'s write-boundary use).
- Snapshot failure ⇒ abort the prune. The `rm -rf` does NOT fire. `events.jsonl` gets a `QuarantineFailed` follow-up event; the original `dest` remains untouched.
- Snapshot success ⇒ proceed with `rm -rf` of original `dest`. `events.jsonl` gets a `QuarantineComplete` follow-up event after the delete succeeds.

**Layout (LOCKED at kickoff):**
- Path: `<meta>/.grex/trash/<ISO8601_timestamp>/<dest_basename>/`
- Per-meta (each meta has its own `.grex/trash/` bucket).
- Recursive snapshot (full subtree contents, not just top dir).
- ISO8601 format: `YYYY-MM-DDTHH-MM-SSZ` (colons replaced with hyphens for cross-platform path safety; `Z` denotes UTC).

**Lean4 gate (HARD — proof-first per Rule 8):**
- Theorem name: `Grex.Walker.quarantine_snapshot_precedes_delete`.
- Statement (informal): for every prune operation invoked with `--quarantine`, the snapshot copy completes successfully (full subtree present at `<meta>/.grex/trash/<ts>/<basename>/`) AND the audit-log event is fsynced BEFORE any `rm` syscall on the original `dest` is issued. If the snapshot fails or the audit log fsync fails, no `rm` syscall fires.
- Bridge axiom (if needed): added to `proof/Grex/Bridge.lean` connecting the Lean abstract copy-then-delete model to the Rust runtime; documented in SSOT (`.omne/proof/impl-axiom-bridge.md`, separate `grex-inst` repo).
- `lake build` must be green with zero `sorry` / zero `admit` BEFORE the Rust `--quarantine` code lands.
- Order of operations (Rule 8): (a) write/extend `proof/Grex/Walker.lean` → (b) `lake build` green → (c) THEN add Rust `--quarantine` code.

**Acceptance:**
- `grex sync --force-prune --quarantine` on a meta with a stale child snapshots the child's full subtree to `<meta>/.grex/trash/<ts>/<child_basename>/` BEFORE deleting the original.
- `events.jsonl` shows `QuarantineStart` (with fsync) BEFORE any unlink syscall — verified by strace/ETW trace test on Linux/Windows.
- Snapshot failure (e.g., disk full, permission denied) aborts the prune; original `dest` untouched; `QuarantineFailed` event recorded.
- Snapshot is recursive (subdirectory contents preserved verbatim, including `.git/` if present in the pruned tree).
- ISO8601 timestamp directory name is unique per prune invocation (prevents collision on rapid-fire prunes).
- Lean4 theorem `quarantine_snapshot_precedes_delete` proves under `lake build` with zero `sorry`/`admit`.

**Files touched (estimate):**
- `proof/Grex/Walker.lean` (new theorem + helper lemmas, ~80 lines).
- `proof/Grex/Bridge.lean` (new bridge axiom if needed, ~10 lines).
- `crates/grex-core/src/sync/quarantine.rs` (new ~200 LOC).
- `crates/grex-core/src/sync/walker.rs` (wire the flag into prune path, ~30 LOC delta).
- `crates/grex/src/cli/verbs/sync.rs` (clap flag, ~5 LOC).
- `crates/grex-core/tests/quarantine.rs` (new ~250 LOC, including failure-injection tests).

**Verification:**
- `lake build` green (HARD GATE — runs first).
- `cargo test -p grex-core quarantine` green.
- Trace-level test asserts ordering: `QuarantineStart` fsync → snapshot complete → `unlink` syscall.
- Failure-injection test asserts `rm -rf` NEVER fires when snapshot fails.

**Lean4 gate:** YES — proof-first, blocks all Rust work for this sub-feature.

## Acceptance criteria (release-level)

For v1.2.1 to ship as a clean PATCH:

1. All 5 sub-features land on `feat/v1.2.1` (or sibling branches stacked off it) and merge to `main` in delivery order.
2. `cargo test --workspace` 874+ tests green (existing baseline + new tests from each sub-feature).
3. `lake build` green; zero `sorry`; zero `admit`. New theorem `quarantine_snapshot_precedes_delete` present.
4. `mdbook build grex-doc/` exit 0; nav lists new concept chapters.
5. `cargo xtask gen-man` exit 0; man pages reflect new flags (`--quarantine`, `--scan-undeclared`, `migrate-lockfile` subcommand).
6. Real-world verify on `E:\repos\code` (14 plain-git children): `grex sync .` exit 0 (idempotent skip, parallel scheduler active); `grex doctor --scan-undeclared` exit 0 (clean tree); `grex migrate-lockfile --dry-run` exit 0 (already on v1.2.0 schema, no diff).
7. Installed `grex 1.2.1` reports new flags in `--help` output for `sync`, `doctor`, and the new `migrate-lockfile` subcommand.
8. CHANGELOG updated with PATCH entry covering all 5 items.
9. All 4 crates published to crates.io at `1.2.1`; tag `v1.2.1` on `main`.

## SemVer rationale

**PATCH** (1.2.0 → 1.2.1). Justification:

- Every change is additive: new flag (`--quarantine`, `--scan-undeclared`), new subcommand (`migrate-lockfile`), new internal scheduler strategy (rayon — same observable behaviour, better wall-clock), new doc chapters, new optional `SyncOptions` field (`parallel: Option<usize>`).
- No field removed. No error variant renamed. No existing flag semantics changed. No lockfile schema change. No `pack.yaml` schema change.
- The migrator subcommand is a new entry point for a v1.2.0-shipped library function — exposes existing capability, doesn't add new capability.
- The rayon swap is a scheduling-strategy refinement under the existing `sync_disjoint_commutes` axiom — proven safe by the v1.2.0 Lean4 gate.
- The quarantine flag is opt-in (default OFF); existing `--force-prune` semantics unchanged.

Per Rule 6: SemVer label is the maintainer's call. PATCH is the maintainer's ruling at v1.2.1 kickoff.

## Cross-references

- **Canonical algorithms (SSOT, separate `grex-inst` repo, mounted at `.omne/`):**
  - `.omne/cfg/walker.md` — parent-relative walker (v1.2.0).
  - `.omne/cfg/lockfile.md` — distributed lockfile schema.
  - `.omne/cfg/concurrency.md` — M6 + v1.2.0 concurrency primitives.
  - `.omne/cfg/force-prune.md` — NEW in v1.2.1, separate SSOT-repo commit.
  - `.omne/cfg/toctou.md` — NEW in v1.2.1, separate SSOT-repo commit.
- **Lean4 proof:** `proof/Grex/Walker.lean` (existing 14 theorems + new `quarantine_snapshot_precedes_delete`); `proof/Grex/Bridge.lean` (existing 9 bridge axioms + 1 new if needed for quarantine).
- **History context:** `.omne/cfg/history.md` — v1.2.0 ship + v1.2.1 deferred-items pickup.
- **v1.2.0 endpoint:** `progress.md` "## Endpoint (2026-04-30, main — v1.2.0 SHIPPED)" — origin of the 5 deferred items.
- **Per Rule 7:** `.omne/**` edits land in the SSOT repo, NOT in this `feat/v1.2.1` grex branch. SSOT changes ship through a separate commit channel.
