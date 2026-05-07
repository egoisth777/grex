# walker

How `grex sync` traverses your nested meta-pack tree under v1.2.0+ — phase by phase, with the rules that decide what to clone, what to recurse into, and what to refuse.

> Canonical source: [inst/walker.md](../../inst/walker.md) (SSOT, separate `grex-inst` repo). This page is the user-facing projection; the SSOT is normative for behaviour.

## What is a meta pack?

A **pack** is any directory carrying `<dir>/.grex/pack.yaml`. There are two flavours:

- **meta pack** — `pack.yaml` lists `children:`. Owns its own lockfile at `<meta>/.grex/grex.lock.jsonl`. Recursion enters here.
- **leaf pack** — `pack.yaml` has no `children:`. Holds actions, no lockfile.

The directory where you run `grex` is the **cwd-meta** — the entry point for the recursion. There is no longer a single global "workspace root" anchor (retired in v1.2.0); every recursion frame computes destinations against ITS own meta dir.

## Three changes vs. v1.1.x

1. **Parent-relative resolution.** `dest = current_meta.join(child.path)`. Each frame uses its own meta dir as the join anchor.
2. **Distributed lockfile.** Each meta has its own `<meta>/.grex/grex.lock.jsonl` listing ONLY direct children. Sub-metas are autonomous — a parent has zero knowledge of grandchildren. See [lockfile](./lockfile.md).
3. **Cargo-style parallel.** Direct siblings sync in parallel; sub-meta recursion fires in parallel across siblings. Bounded by [concurrency](./concurrency.md) primitives.

The walker is **manifest-graph-driven**, not filesystem-driven. It only ever visits paths declared by some live manifest's `children:` list. Undeclared directories on disk — even those carrying their own `.git/` — are NOT auto-discovered, NOT auto-registered. v1.1.1's sync-time auto-synthesis is **retired**; see [§5-way classifier](#5-way-classifier-phase-1).

## The three phases

`sync(cwd_meta)` runs three phases per recursion frame. Each frame is autonomous: load my own `pack.yaml`, sync only my direct children, then recurse.

### Phase 1 — sync direct children (parallel)

For each `child` in `manifest.children`, in parallel:

1. Compute `dest = canonical(cwd_meta.join(child.path))`. Pre-canonicalization rejects relative segments that would resolve outside `cwd_meta`.
2. Re-verify no path segment is a symlink crossing the parent boundary (see [§Symlink hardening](#symlink-hardening) and [toctou](./toctou.md)).
3. `mkdir -p dest.parent()` (idempotent — concurrent siblings sharing an ancestor like `tools/` race-safely).
4. Apply the **5-way classifier** (next section).
5. Upsert a `LockEntry` into `<meta>/.grex/grex.lock.jsonl`, keyed by canonical meta-relative POSIX `path`.

After all children settle, if any landed on the "untracked git" branch the walker returns `Err(UntrackedGitRepos(list))` with the **complete list** — no partial completion. Phase 2 and Phase 3 do not run for this frame.

#### 5-way classifier (Phase 1)

The walker examines `dest` and routes to exactly one of five branches (evaluated top-down, mutually exclusive):

| # | Pre-condition at `dest`                                       | Action                                                                                          |
|---|--------------------------------------------------------------|------------------------------------------------------------------------------------------------|
| 1 | Does not exist                                                | `git clone child.url dest --branch child.ref`                                                   |
| 2 | Exists AND is an empty directory                              | Treat as branch 1 — retry the clone (recovers a failed mid-clone that left an empty dest).      |
| 3 | `dest/.git` exists AND `dest/.grex/pack.yaml` does NOT        | Push onto the untracked list. NO synthesis under v1.2.0+; user must run `grex add <url> <path>`.|
| 4 | `dest` exists, is non-empty, AND lacks `.git/`                | Return `Err(DestOccupied(dest, content_summary))`. Foreign content; refuses to clone-over.      |
| 5 | `dest/.git` AND `dest/.grex/pack.yaml` BOTH present (registered pack) | `git fetch` + checkout `child.ref`. Skip-on-hash if `actions_hash` and SHA unchanged. |

Branches 1, 2, and 5 are the only ones that mutate `dest`. Branch 2 explicitly recovers a failed-mid-clone state, so a second `sync` always reaches branch 5 (idempotent). Branch 4 is a hard error — a typo or stale checkout that the walker refuses to silently destroy.

### Phase 2 — prune children removed from manifest

Read the lockfile. For each entry whose `path` is NOT in the current manifest's `children:` paths:

1. If `dest/.git` does not exist → drop the lockfile entry, no `rm -rf` (idempotent — already gone).
2. **Prune-safety check** (default-deny — bypass only with `--force-prune`):
   - HEAD SHA must match `entry.sha`.
   - Working tree must be clean (`git status --porcelain --ignored` empty — covers tracked edits AND ignored content like `target/` or `node_modules/`).
   - No in-progress git op (rebase, merge, cherry-pick, revert, bisect — see [force-prune §In-progress probe](./force-prune.md#in-progress-git-op-probe)).
   - **Recursive consent walk.** If dest contains its own non-empty `.grex/grex.lock.jsonl`, recursively check every grandchild for the same three conditions. Any dirty/in-progress grandchild → refuse the prune unless `--force-prune-recursive`.
3. `rm -rf dest` (delegated to platform-native helper).
4. Delete the lockfile entry (atomic rewrite).

Cleanup is **CLI-invocation-driven**, not eager. Removing a child from `pack.yaml` triggers prune on the next `grex sync` / `update`, not on edit. See [force-prune](./force-prune.md) for the full safety contract and audit log.

### Phase 3 — recurse into child metas (parallel, autonomous)

For each `child`, in parallel:

1. Compute `child_dest = cwd_meta.join(child.path)`.
2. If `child_dest/.grex/pack.yaml` exists, parse it.
3. If the parsed manifest has non-empty `children:`, recursively call `sync(child_dest)`.

Each recursion is a fresh autonomous frame: it loads its own manifest, walks its own lockfile, syncs its own direct children. Sibling sub-meta syncs run in parallel; the per-pack `.grex-lock` (see [concurrency §Per-pack PackLock](./concurrency.md#per-pack-packlock)) prevents two ops on the same pack path even across recursion frames.

## Recursive consent (`--with-children`)

Phase 2 prune semantics deliberately cascade safety checks down the sub-meta tree. A meta whose declared child has its OWN sub-children (grandchildren) cannot be silently pruned if any grandchild is dirty or has an in-progress git op.

Three flag levels graduate the override:

| Flag                              | Effect                                                                                                                                |
|-----------------------------------|---------------------------------------------------------------------------------------------------------------------------------------|
| (none — default)                  | Default-deny. Refuse on any SHA mismatch, dirty tracked file, dirty ignored file, in-progress op, or dirty grandchild.                |
| `--force-prune`                   | Bypass clean-tree assertions at the named dest. Still respects in-progress ops and still refuses if any grandchild is dirty.          |
| `--force-prune-with-ignored`      | Allow ignored content (e.g. `target/`, `node_modules/`) to be destroyed without warning at the named dest.                            |
| `--force-prune-recursive`         | Cascades the bypass to grandchildren. Required to prune past a dirty grandchild. See [force-prune §Blast radius](./force-prune.md#blast-radius). |

`grex remove --force <path>` is the per-path equivalent of `--force-prune`: it bypasses checks 2 and 3 at the named dest only. It does NOT cascade past one level.

## Validator rules — `child.path`

Applied at every recursion depth, identical rules:

| Rule                                                                          | Behaviour                                                                                |
|-------------------------------------------------------------------------------|------------------------------------------------------------------------------------------|
| Forward slash `/`                                                             | Allowed (multi-segment paths). Each segment must match `^[a-z][a-z0-9-]*$`.              |
| Backslash `\`                                                                 | Normalised to `/` at parse-time on all platforms.                                        |
| `..` segment (any position)                                                   | Rejected.                                                                                |
| Absolute path                                                                 | Rejected.                                                                                |
| Symlink crossing parent boundary                                              | Rejected post-canonicalization.                                                          |
| Empty path                                                                    | Rejected.                                                                                |
| Duplicate `path` across two `children:` entries                               | Rejected at parse-time as `DuplicateChildPath(path)`.                                    |
| `:` in any segment                                                            | Rejected (NTFS Alternate Data Streams).                                                  |
| `$` in any segment                                                            | Rejected (variable expansion / Windows special).                                         |
| `~digit` pattern (`progra~1`)                                                 | Rejected (Windows 8.3 short-name aliasing).                                              |
| NUL byte / control chars `\x01`-`\x1F`, `\x7F`                                | Rejected.                                                                                |
| Drive-letter prefix (`C:`, `D:`)                                              | Rejected.                                                                                |

Path segments are NFC-normalised at parse-time before deduplication. Two manifests declaring `caf\u00E9/foo` (NFC) and `cafe\u0301/foo` (NFD) collide post-normalisation.

## Untracked git policy (5-way branch 3)

v1.1.1's sync-time auto-synthesis (silently registering a plain `.git/` discovered at a declared dest) is **RETIRED**. Under v1.2.0+ the walker NEVER synthesises a manifest from a plain `.git/`. A declared dest with `.git/` but no `.grex/pack.yaml` is an **error**, never silently registered.

Contract:

- The walker collects ALL untracked git repos across one `sync` invocation.
- After Phase 1 completes for a frame, if any untracked were collected, the frame returns `Err(UntrackedGitRepos(list))` with the COMPLETE list of offenders.
- Phase 2 (prune) and Phase 3 (recurse) do NOT run for that frame.

User remediation: explicitly register each path with `grex add <url> <path>`. The walker has no opinion on which `url` is correct — that is operator-supplied by design.

The error message cites every untracked dir's absolute path so you can fix all in one batch rather than iteratively.

## Symlink hardening

`dest_has_git_repo(dest)` refuses symlinked destinations outright via `std::fs::symlink_metadata`. Closes the symlink-redirection attack: a parent declaring `path: code` against a meta where `<meta>/code -> $HOME` cannot trick the walker into operating on `$HOME/.git`.

**Reparse-point and gitfile policy.** Maintainer-locked: REJECT ALL Windows junctions and non-symlink reparse points. v1.2.0+ rejects on Windows: `IO_REPARSE_TAG_MOUNT_POINT` (junctions, `mklink /J`), all reparse points except proper symlinks, and gitfile `.git` (regular file containing `gitdir: ...`). POSIX symlinks accepted with the boundary check; Windows proper symlinks accepted with the same check (they have a proper security model since Win10). Junctions and gitfile `.git` are unconditionally rejected — no flag, no override.

For the dirfd-binding TOCTOU mitigation that closes the path-swap window between canonicalize and clone, see [toctou](./toctou.md).

## Cycle detection

Each recursion pushes `pack_identity_for_child(child)` (`url:<url>@<ref>`) onto an in-progress stack; a repeat returns `TreeError::CycleDetected`. Identity for the cwd-meta itself is path-keyed; for children it is URL+ref so the same repo at two distinct refs is distinct.

## Lockfile keying

Lockfile entries within a meta are keyed by the canonical relative POSIX path of the child within that meta — single segment for direct children, but the writer always normalises through the path-keyed code path. v1.1.x bare-name keys remain valid as the degenerate single-segment case; readers fall back to bare-name lookup for legacy entries. See [lockfile §Path keying and v1.1.1→v1.2.0 read-fallback](./lockfile.md#path-keying-and-v111--v120-read-fallback) for the full migration story.

## Cross-references

- Distributed lockfile schema, three readers, v1.1.1→v1.2.0 migration: [lockfile](./lockfile.md)
- Bounded semaphore + per-pack lock + Lean4 invariant: [concurrency](./concurrency.md)
- Force-prune semantics, audit log, blast radius: [force-prune](./force-prune.md)
- BoundedDir TOCTOU primitive (cap-std + Linux openat2): [toctou](./toctou.md)
- Manifest event log + crash recovery: [manifest](./manifest.md)
- Pack layout + `.grex/` contract: [pack-spec](./pack-spec.md)
