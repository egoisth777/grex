# feat-v1.1.1-plain-git-children — sync-walk plain git repos as scripted-no-hooks children

**Status**: draft
**Milestone**: v1.1.1
**Depends on**: v1.1.0 (already shipped — squash `e54dc64`, tag `v1.1.0`, all 4 crates live on crates.io)

## Why

1. **Locked positioning ("nested meta-repo manager") promises arbitrary-tree git management.** v1.1.0 only manages trees where every node already carries `.grex/pack.yaml`. The walker's `loader.load(&dest)?` raises `TreeError::ManifestNotFound` the moment a child is a plain git repo, even though git-pulling a plain repo is the most basic thing a "manages trees of git repositories as a single addressable graph" tool should do.
2. **Real workspace evidence.** User's `E:\repos\code` (14 plain-git children at flat-sibling layout, the very layout v1.1.0 just enabled) cannot sync end-to-end. `grex sync .` on that workspace surfaces `tree walk failed: pack manifest not found at .\algo-leet\.grex\pack.yaml` on the first leaf — auto-migration safety net engaged correctly, but the walk itself is excluded from finishing.
3. **Bootstrap-pattern precedent.** The grex-org bootstrap (`REPOS.json` + `.scripts/sync.py` shim architecture) treats plain git repos as first-class managed children. grex must do the same to be a true productization of the bootstrap pattern it descends from — anything less leaves a class of users (every existing repo-tree owner) outside grex's reach.

## What changes

Four candidate approaches were considered. Approach A is recommended; B kept as fallback if A's silent treatment feels too implicit; C and D rejected.

### (A) Lenient mode — synthetic scripted-no-hooks pack [RECOMMENDED]

Walker, when about to load a child manifest at `<workspace>/<bare-name>/`:

- If `.grex/pack.yaml` is present → existing behaviour (load via `FsPackLoader::load`, validate, recurse).
- If `.grex/pack.yaml` is **absent** AND `<child>/.git/` is **present** → synthesize an in-memory `PackManifest`:
  ```
  PackManifest {
      schema_version: "1",
      name: <bare-name>,
      type: Scripted,
      version: None,
      depends_on: [],
      children: [],
      actions: [],
      teardown: [],
  }
  ```
  No file is written. The synthesis happens at the loader-fallback boundary; the rest of the walker (cycle detection, name verification, edge recording) sees a normal manifest. Synthetic packs declare `children: []`, so they never trigger further recursion (leaf by definition).
- If both `.grex/pack.yaml` and `.git/` are absent → existing `TreeError::ManifestNotFound` (this is the genuine "you pointed at nothing" case, must stay an error).

`scripted` lifecycle on a hooks-less pack is already a no-op for `setup`, `update`, and `teardown` per pack-spec; `sync` is `git pull self` (same as every other type). Net effect: plain-git children get pulled, recurse stops at the leaf, exit 0.

### (B) Auto-bootstrap — sync-time write of `.grex/pack.yaml` into plain-git children [ALTERNATIVE]

Sync-time materialises a real `.grex/pack.yaml` (type=scripted, no hooks) into every flat-sibling git child without one. Reversible — user can `git rm` the file. Explicit on-disk evidence of grex's view.

Why kept as fallback rather than picked: pollutes 14 child working trees with grex artefacts the user didn't author. The user would need 14 separate commits in 14 separate sub-repos for a sync that should be a no-op intent. If A's silent synthesis turns out to confuse users in practice (no on-disk evidence of why a given repo was walked), revisit B.

### (C) Relaxed `path:` regex — allow `subdir/<bare-name>` [REJECTED]

Permits one level of nesting in `children[].path`. Affects cycle detection identity strings, doctor scope, lockfile path keys, and the v1.1.0 flat-sibling exploit gate. Doesn't help the per-child-pack.yaml requirement at all — orthogonal fix to a different problem. Park indefinitely.

### (D) Scan-mode sync — `grex sync --scan` walks any git repo at any depth [REJECTED]

Closest to user mental model ("sync everything git-like under here"), but loses the meta→children explicit graph. Becomes "any git repo at depth N gets pulled" — breaks the pack-as-contract model that doctor/manifest/lockfile rely on. Park; revisit only if A+B both prove insufficient.

## Spec deltas (under approach A)

### Walker

- New synthesis fallback in the manifest-load step. Pseudocode in [`design.md`](./design.md). Symbol-level location: `crates/grex-core/src/tree/walker.rs :: Walker::handle_child` (the `self.loader.load(&dest)?` call site) gains a fallback branch routed through a new `crates/grex-core/src/tree/loader.rs :: synthesize_for_plain_git` helper.
- Synthetic manifests carry no marker on the in-memory struct itself — instead, the walker records `synthetic: true` on the resulting graph node, surfaced through to the lockfile and doctor.

### Pack-spec doc

- New section "Plain-git children" added to `man/concepts/pack-spec.md` and the `grex-doc/src/concepts/pack-spec.md` mirror, explaining the implicit contract: no recursion, sync = git pull only, no hooks, `.grex/pack.yaml` not required.
- Section explicitly cross-references the `synthetic: true` marker in lockfile and the `~` marker in `grex ls`.

### `grex doctor`

- Synthetic packs report `OK (synthetic)` rather than missing-manifest error. Schema validation skipped (no manifest to validate); gitignore drift checks still run.

### `grex ls`

- Synthetic packs surface with a `~` prefix marker (or similar — final glyph TBD at impl time) so users see at a glance which packs are declared vs synthesized.

### Lockfile

- `LockEntry` (`crates/grex-core/src/lockfile/entry.rs :: LockEntry`) gains a `synthetic: bool` field, default `false` via `#[serde(default)]`. Additive — existing lockfiles parse forward-compatibly. Verified at impl time that downstream readers treat a missing field as `false`.

## What does NOT change

- `pack.yaml` schema v1 — every field stays.
- The bare-name regex on `children[].path` (`^[a-z][a-z0-9-]*$`) — v1.1.0 enforcement stays intact. Synthesis only changes what happens *after* path validation succeeds and the destination has no manifest.
- Public API signatures of `Walker::walk`, `PackLoader::load`, `LockEntry`'s existing fields.
- `--workspace` override semantics.
- `grex.jsonl` manifest JSONL on-disk format (no new field; the `synthetic` bit lives on the lockfile entry, not the manifest entry).
- MCP tool schemas.
- Cycle detection behaviour — synthetic packs have empty `children`, so they cannot introduce cycles.

## Acceptance criteria

1. New e2e test `crates/grex/tests/plain_git_children_sync.rs`: workspace with N flat-sibling plain-git children → `grex sync <workspace>` walks each, runs `git pull` on each, exits 0.
2. Idempotent re-sync passes (second run also exits 0; no unintended state change).
3. `grex doctor` reports synthetic packs as `OK (synthetic)`, not as missing-manifest errors.
4. `grex ls` distinguishes synthetic from declared (visible marker on synthetic entries).
5. Existing 703 tests still pass with no behavioural regressions on packs that DO have `.grex/pack.yaml`.
6. Real-world: `cargo install grex-cli --force --version 1.1.1` then `grex sync E:\repos\code` walks all 14 plain-git children end-to-end, exits 0, idempotent re-sync clean.
7. Workspace where `pack.yaml` types are mixed (meta + declarative + scripted real packs, plus synthetic-scripted plain-git children) all coexist in the same walk without error.
8. Mixed-tree e2e: a meta pack with one declared `children:` entry that resolves to a plain-git repo (no own `.grex/pack.yaml`) walks successfully.

## Source-of-truth links

- [`progress.md`](../../../progress.md) §"Endpoint (2026-04-27, v1.1.0 SHIPPED)" — v1.1.0 ship + the `E:\repos\code` exit-3 evidence that triggered this proposal.
- [`man/guides/migration.md`](../../../man/guides/migration.md) — referenced by impl PR for the migration-story refresh.
- [`grex-doc/src/concepts/pack-spec.md`](../../../grex-doc/src/concepts/pack-spec.md) §"The 3 built-in pack-types" — the v1.1.1 callout placeholder lands here.
- [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) §"The 3 built-in pack-types" — same callout, mirrored.
- `crates/grex-core/src/tree/walker.rs :: Walker::handle_child` — fallback insertion point (load step).
- `crates/grex-core/src/tree/loader.rs :: FsPackLoader::load` — `TreeError::ManifestNotFound` site that the synthesis path bypasses for plain-git children.
- `crates/grex-core/src/lockfile/entry.rs :: LockEntry` — `synthetic: bool` field addition site.
