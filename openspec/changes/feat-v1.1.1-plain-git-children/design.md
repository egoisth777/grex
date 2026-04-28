# feat-v1.1.1-plain-git-children — design

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)

## Root cause

The walker treats `.grex/pack.yaml` as the pack-existence signal. Pack-existence and git-repo-existence are not the same thing. Conflating them excludes every plain-git child from a tree-walk that nominally claims to manage "trees of git repositories as a single addressable graph."

Concretely: `crates/grex-core/src/tree/walker.rs :: Walker::handle_child` calls `self.loader.load(&dest)?` immediately after `resolve_destination`. The loader (`crates/grex-core/src/tree/loader.rs :: FsPackLoader::load`) returns `TreeError::ManifestNotFound(manifest_path)` whenever `<child>/.grex/pack.yaml` is missing, regardless of whether `<child>/.git/` exists. The walk aborts on the first such child.

## Decision: synthesize at the loader-fallback boundary, not in the loader

Two places this could plug in:

1. **Inside `FsPackLoader::load`** — change the loader to silently synthesize when the manifest is missing.
2. **At the walker's call site** — wrap the `self.loader.load(&dest)?` call in a fallback branch that, on `ManifestNotFound`, checks for `<dest>/.git/` and synthesizes.

Choice: **(2) at the walker's call site.** Reasons:
- The `PackLoader` trait contracts that `load` returns `ManifestNotFound` when no manifest exists. Silently synthesizing in the loader breaks the contract for every other caller (loader is also reachable from CLI verbs that want a real manifest only).
- Keeping the synthesis explicit at the walker makes the new code path visible in the walker's error-path graph — easier to reason about, easier to test in isolation.
- Test loaders (in-memory mocks) shouldn't suddenly grow a "do you have a `.git/` here too?" responsibility — that's a walker concern (the walker already checks `dest_has_git_repo`).

## Synthesis algorithm (approach A)

Pseudocode for the new walker fallback, replacing the current `let child_manifest = self.loader.load(&dest)?;` line in `Walker::handle_child`:

```rust
let child_manifest = match self.loader.load(&dest) {
    Ok(m) => m,
    Err(TreeError::ManifestNotFound(_)) if dest_has_git_repo(&dest) => {
        // Plain-git child: synthesize a leaf scripted-no-hooks manifest.
        synthesize_plain_git_manifest(child)
    }
    Err(e) => return Err(e),
};
```

Where `synthesize_plain_git_manifest(child: &ChildRef) -> PackManifest` returns:

```rust
PackManifest {
    schema_version: "1".into(),
    name: child.effective_path(),  // bare-name from the parent's children[] entry
    type_: PackType::Scripted,
    version: None,
    depends_on: vec![],
    children: vec![],
    actions: vec![],
    teardown: vec![],
}
```

Downstream consequences:
- `verify_child_name(&child_manifest.name, child, &dest)?` — passes by construction (synthesized name == `effective_path()` == what the parent declared).
- `validate_children_paths(&child_manifest)?` — empty `children` list, trivially OK.
- `state.push_node` records the node with the synthesized manifest. The walker also tags this node as synthetic (see "Lockfile shape change" below).
- `walk_recursive(child_id, &child_manifest, …)` — empty `children`, empty `depends_on`, returns immediately. Leaf by construction.

## Lockfile shape change

Add a `synthetic: bool` field to `LockEntry`:

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
    pub synthetic: bool,  // NEW: true iff pack manifest was synthesized in-memory
}
```

Properties:
- `#[serde(default)]` makes existing v1.1.0 lockfiles forward-compatible — missing field deserializes as `false`.
- Field is appended at the end of the struct so JSON-line ordering on write places it last (cosmetic; serde does not depend on field order).
- `LockEntry` does NOT currently carry `#[non_exhaustive]`. Adding the new field is therefore semver-additive on its own (existing struct-literal constructors will fail to compile only inside the workspace, where we update them in the same PR). No external crate consumes `LockEntry` at v1.1.0 (it lives in `grex-core` and is only constructed by sibling crates).

## Doctor behaviour under synthetic packs

`crates/grex-core/src/doctor.rs` (or its v1.1.0 successor location) gains a branch on `entry.synthetic`:

- **Skip** schema validation (no manifest exists on disk to validate).
- **Skip** action/teardown idempotence checks (no actions exist).
- **Run** gitignore drift checks (the child still IS a git repo; drift is meaningful).
- **Run** orphan-lock cleanup checks (same as any pack).
- **Report** as `OK (synthetic)` in human output. JSON output gains a `synthetic: true` field on the per-pack diagnostic.

## `grex ls` surfacing

`crates/grex/src/cli/verbs/ls.rs` gains a `~` prefix on synthetic-pack lines:

```
dev-env (meta)
├── warp-cfg (declarative)
├── ~ algo-leet (scripted, synthetic)
└── ~ neetcode (scripted, synthetic)
```

JSON output for `--json` mode adds `"synthetic": true` to entries. Final glyph choice TBD at impl time — `~` is the placeholder; `*` and `?` are alternatives if `~` clashes with shell-glob expectations in some terminals.

## Cycle detection unchanged

Synthetic packs declare `children: []` and `depends_on: []`. They cannot introduce cycles. The existing cycle detector (`Walker::handle_child` :: stack-push / stack-pop pattern) needs no changes — the recursion bottoms out at synthetic leaves the same way it bottoms out at any other empty-`children` manifest.

## Why NOT pick approach B (auto-bootstrap)

Auto-bootstrap writes `.grex/pack.yaml` into every flat-sibling git repo without one. Costs:

- **Pollutes 14 child working trees** with grex artefacts the user didn't author. The user must then either `git add` + commit those files in 14 separate sub-repos (turning a sync into a 14-PR campaign), or live with permanent uncommitted-changes warnings in every sub-repo's `git status`.
- **Couples sync to write-side actions on owned-but-unrelated repos.** A sync intent should not mutate child repos' working trees by default. v1.1.0 already deliberately avoided this for migration (auto-migration only touches the workspace root's `.grex/workspace/` legacy bucket, never sub-repo working trees).
- **Reversibility is theoretical.** Once 14 commits land in 14 different sub-repos, undoing is per-repo `git revert` work.

Synthesis (A) achieves the same end state (sync walks plain-git children) with zero on-disk side effects. Kept B as documented fallback in case the silent treatment of A turns out to confuse users.

## Why NOT pick approach C (relaxed `path:` regex)

C lets `children[].path` carry one level of nesting (`subdir/<bare-name>`). It does not solve the per-child-pack.yaml requirement at all — orthogonal fix to a different problem. Adopting C would still leave the user's `E:\repos\code` workspace unwalkable. Park.

## Why NOT pick approach D (scan-mode sync)

D walks any git repo at any depth, ignoring `pack.yaml` entirely. Costs:

- **Loses the meta→children explicit graph.** The pack-as-contract model is the foundation of doctor / manifest / lockfile / cycle detection. Scan mode is "sync any git thing here" — incompatible with everything that depends on a declared graph.
- **No pack-name → on-disk-path mapping.** The walker currently uses `child.effective_path()` to verify name/path agreement; scan mode has no parent-declared name to verify against.
- **Lockfile loses meaning.** Lockfile keys on pack id; scan mode produces packs with no parent-declared id.

A solves the user's actual case (declared meta-pack with plain-git children) without breaking any of these invariants. Park D; revisit only if A+B both prove insufficient.

## SemVer impact

**PATCH** (1.1.0 → 1.1.1). Justification:

- **User override callout**: this change is technically additive (new walker code path + new lockfile field), which under strict SemVer would warrant a MINOR bump. The user has explicitly chosen PATCH for this release. Future additive features should reassess MINOR vs PATCH on a per-change basis.
- Walker behaviour change is **additive** — the new code path activates only on `ManifestNotFound + dest_has_git_repo` (a state the v1.1.0 walker treated as a hard error). Workspaces that worked under v1.1.0 continue to work identically.
- `LockEntry::synthetic` is `#[serde(default)]` additive — v1.1.0 lockfiles parse cleanly under v1.1.1; v1.1.1 lockfiles parse cleanly under v1.1.0 (extra field ignored by older serde-renaming-into-struct readers, modulo serde's `deny_unknown_fields` which we do not use on `LockEntry`). Older readers therefore stay compatible.
- Doctor / ls surface changes are output-only; no consumer of structured output (JSON) gets a *removed* field.
- No CLI verb removed; no flag removed; no MCP tool schema field removed.

## Migration story

Zero-touch for v1.1.0 users:

- Workspaces with all-pack.yaml children: identical behaviour, zero diff in walk output.
- Workspaces with mixed (some pack.yaml, some plain-git) children: previously errored on first plain-git child; now walk to completion. New synthetic packs surface in `grex ls` with `~` marker; lockfile entries gain `synthetic: true`.
- Workspaces with all-plain-git children (the `E:\repos\code` case): previously couldn't sync at all; now sync end-to-end as if every child were a scripted-no-hooks pack.

No flag required to opt in; the new behaviour is the new default. Opt-out (force the v1.1.0 strict behaviour) is NOT exposed as a flag in this round — if real-world demand surfaces, add `grex sync --strict-manifests` later as a non-default opt-in.

## Risks

1. **Synthetic pack lockfile entries leak into doctor's "missing manifest" check.** Mitigation: doctor reads `entry.synthetic` and skips the missing-manifest assertion for synthetic entries. Covered by the doctor test added under tasks.md.
2. **`grex doctor --json` consumers parse `OK (synthetic)` as a string status.** Mitigation: keep the human string OK + add a structured `synthetic: true` field; consumers reading the structured field get the precise signal.
3. **Plugin-pack registration runs even for synthetic packs.** Synthetic packs declare `type: Scripted` (a built-in); no plugin lookup fires. Verify at impl time that no plugin-resolve path runs on synthetic entries.
4. **A child's `.grex/` exists but only contains `targets/` or `files/` (no pack.yaml).** Loader still returns `ManifestNotFound` for the missing `pack.yaml`; synthesis fires; the orphan `targets/` / `files/` are silently ignored. Acceptable — a hand-authored partial `.grex/` without `pack.yaml` is malformed input, and silent synthesis is no worse than the v1.1.0 hard-error.
