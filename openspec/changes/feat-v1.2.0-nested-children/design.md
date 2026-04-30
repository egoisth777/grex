# feat-v1.2.0-nested-children — design

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**Sibling SSOT**: [`rust-design-decisions.md`](./rust-design-decisions.md) (in-flight, authored by rust-expert subagent — code-mechanism choices: TOCTOU crate, fd-lock layering, error-variant surface, concurrency primitives).

## Architecture

### Core model

- **Pack** = a directory containing a `.grex/` subdirectory. `pack.yaml` lives **inside** `.grex/` (corrected mid-session — it is `<pack>/.grex/pack.yaml`, never `<pack>/pack.yaml`).
- **Meta** = a pack with a non-empty `children:` list in its manifest.
- **Leaf** = a pack with empty `children:`.
- **Workspace** is no longer a global anchor. It is the cwd of the CLI invocation. Every meta is its own root for the purpose of resolving its children.

### Resolution rule

```
dest = current_meta.join(child.path)
```

No global workspace anchor. The recursion entry point is `cwd` of the CLI verb. Each meta resolves *its own* `children[].path` against its own directory; sub-metas resolve their children against themselves. A meta has no awareness of, and no authority over, its parent meta or its grandchildren.

### Distributed lockfile (β)

Each meta has its own lockfile at `<meta>/.grex/grex.lock.jsonl`. The lockfile tracks **direct children only**:

- A meta's lockfile lists exactly its direct children (one `LockEntry` per `children[]` entry).
- Sub-meta lockfiles are autonomous and authoritative for their own subtree.
- A parent meta does not read or write a sub-meta's lockfile during its own walk.
- Folding `grex ls` across the tree means reading each meta's lockfile in turn (one read per meta, depth-first).

`LockEntry` schema additions:

```rust
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct LockEntry {
    pub id: String,
    pub sha: String,
    pub branch: String,
    pub installed_at: DateTime<Utc>,
    pub actions_hash: String,
    pub schema_version: String,
    #[serde(default)]
    pub synthetic: bool,           // v1.1.1 — kept for backward-compat reads, dead under v1.2.0
    #[serde(default)]
    pub path: Option<String>,      // v1.2.0 — relative-to-meta dest; None falls back to `id` for v1.1.x reads
}
```

Read-time fallback rule for v1.1.x lockfiles: when `entry.path` is `None`, treat `entry.id` as the relative path (this is the v1.1.x flat-sibling invariant — `id == bare-name == path`).

### Recursion entry

The CLI verb's cwd is the entry meta. `grex sync` (no argument) walks the cwd's subtree. `grex sync <path>` walks `<path>`'s subtree. There is no concept of "the workspace" beyond "the meta you started from".

### Cargo-style parallel scheduler

Two parallel axes:

1. **Sibling parallelism** within one meta (existing M6-1 pattern — siblings without depends_on edges run concurrently).
2. **Sub-meta parallelism** across the recursion frontier. As soon as a sub-meta's parent finishes its prepare phase, the sub-meta walker is dispatched on the same scheduler. Depth-first ordering is *not* required.

The single scheduler instance is shared across all metas in the recursion. Each meta acquires an exclusive fd-lock on its own `.grex/` before writing its lockfile, preventing two walkers from racing on the same lockfile (which can happen if two parent metas both declare the same physical directory as a child — caught at validate-time, but the fd-lock is the belt-and-braces backstop).

### Synthesis retired (Stage 0 decision: keep-legacy `~` glyph)

v1.1.1's synthetic-pack fallback is removed at sync time. The new behaviour:

- Walker scans `<meta>/` for any directory containing `.git/` that is **not** declared in `<meta>/.grex/pack.yaml`'s `children:` list.
- Each such directory is collected into a `Vec<PathBuf>` accumulator.
- At end of walk (after all sibling and sub-meta work completes), if the accumulator is non-empty, the walker raises a single `TreeError::UntrackedChildren { paths }` listing every offending path with a one-line fix (`grex add <path>`).
- The `LockEntry.synthetic` field stays in the schema (forward-readable, read-only deprecated) but is always `false` for newly written entries under v1.2.0. The serializer omits the field when `false` to keep new lockfiles clean.

**`ls` synthetic glyph (Stage 0 LOCKED — keep-legacy).** `ls.rs` continues to render the `~` marker for any lockentry whose `synthetic` field is `true` (reading forward from v1.1.1 lockfiles). Newly written v1.2.0 entries never carry the marker because they never set `synthetic: true`. The glyph self-extincts as users re-sync; no flag, no migration script, no UX cliff. `ls.rs`'s synthesis *fallback* path (the v1.1.1 code that synthesized lockentries from on-disk `.git/` directories at render time) is dropped — `ls` now reads strictly from each meta's lockfile.

Deprecation path for the `synthetic` field: read-only in v1.2.0; never written; serializer omits when `false`. Removal deferred to 2.0 along with the rest of the v1.1.x compat surface.

### Cleanup

When a child is removed from a parent's `pack.yaml` (manifest delta), the next CLI command — sync, doctor, or remove — performs:

1. **Detect**: child is in lockfile but not in manifest → orphan.
2. **Recursive consent walk** on the orphan dest (if it is itself a sub-meta): refuse to prune if any descendant has dirty changes, in-progress git state (rebase/merge/bisect), or a fresh error variant.
3. **Prune**: `rm -rf <dest>` via the platform-native `rmtree.ps1`/`.sh` shim, then drop the corresponding lockentry from the parent's lockfile.

A `--force-prune` flag bypasses the consent walk (audit-log entry written) for tear-down workflows.

## 8 invariants (Lean-proven)

Each is a theorem in `proof/Grex/Walker.lean` (368 lines, `lake build` clean, 4 bridge axioms link the abstract walker to the Rust impl).

**Stage 0 LOCKED — Lean4 hard gate.** Per `.omne/schemas/rules.md` Rule 8: any v1.2.0 work introducing a non-simple algorithm beyond M6 reuse requires its Lean4 proof to compile clean (`lake build` green, zero `sorry`, zero `admit`) BEFORE any Rust change lands. The walker-invariant proof at commit `cee83d7` discharges I1–I8; new obligations (if any arise during impl) gate the corresponding Rust stages — see `tasks.md` Stage 0.5.

| # | Invariant | Theorem name | Proof method |
|---|-----------|--------------|--------------|
| I1 | **Boundary preservation** | `Walker.boundary_preserved` | Structural induction on the meta tree; symlink-cross-boundary excluded by validator. |
| I2 | **Distributed isolation** | `Walker.lockfile_isolation` | Each `LockfileWrite` event has a `meta_id` field; a meta's lockfile is touched only by its own walker frame. Proven by case-analysis on the event log. |
| I3 | **Termination** | `Walker.terminates` | Well-founded recursion on a strict-decreasing measure (remaining-undeclared-meta-count). |
| I4 | **Idempotency** | `Walker.idempotent` | Pure functional walker over a fixed meta tree; second walk produces the same lockfile-write event sequence (modulo timestamps, factored out). |
| I5 | **Sub-meta autonomy** | `Walker.sub_meta_autonomy` | A walker frame rooted at meta `M` writes only to lockfiles in `M`'s subtree; proven by induction on the recursion frame. |
| I6 | **No untracked (declared paths)** | `Walker.no_untracked` | Every `.git/` reachable under a declared meta is either declared (in lockfile) or surfaces in the `UntrackedChildren` error accumulator at end of walk. |
| I7 | **Cleanup safety** | `Walker.cleanup_safe` | Prune executes only when the recursive consent walk returns `Clean`; dirty/in-progress states block the prune. |
| I8 | **Concurrency safety** | `Walker.concurrency_safe` | Per-meta fd-lock guarantees mutual exclusion on lockfile writes; proven by reduction to the lock-acquisition order. |

Bridge axioms (4) live in `proof/Grex/Bridge.lean` and link Lean's abstract `Path`/`Meta`/`LockEntry` to the Rust types. Documented in `proof/Grex/Bridge.md`.

## Algorithm (lifted from `.omne/cfg/walker.md`)

Phase-structured walker, parent-relative. Pseudocode:

```text
fn walk(meta_dir: PathBuf, scheduler: &Scheduler, errors: &Accumulator) -> Result<(), TreeError> {
    // Phase 0: load + validate
    let manifest = load_manifest(&meta_dir)?;
    validate_manifest(&manifest)?;            // per-segment regex + NFC + windows-special reject
    let lockfile = read_lockfile_or_empty(&meta_dir);

    // Phase 1: 5-way branch per child
    for child in &manifest.children {
        let dest = meta_dir.join(&child.path);
        validate_dest_path(&dest, &meta_dir)?; // .., absolute, symlink-cross-boundary, junction, gitfile, NFC dup
        match classify(&dest) {
            DestClass::Missing             => actions::clone(child, &dest),
            DestClass::PresentDeclared     => actions::sync(child, &dest, &lockfile),
            DestClass::PresentDirty        => Err(TreeError::DirtyTree { dest }),
            DestClass::PresentInProgress   => Err(TreeError::GitInProgress { dest, kind }),
            DestClass::PresentUndeclared   => unreachable!(), // would have been caught at scan-undeclared
        }
    }

    // Phase 1b: scan for undeclared `.git/` siblings, accumulate into errors
    for entry in fs::read_dir(&meta_dir)? {
        if entry.is_dir() && entry.path().join(".git").exists() && !declared(&entry, &manifest) {
            errors.push_untracked(entry.path());
        }
    }

    // Phase 2: prune-safety pass
    for orphan in lockfile.entries_not_in(&manifest.children) {
        let consent = recursive_consent_walk(&orphan.dest)?;
        if consent.is_clean() {
            rmtree_native(&orphan.dest)?;
            lockfile.drop_entry(&orphan.id);
        } else {
            return Err(TreeError::PruneBlocked { dest: orphan.dest, reason: consent });
        }
    }

    // Phase 3: parent-relative recursion + cargo-parallel
    let lock = acquire_fd_lock(&meta_dir.join(".grex"))?;
    write_lockfile(&meta_dir, &lockfile)?;
    drop(lock);

    for child in manifest.children.iter().filter(|c| is_sub_meta(&meta_dir.join(&c.path))) {
        scheduler.dispatch(walk(meta_dir.join(&child.path), scheduler, errors));
    }

    Ok(())
}
```

The scheduler is shared across all `walk` invocations. The errors accumulator is a `Mutex<Vec<UntrackedPath>>` shared across the walk; `UntrackedChildren` is raised once at the end if non-empty.

## Validator rules

Per-segment validation on `child.path`:

- **Allow**: `/` as separator (relaxed from v1.1.1's bare-name regex).
- **Reject**: `..` (any segment), absolute paths (any segment starts with `/` or drive letter on Windows).
- **Reject**: symlinks whose resolved target escapes the parent meta dir (TOCTOU-safe via `cap-std` or `openat2(RESOLVE_BENEATH)` — see [`rust-design-decisions.md`](./rust-design-decisions.md)).
- **Reject**: Windows junctions, NTFS reparse points (any kind that is not a proper symlink created via `New-Item -ItemType SymbolicLink`).
- **Reject**: gitfile `.git` (a `.git` *file* pointing elsewhere, not a `.git/` directory).
- **Reject**: Unicode-NFC duplicate names within one meta's children (NFC-normalise both sides, compare).
- **Reject**: segments containing `:` (Windows ADS / drive separator), `$` (DOS device names), `~<digit>` (Windows short-name aliases like `~1`).
- **Reject**: reserved Windows segment names (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`).

## Error variants

New `TreeError` variants for v1.2.0 (additive — existing variants retained):

| Variant | Trigger |
|---------|---------|
| `UntrackedChildren { paths: Vec<PathBuf> }` | One or more `.git/` directories under a declared meta were not declared in its `pack.yaml`. |
| `PathEscapesParent { dest: PathBuf, parent: PathBuf }` | `..` or absolute path resolves outside the parent meta. |
| `SymlinkCrossesBoundary { src: PathBuf, target: PathBuf }` | A symlink in the dest path resolves outside the parent. |
| `WindowsReparseRejected { dest: PathBuf, kind: ReparseKind }` | Junction or non-symlink reparse point in dest path. |
| `GitfileRejected { dest: PathBuf }` | `.git` is a file, not a directory. |
| `UnicodeNfcDuplicate { meta: PathBuf, names: Vec<String> }` | Two child names normalise to the same NFC form. |
| `WindowsSpecialSegment { dest: PathBuf, segment: String }` | Reserved Windows name or special character in segment. |
| `GitInProgress { dest: PathBuf, kind: InProgressKind }` | Detached HEAD, mid-rebase, mid-merge, mid-bisect (per BLOCKER #4). |
| `PruneBlocked { dest: PathBuf, reason: ConsentRefusal }` | Recursive consent walk refused to prune. |
| `DirtyTreeWithIgnored { dest: PathBuf, paths: Vec<PathBuf> }` | `git status --porcelain --ignored` surfaces gitignored artefacts (per BLOCKER #6). |

Display impls follow the existing pattern — single-line user-facing message + a `--verbose` long form with the suggested fix (e.g., `grex add <path>` for `UntrackedChildren`).

## Concurrency

- **Distributed lockfile**: each meta's `.grex/grex.lock.jsonl` is written under a per-meta fd-lock acquired on `<meta>/.grex/` (a flock-style lock on a sentinel file). Two walkers cannot write the same lockfile concurrently.
- **Per-meta fd-lock**: a sentinel file at `<meta>/.grex/.lock` is `flock`-ed for the duration of the lockfile write. Lock is held only across the write, not the entire walk frame.
- **Cargo-parallel scheduler (Stage 0 LOCKED — rayon)**: a `rayon` sync work-stealing pool. Sibling tasks within one meta and sub-meta tasks across the frontier all share the same pool. Tokio is rejected: libgit2 is sync, `spawn_blocking` thread churn buys nothing, and there is no network-multiplexing payoff (each git fetch = one TCP/process). Rayon directly reuses the M6 concurrency primitives — bounded semaphore + per-pack `.grex-lock` + manifest fd-lock — whose correctness is already discharged by the Lean4 `I1 no_double_lock` invariant (commit `cee83d7`); the v1.2.0 scheduler inherits that proof rather than re-deriving it.
- **No global lock**: there is intentionally no workspace-wide lockfile to lock — that is the entire point of distributing the lockfile.

## Security

- **TOCTOU mitigation (Stage 0 LOCKED — hybrid `openat2(RESOLVE_BENEATH)` + `cap-std`)**: every dest path is resolved on a kernel-confirmed handle so the symlink-resolution check and the subsequent `fs::read_dir`/`clone` happen against the same boundary-enforced fd. Closes the `canonicalize(dest) → clone(dest)` race window.
  - **Linux**: `openat2(RESOLVE_BENEATH)` — single syscall, kernel-enforced boundary. Invocation via raw `libc::syscall(SYS_openat2, ...)` or the `openat2` crate (whichever has fewer transitive deps at impl time).
  - **Windows / macOS**: `cap-std` — capability-based dirfd handles, userspace, cross-platform. Acts as the fallback for platforms without `RESOLVE_BENEATH`.
  - The naive `Path::canonicalize` + `Path::starts_with` pattern is rejected unconditionally — known TOCTOU race, regardless of platform.
- **Symlink/junction/gitfile policy**: proper symlinks (created via `New-Item -ItemType SymbolicLink` on Windows, `ln -s` on POSIX) are *allowed* as long as the resolved target stays within the parent meta. Junctions, NTFS reparse points (other than symlinks), and gitfile `.git` files are rejected unconditionally.
- **Dirty-tree oracle expanded**: `git status --porcelain --ignored` (the `--ignored` flag is required to surface gitignored build artefacts that `--porcelain` alone would miss). In-progress state checks scan for `.git/rebase-merge/`, `.git/rebase-apply/`, `.git/MERGE_HEAD`, `.git/CHERRY_PICK_HEAD`, `.git/BISECT_LOG`, `.git/REVERT_HEAD`, and detached-HEAD detection via `git symbolic-ref HEAD` exit-1.

## Migration

### v1.1.x → v1.2.0 lockfile (Stage 0 LOCKED — default-OFF, explicit opt-in)

- v1.1.x lockfiles live at `<workspace>/.grex/grex.lock.jsonl` (single file, flat).
- v1.2.0 reader, when invoked from cwd `<meta>`, reads `<meta>/.grex/grex.lock.jsonl` first.
- **No silent rewrites.** If the reader detects a v1.1.1-shaped lockfile (single flat file at the cwd, no per-meta distribution), it errors out immediately:
  > `v1.1.1 lockfile detected, run grex migrate-lockfile`
- Migration is opt-in via the explicit `--migrate-lockfile` flag (or the `grex migrate-lockfile` subcommand). With the flag, the v1.1.x lockfile is split into per-meta lockfiles (one per declared meta) and the legacy file is renamed to `grex.lock.jsonl.v1_1.bak`.
- Per-`LockEntry`, missing `path` field falls back to `entry.id` (the v1.1.x bare-name == path invariant) — this fallback is the *read* path; it does not trigger a write.
- `synthetic: true` entries from v1.1.1 are accepted on read (preserving the `~` glyph in `ls`) but trigger a one-shot advisory log line per walk: `note: synthetic pack <id> — synthesis is retired in v1.2.0; consider 'grex add <path>' to declare it explicitly`.

Rationale: default-OFF preserves the SemVer-MINOR contract. A v1.2.0 binary on a v1.1.1 lockfile must never silently mutate user state on disk. The explicit error + named command keeps users in control of their own data.

### Migration module — isolation contract (Rule 9)

The v1.1.x → v1.2.0 lockfile migrator lives as an **isolated module** with no inbound callers from steady-state code paths. Concrete shape:

- Lives at `grex-core::lockfile::migrate_v1_1_1` (or equivalent — final path decided at impl time, but the module is single-purpose and self-contained).
- **Inbound callers**: only the `grex migrate-lockfile` subcommand and the `--migrate-lockfile` flag dispatcher. Walker, sync, ls, doctor, remove, add, init — none of these reach into the migrator.
- **Outbound dependencies**: read v1.1.x lockfile shape, write v1.2.0 per-meta lockfiles, rename legacy file. No coupling back to walker types or scheduler primitives.
- **Removal path**: in a future minor release (e.g. v1.4 or v1.5), once telemetry shows v1.1.x lockfiles are extinct in the wild, the entire migrator module + its CLI flag + its subcommand can be deleted in a single PR without touching any other unit. This is a deliberate constraint: the migrator is a temporary bridge, not architecture.
- **Test isolation**: migrator tests live in their own integration test file (`crates/grex-core/tests/lockfile_v1_1_compat.rs`); steady-state walker/sync/ls tests never invoke the migrator path.

### Event-log migration

The event-log is already v1.2.0-compatible (M6-2 made it pack-id-keyed, not workspace-keyed). No migration needed.

### Pack-template

The `pack-template` repo (seeded at v1.0.0) gets a v1.2.0 update PR demonstrating nested-children layout: a 2-level meta tree with `apps/` and `services/` subdirs, each its own meta with its own children. Lands post-merge.

### `SyncOptions` and MCP envelope

- `SyncOptions::workspace` and `SyncOptions::with_workspace()` retained, marked `#[deprecated(since = "1.2.0", note = "use cwd_meta")]`.
- New `SyncOptions::cwd_meta` and `with_cwd_meta()` added as the canonical API. The two are aliases on the same internal field for v1.2.0; removal of `workspace` deferred to 2.0.
- MCP `SyncParams.workspace` and `LsResponse.workspace` keep field names — value semantics shift from "workspace anchor" to "cwd meta entry point". MCP changelog entry documents the semantic shift; tool-schema diff is zero.

## Open questions (12 R2 BLOCKERs)

These are the 12 deduped BLOCKERs from the R2 review round. Stage 0 has now LOCKED the five mechanism-level decisions (TOCTOU, scheduler, ls glyph, Lean gate, lockfile migration default); the remaining items below carry their R2 resolutions verbatim.

1. **TOCTOU symlink chain (Stage 0 LOCKED — hybrid).** `openat2(RESOLVE_BENEATH)` on Linux (kernel-enforced boundary, single syscall) + `cap-std` on Windows/macOS (capability-based dirfd handles, userspace fallback). Closes the `canonicalize(dest) → clone(dest)` race window. Naive `Path::canonicalize` + `Path::starts_with` rejected on every platform. See §Security for invocation detail.
2. **Windows junctions / NTFS reparse / gitfile `.git`.** Reject all three unconditionally. Proper symlinks (`SymbolicLink` reparse tag) allowed if target stays within parent. Detection via `std::os::windows::fs::FileTypeExt::is_symlink_dir` + reparse-tag inspection (`fsutil reparsepoint query` heuristic, or direct `DeviceIoControl` if a crate exposes it).
3. **Validator missing Unicode-NFC normalize.** Add `unicode-normalization` crate dependency; NFC-normalise every segment before comparison. Add reject for `:`, `$`, `~<digit>`.
4. **Detached HEAD / mid-rebase not in dirty-tree oracle.** Expand the oracle: scan `.git/rebase-merge/`, `.git/rebase-apply/`, `.git/MERGE_HEAD`, `.git/CHERRY_PICK_HEAD`, `.git/BISECT_LOG`, `.git/REVERT_HEAD`. Detached HEAD via `git symbolic-ref --quiet HEAD` exit-1.
5. **Sub-meta transitive `rm -rf` violates [I5].** Add `recursive_consent_walk` before any prune. The consent walk is read-only — it never enters another meta's lockfile, only its on-disk state. Refusal returns a `ConsentRefusal` enum with the specific blocker.
6. **`.gitignore`'d build artefacts not in `git status --porcelain`.** Add `--ignored` flag to the porcelain call. Surface as `DirtyTreeWithIgnored` (separate variant from plain `DirtyTree` so users can distinguish "I have uncommitted changes" from "I have stale build artefacts").
7. **`LockEntry.path: Option<String>` — read-time fallback rule.** Documented above: `None` → `entry.id` for v1.1.x reads. New writes always populate `path`.
8. **`SyncOptions::workspace` + `with_workspace()` deprecation.** Add `cwd_meta` alias; deprecate `workspace` with `#[deprecated(since = "1.2.0", note = "use cwd_meta")]`. Remove in 2.0.
9. **MCP `SyncParams.workspace` + `LsResponse.workspace`.** Keep field names; shift value semantics. Document in MCP changelog. Zero tool-schema diff.
10. **Missing acceptance criteria block in walker.md.** Acceptance criteria moved into [`proposal.md`](./proposal.md) as the SSOT; `walker.md` cross-references back. Resolved at openspec time.
11. **test-plan.md is v1.0-vintage; no v1.2.0 scenarios.** New v1.2.0 test fixtures live under `crates/grex/tests/fixtures/nested-children/` and `crates/grex-core/tests/fixtures/`. test-plan.md updated in-flight.
12. **Sub-meta prune consent (overlaps walker-algo).** Resolved together with #5. The consent walk is the single mechanism for both BLOCKERs.

Non-blocking R2 CONCERNs (~25) are routed to in-flight fix agents; their resolutions land in subsequent commits referenced from the impl PR.

## Risks

1. **`cap-std` adoption may bring a transitive dep tree.** Mitigation: rust-expert evaluates dep-tree size in [`rust-design-decisions.md`](./rust-design-decisions.md); if the tree is heavy, an alternative is a thin in-house `RESOLVE_BENEATH` wrapper for Linux + a Windows-specific `CreateFileW` helper.
2. **Cargo-parallel scheduler complicates error reporting.** Mitigation: errors are accumulated in a `Mutex<Vec<TreeError>>`; the walker returns a single aggregated `TreeError::Multiple { errors }` if the accumulator is non-empty at end of walk. Per-error provenance (which meta raised it) is preserved.
3. **v1.1.x → v1.2.0 lockfile migration could surprise users by mutating on-disk state.** Mitigation (Stage 0 LOCKED — default-OFF): a v1.2.0 binary meeting a v1.1.1 lockfile errors with `v1.1.1 lockfile detected, run grex migrate-lockfile`. No silent rewrites, no `.bak` the user did not author. `--migrate-lockfile` is the explicit opt-in flag. The migrator is an isolated module (see §Migration module — isolation contract) deletable in a future minor release.
4. **Per-meta fd-lock can deadlock if the same physical directory is declared by two parents.** Mitigation: the validator catches duplicate-physical-dest at validate-time (canonicalise dest, compare; raise `DuplicateChildDest`). Fd-lock is the belt-and-braces backstop.
5. **Lean proof bridge axioms drift from Rust impl.** Mitigation: bridge axioms are documented in `proof/Grex/Bridge.md` with a "what each axiom assumes about the Rust side" section. Future Rust changes that touch bridge-relevant code paths must update the bridge doc; CI gate pending (out-of-scope for v1.2.0).
