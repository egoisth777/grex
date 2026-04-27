# feat-v1.1.0-flat-children-layout — design

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)

## Root cause

`.grex/workspace/` was an **implementation accident**, not a designed layout. The spec ([`man/concepts/pack-spec.md:176`](../../../man/concepts/pack-spec.md)) has always declared `children[].path` as a *bare name* (no `/`, no `\`), and the migration guide ([`grex-doc/src/guides/migration.md`](../../../grex-doc/src/guides/migration.md)) has always described `import` + `sync` against a flat-sibling layout. Two lines of code disagreed:

- [`crates/grex-core/src/sync.rs:643-649`](../../../crates/grex-core/src/sync.rs) — `resolve_workspace()` default appends `.grex/workspace`.
- [`crates/grex-core/src/sync.rs:1656`](../../../crates/grex-core/src/sync.rs) — `scan_recovery()` independently hardcodes the same path for backup-file scanning.

No spec ever advertised `.grex/workspace/`. No fixture uses it. No doc references it as the intended layout. The two lines simply leaked an early prototype's directory choice into the public default.

## Layout: before vs after

The user's real workspace at `E:\repos\code` has 14 children, e.g.:

```
E:\repos\code\
├── .grex\
│   └── pack.yaml             (parent — meta pack listing 14 children)
├── algo-leet\
│   └── .grex\pack.yaml       (child — declarative pack)
├── grex-org\
│   └── .grex\pack.yaml
├── …12 more children, each with .grex/pack.yaml…
└── REPOS.json
```

### Before (broken default)

`grex sync E:\repos\code` resolves child `algo-leet` to `E:\repos\code\.grex\workspace\algo-leet` and looks for its manifest at `E:\repos\code\.grex\workspace\algo-leet\.grex\pack.yaml`. That path does not exist; the file is at `E:\repos\code\algo-leet\.grex\pack.yaml`. Sync fails with:

```
tree walk failed: pack manifest not found at .\.grex\workspace\algo-leet\.grex\pack.yaml
```

User's only escape today is to pass `--workspace E:\repos\code` explicitly to bypass the broken default — but the spec promises no workspace flag is required.

### After (flat-sibling default)

`grex sync E:\repos\code` resolves child `algo-leet` to `E:\repos\code\algo-leet` and finds its manifest at `E:\repos\code\algo-leet\.grex\pack.yaml`. Walks all 14 children. No `--workspace` flag needed.

## Resolution algorithm

One line, one function:

```text
resolve(parent_pack_root, child_ref) := pack_root_dir(parent_pack_root).join(child_ref.effective_path())
```

- `pack_root_dir(p)` already exists at [`crates/grex-core/src/sync.rs:652-662`](../../../crates/grex-core/src/sync.rs); it strips a trailing `pack.yaml` to get the pack root directory. Unchanged.
- `child_ref.effective_path()` already exists at [`crates/grex-core/src/pack/mod.rs:165-172`](../../../crates/grex-core/src/pack/mod.rs); returns the bare-name `path` field or derives one from the `url` tail. Unchanged.
- `Walker::resolve_destination` at [`crates/grex-core/src/tree/walker.rs:184`](../../../crates/grex-core/src/tree/walker.rs) already calls `self.workspace.join(child.effective_path())`. Unchanged. The walker is layout-agnostic; the only thing that moves is the value of `self.workspace`.

The fix is anchor-only. No new types, no new traits, no signature changes.

## Cycle detection

Unchanged. Cycle keys remain:

- `(url, ref)` pair for children (same as today — the layout choice has nothing to do with cycle identity; what matters is which remote ref is being walked).
- `path:<display>` for the root pack (when no upstream URL is known).

The flat-sibling default does not change the dimensionality of the cycle space — it only relocates where on disk the same nodes materialise.

## Lockfile path migration concern

Today: `<workspace>/.grex.sync.lock` resolves to `<pack_root>/.grex/workspace/.grex.sync.lock`.
After: `<workspace>/.grex.sync.lock` resolves to `<pack_root>/.grex.sync.lock`.

**Migration impact**: any in-flight sync on the previous version that crashed and left an orphaned lock file at the old path will not be discovered by the new version's lock check. The orphaned file is harmless (it just sits there) but it does mean the user might see disk-noise after upgrading.

**Mitigation**: documented in CHANGELOG `[1.1.0]` upgrade notes — recommend running `grex doctor` after upgrade to detect and clean up the orphaned `.grex/workspace/.grex.sync.lock` if any. This is not a blocker; it's a one-time hygiene note.

## Backup recovery (`*.bak` files)

`scan_recovery()` at [`crates/grex-core/src/sync.rs:1654-1660`](../../../crates/grex-core/src/sync.rs) walks the workspace looking for backup files. Today it walks `pack_root.join(".grex").join("workspace")` then *also* walks `pack_root` itself (lines 1657-1660). The "also walk pack_root" line was a defensive fallback for symlink destinations that live at the top of the tree.

After the change: walk anchored at `pack_root_dir(pack_root)` directly. The "also walk pack_root" fallback collapses into the primary walk (they are now the same directory). No functional loss.

## Validator rationale

The bare-name rule has been declared in [`man/concepts/pack-spec.md:176`](../../../man/concepts/pack-spec.md) since v1.0.0 but never enforced. Enforcing it now is a consequence of the layout change, not an add-on:

- Without the rule, a user could write `path: ../sibling` to escape the parent pack root entirely.
- With the old `.grex/workspace/` default, that escape was bounded inside `.grex/workspace/`. With the new flat default, the escape lands directly under `pack_root` siblings — a far less recoverable mess.
- Enforcing the rule closes the path-traversal smell *before* anyone has a chance to depend on the loophole.

Variant placement: new `PackValidationError::ChildPathInvalid { child_name: String, path: String, reason: String }` added to [`crates/grex-core/src/pack/validate/mod.rs`](../../../crates/grex-core/src/pack/validate/mod.rs), wired into `run_all` alongside the existing 3 validators.

## Why NOT support `path: ../sibling` or `path: ../../shared/foo`

Three independent reasons, any one sufficient:

1. **Cycle detection edge cases**. Path-traversal lets the same git repo materialise at multiple absolute paths through different traversal routes. The `(url, ref)` cycle key still works, but the *display path* used in error messages becomes ambiguous, and the user-facing message "cycle detected via `child-a` → `child-b`" loses its locality.
2. **`git rm -r .grex` cleanup invariant**. Today users can `git rm -r .grex/workspace` to wipe all clones. Path-traversal breaks this invariant: child clones can land outside the cleanup target.
3. **`grex doctor` scan-bound**. Doctor walks the workspace to verify manifest consistency. With path-traversal allowed, the walk has no upper bound — it could escape into the user's home directory.

Bare-name keeps all three properties intact for a one-line cost (the regex check).

## Why NOT keep `.grex/workspace/` as opt-in

Considered: add a `children_root: ".grex/workspace"` field to `pack.yaml` for opt-in.

Rejected for three reasons:

1. **"One obvious way" principle**. Two layout modes is one too many. Either children are siblings or they are nested; making it user-configurable doubles the surface area of every doc, every error message, and every test fixture.
2. **Support burden**. Every future bug report would need to know which mode the reporter was in. The flag value would creep into every diagnostic output.
3. **No real users to protect**. v1.0.0 published 2026-04-23 (3 days ago at the time of this proposal); the population that deliberately built `.grex/workspace/`-rooted layouts is approximately 0. There is no backward-compatibility burden worth the cost of the opt-in.

If a future use-case emerges (which would be surprising — see "Positioning" memory note), adding `children_root:` later is a strict superset of the v1.1.0 behaviour and can be slotted in as a v1.2.0 MINOR.

## Out-of-scope for this PR

- **Auto-migration tool** for users with existing `.grex/workspace/` layouts. The expected user count is 0; if someone hits this they can `mv .grex/workspace/* ./ && rmdir .grex/workspace` manually.
- **Changes to `--workspace` flag semantics**. The flag still accepts an explicit override; only the *default* changes.
- **Changes to per-child `pack.yaml` requirement**. Children still need their own `.grex/pack.yaml`; the directory layout above that file is the only thing moving.
- **Changes to action surface or MCP surface**. Both unchanged.
- **Doc-site versioning** (v1.0 / v1.1 selector). Single-version site stays; revisit when v1.2 lands.

## Risks

1. **Stale lockfiles at the old path** (covered above) — harmless, documented, mitigated by `grex doctor`.
2. **A pack.yaml in the wild that uses `path: foo/bar`** — would now hard-fail validation. Mitigation: error message points at the offending child + spec section; CHANGELOG calls this out under "Breaking-by-correctness".
3. **Test fixtures elsewhere that rely on the old default** — searched: only `crates/grex-core/tests/meta_recursion.rs` constructs a workspace, and it already uses flat-siblings + explicit `--workspace`. No other test fixtures depend on the old default.
4. **`scan_recovery()` walks more files than before** — potentially. The old default walked only `.grex/workspace/`; the new default walks the full pack root. For meta-repos that's the same set of dirs (children-as-siblings); for declarative packs the recovery walk now sees the pack's own `actions/` outputs. Cost is bounded by the workspace size and is the *correct* behaviour (backup files anywhere in the workspace should be discoverable). Acceptable.
