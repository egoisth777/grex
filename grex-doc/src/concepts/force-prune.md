# force-prune

Default-deny safety contract for [walker Phase 2](./walker.md#phase-2--prune-children-removed-from-manifest) — and the `--force-prune` family of flags that override it. With audit log, blast-radius analysis, and a forward reference to the v1.2.1 `--quarantine` snapshot.

> Canonical source: [inst/walker.md §Cleanup semantics](../../../inst/walker.md) (SSOT, separate `grex-inst` repo). A dedicated `inst/force-prune.md` will land in the SSOT repo separately. This page is the user-facing projection.

## When does prune fire?

Cleanup is **CLI-invocation-driven**, not eager. Removing a child from `pack.yaml` `children:` triggers prune on the next `grex sync` / `update` invocation, not on edit. Phase 2 reconciles each meta's lockfile against its current manifest and `rm -rf`s any orphans — subject to the safety contract below.

| Property         | Behaviour                                                                                    |
|------------------|----------------------------------------------------------------------------------------------|
| Trigger          | Manifest edit removes child, then user runs any `grex` command that touches the meta.        |
| Scope            | `rm -rf <meta>/<child.path>` AND drop the lockfile entry.                                    |
| Eagerness        | CLI-invocation-driven (NOT filesystem-watcher-eager).                                        |
| Idempotency      | Re-running with the child still removed: lockfile already lacks entry, `rm -rf` is a no-op.  |
| Cross-meta       | Each meta cleans its OWN orphans only.                                                       |
| Safety           | Default-deny on dirty / SHA-mismatched / in-progress dest; bypass via `--force-prune`.       |

## Safety contract

Phase 2 must NOT silently destroy modified or shared content. Before `rm -rf`, the walker verifies the dest still matches the state recorded in the lockfile.

### Adversary scenario

User `cp -r`s a child folder into a sibling meta and re-registers it there, then removes the original entry from the source meta's `pack.yaml`. Without verification, the source meta's Phase 2 destroys the now-shared folder while the sibling meta still believes it owns it.

### The five checks

Default behaviour (override only with the `--force-prune` family below):

1. **Missing `.git/` at dest.** Treated as already-gone — drop the lockfile entry, no `rm -rf`. Idempotent.
2. **HEAD SHA mismatch** (`git rev-parse HEAD` ≠ `LockEntry.sha`). Abort with `Err(DirtyDestRefuseToPrune(path, lockfile_sha, dest_sha))`. The user has either rebased, fetched without resyncing, or the dest was swapped for foreign content.
3. **Dirty working tree** (`git status --porcelain --ignored` non-empty). Abort with `Err(DirtyDestRefuseToPrune(...))`. The user has uncommitted edits OR gitignored content (build artefacts, deps caches, e.g. `target/`, `node_modules/`) that prune would silently destroy.
4. **Sub-meta consent walk.** If dest contains `<dest>/.grex/grex.lock.jsonl` with non-empty entries, recursively check every grandchild for the same conditions. Any dirty/in-progress grandchild → refuse the prune unless `--force-prune-recursive`. `grex remove --force <path>` does NOT cascade past one level.
5. **In-progress git op probe.** Refuse if any of these exist at `<dest>/.git/`:
   - `rebase-merge/`, `rebase-apply/` (in-progress rebase)
   - `MERGE_HEAD`, `CHERRY_PICK_HEAD`, `REVERT_HEAD` (in-progress merge / cherry-pick / revert)
   - `BISECT_LOG`, `sequencer/` (in-progress bisect / sequencer)

   Even if HEAD SHA matches lockfile and the working tree is clean, an in-progress git op blocks prune. No flag bypasses this except `--force-prune-recursive` combined with explicit per-path `--force-prune`.
6. **Match — clean tree, SHA equal to lockfile, no in-progress op, no dirty grandchild.** `rm -rf` proceeds.

## The `--force-prune` flag family

| Flag                          | Effect                                                                                                                                 |
|-------------------------------|----------------------------------------------------------------------------------------------------------------------------------------|
| `--force-prune`               | Bypass clean-tree assertions (checks 2 and 3) at the named dest. Still respects in-progress ops (check 5) and still refuses if any grandchild is dirty (check 4). |
| `--force-prune-with-ignored`  | Allow ignored content (`target/`, `node_modules/`) to be destroyed without warning at the named dest. Useful when the only "dirty" content is a build cache. |
| `--force-prune-recursive`     | Cascades the bypass to grandchildren. Required to prune past a dirty grandchild.                                                       |

`grex remove --force <path>` is the per-path equivalent of `--force-prune`: it bypasses checks 2 and 3 at the named dest only, never cascades.

The flag family is **opt-in by design**: a typo in `pack.yaml` should surface as a refusal, not as data loss.

### Loss profile

Loss of ignored content (build artefacts, deps caches) is recoverable but expensive (re-compile / re-fetch). Loss of tracked dirty edits is unrecoverable. Loss of an in-progress rebase is unrecoverable from `--force-prune-recursive`'s vantage point even though the underlying commits are still in `.git/objects/` — the working state and rebase script are gone.

## Audit log

Every `--force-prune`, `--force-prune-with-ignored`, or `--force-prune-recursive` invocation appends an entry to `<meta>/.grex/events.jsonl` BEFORE the `rm -rf` fires:

```jsonl
{"op":"force-prune","ts":"2026-04-30T10:00:00Z","id":"<pack-id>","schema_version":"1","path":"<meta-relative path>","lockfile_sha":"<sha>","dest_sha":"<sha>","dirty_files":<n>,"ignored_size":<bytes>}
```

The audit entry is **fsync'd** before the deletion proceeds. A crash mid-prune leaves a recoverable trail of what was about to be destroyed:

- The fsync barrier guarantees the audit line hits stable storage before any unlink syscall fires.
- On recovery, `grex doctor` can read the orphan entry and report "force-prune was about to delete `<path>`; the dest is gone — no recovery possible without git or filesystem-level undelete".
- The audit lives in the same per-meta event log used by `add` / `rm` / `update` — see [manifest §events.jsonl event schemas](./manifest.md#eventsjsonl-event-schemas) for the common envelope and atomic-append guarantees.

## Blast radius

The blast radius of a `--force-prune` invocation is bounded as follows.

| Flag                          | Within scope (deletable)                                                                          | Out of scope (untouched)                                                          |
|-------------------------------|---------------------------------------------------------------------------------------------------|----------------------------------------------------------------------------------|
| `--force-prune`               | The named dest's tracked dirty edits at the top level                                              | Any grandchild with its own dirty edits or in-progress op (check 4 still refuses)|
| `--force-prune-with-ignored`  | All of `--force-prune` plus ignored content (`target/`, `node_modules/`, etc.) at the named dest    | Any grandchild's ignored content (check 4 still applies)                          |
| `--force-prune-recursive`     | The full sub-tree, including grandchildren's tracked dirty edits and ignored content               | Sibling metas (cleanup is per-meta — see [walker §Phase 2](./walker.md#phase-2--prune-children-removed-from-manifest)) |

The walker NEVER deletes outside the cwd-meta's own tree. Sibling metas and parent metas are unreachable from any `--force-prune` invocation.

## Recovery

Once `rm -rf` has fired, **there is no in-band recovery path** under v1.2.0. Options:

1. Restore from backup.
2. Use a filesystem-level undelete tool (`extundelete`, ntfsundel, etc.) — typically only succeeds on recent deletes against quiet filesystems.
3. If the deleted dest was a git working tree, `git/objects/` may still be available in a parent's `.git/modules/` subtree (only if grex's clone used submodule semantics — rare).

### v1.2.1 `--quarantine` flag (PLANNED, TBD)

The v1.2.1 release plans an opt-in `--quarantine` flag on `--force-prune` and `--force-prune-with-ignored` that snapshots the entire dest sub-tree to `<meta>/.grex/trash/<ISO8601>/<basename>/` BEFORE the `rm -rf` fires. Failure of the snapshot aborts the prune (no delete). The Lean4 theorem `Grex.Walker.quarantine_snapshot_precedes_delete` is the gate that lets the Rust implementation land — proof-first per the SSOT rule.

The conceptual feature name is "quarantine"; the on-disk folder is named `trash/`. Per-meta scope (each meta has its own `.grex/trash/` bucket). Not present in v1.2.0; see the v1.2.1 spec for the LOCKED layout decisions and acceptance criteria.

Until `--quarantine` lands, `--force-prune` is irreversible. The audit log is the only forensic trail.

## Cross-references

- Walker Phase 2 algorithm + 5-way classifier context: [walker](./walker.md)
- Lockfile entry + path keying (the lookup map for prune candidates): [lockfile](./lockfile.md)
- Per-meta manifest fd-lock + audit-append serialisation: [concurrency](./concurrency.md)
- Audit-log envelope + crash-recovery torn-line detection: [manifest](./manifest.md)
- BoundedDir TOCTOU primitive (the `rm -rf` itself is dirfd-bound): [toctou](./toctou.md)
