# feat-v1.1.0-flat-children-layout — drop hardcoded `.grex/workspace/` child layout; resolve children as flat siblings

**Status**: draft
**Milestone**: v1.1.0 (post-v1.0.3)
**Depends on**: v1.0.3 (current `main` HEAD `78f1c38`) — this branch contains openspec only; the implementation PR rebases onto post-merge `main`.

## Why

Three independent signals all point to the same defect: the runtime currently appends `.grex/workspace/` to the parent pack root before resolving children, but every authoritative source-of-truth says children must resolve as **flat siblings** of the parent pack root.

1. **Positioning misalignment.** Locked tagline (`memory/grex_positioning.md`): grex is a *nested meta-repo manager*. Real users layout their meta-repo with the parent at `E:\repos\code` and child repos as direct subdirectories `E:\repos\code\<child>`. The current default forces them into `E:\repos\code\.grex\workspace\<child>`, which is neither how anyone organises a multi-repo workspace nor what the spec advertises.

2. **Migration-doc contradiction.** [`grex-doc/src/guides/migration.md`](../../../grex-doc/src/guides/migration.md) walks users through `grex import` → `grex sync .` with no relocation step. The doc is correct; the runtime is wrong. Today's session reproduced this end-to-end against the user's real workspace at `E:\repos\code` and got:

   ```
   tree walk failed: pack manifest not found at .\.grex\workspace\algo-leet\.grex\pack.yaml
   ```

   The child `algo-leet` exists at `E:\repos\code\algo-leet\.grex\pack.yaml` — sync looked in the wrong place because `resolve_workspace()` injected `.grex\workspace\` between root and child name.

3. **Test-fixture contradiction.** [`crates/grex-core/tests/meta_recursion.rs`](../../../crates/grex-core/tests/meta_recursion.rs) already constructs its fixture as flat siblings (`root/parent` + `root/child-a`). The tests pass today only because they pass an explicit `--workspace` override that bypasses the broken default. The fixture proves the maintainers' mental model matches the spec; only the default code path disagrees.

## What changes (4 sub-changes)

### 4a. Code — drop `.grex/workspace/` default + fix backup-scan anchor

- [`crates/grex-core/src/sync.rs:643-649`](../../../crates/grex-core/src/sync.rs) — `resolve_workspace()` default branch returns `pack_root_dir(pack_root)` directly. The `.join(".grex").join("workspace")` chain is removed.
- [`crates/grex-core/src/sync.rs:1656`](../../../crates/grex-core/src/sync.rs) — `scan_recovery()` independently hardcodes `.grex/workspace/` for backup-file scanning. Replace with a recursive walk anchored at `pack_root_dir(pack_root)`.
- [`crates/grex-core/src/tree/walker.rs:184`](../../../crates/grex-core/src/tree/walker.rs) — unchanged. `self.workspace.join(child.effective_path())` already does the right thing once the anchor is fixed.
- `--workspace` flag still accepts a manual override (unchanged); only the *default* changes.

### 4b. Validator — enforce bare-name `children[].path`

- [`man/concepts/pack-spec.md:176`](../../../man/concepts/pack-spec.md) declares: *"`children[].path` must be bare name (no `/` or `\`)"*. No code currently enforces this.
- Add `child_path_validator` under [`crates/grex-core/src/pack/validate/`](../../../crates/grex-core/src/pack/validate/) wired into `run_all`.
- Regex: `^[a-z][a-z0-9-]*$` (matches the existing `name` rule).
- Reject (with clear error message): `path: ../escape`, `path: foo/bar`, `path: foo\bar`, `path: .`, `path: ..`, `path: ""`, `path: /abs`.
- New `PackValidationError` variant `ChildPathInvalid { name, path, reason }`.
- [`crates/grex-core/src/pack/mod.rs:165-172`](../../../crates/grex-core/src/pack/mod.rs) `effective_path()` keeps its current shape (it returns the string verbatim); validation runs *before* it is called, so by the time it runs, `path` has already passed the regex.

### 4c. Doc updates — clarify flat-sibling resolution

- [`grex-doc/src/concepts/pack-spec.md`](../../../grex-doc/src/concepts/pack-spec.md) — already says bare-name; add an explicit *"children resolve as flat siblings of the parent pack root"* sentence.
- [`grex-doc/src/guides/migration.md`](../../../grex-doc/src/guides/migration.md) — already correct; verify the steps end-to-end after the refactor.
- [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) — mirror update.
- [`.omne/cfg/pack-spec.md`](../../../.omne/cfg/pack-spec.md) — mirror update.
- [`crates/grex/src/cli/args.rs`](../../../crates/grex/src/cli/args.rs) — `--workspace` help text drops the `.grex/workspace` reference; states the default is the pack root itself.

### 4d. Workspace version bump 1.0.3 → 1.1.0 + version-test bump + CHANGELOG

- `Cargo.toml` `[workspace.package].version = "1.1.0"`.
- `[workspace.dependencies]` internal `version = "1.1.0"` for `grex-core`, `grex-mcp`, `grex-plugins-builtin`.
- `crates/xtask/Cargo.toml` `grex-cli = { version = "1.1.0", ... }`.
- [`crates/xtask/tests/version_test.rs`](../../../crates/xtask/tests/version_test.rs) — `EXPECTED_WORKSPACE_VERSION` from `"1.0.3"` to `"1.1.0"`.
- `CHANGELOG.md` — add `[1.1.0] - YYYY-MM-DD` section under `[Unreleased]` describing the behavioural change in plain language plus the upgrade note (any in-flight sync must complete before upgrade — see [`design.md`](./design.md) "Lockfile path" section).

## What does NOT change

- `pack.yaml` schema — no field added or removed; `children[].path` semantics tighten but the field itself is unchanged.
- Public Rust API — `ChildRef`, `Walker::new()`, `effective_path()` signatures and return types unchanged. External library consumers unaffected by the runtime change.
- `--workspace` CLI flag — still allows manual override; only the *default* changes.
- Per-child `pack.yaml` requirement — children still require their own `.grex/pack.yaml` for recursive descent. Layout location of that file is the only thing that moves.
- Existing 7 actions (`symlink`, `mkdir`, `touch`, `chmod`, `git_clone`, `pkg_install`, `script`) — untouched.
- MCP tool surface — no tool added or removed; behaviour follows the runtime change.
- Release pipeline — `cargo-dist` config + `release.yml` workflow unchanged; the version bump rides through the existing pipeline.
- Doc-site workflow — `doc-site.yml` already triggers on `main` push (post PR #49); the doc updates ship automatically on merge.

## Acceptance criteria

1. `cargo test --workspace` green — all existing 682 tests pass.
2. New e2e `crates/grex/tests/import_then_sync.rs` passes — writes `REPOS.json` + sub-repos at flat-sibling layout, runs `grex import`, runs `grex sync .`, asserts every child cloned at the expected sibling path.
3. **Manual gate**: `grex sync E:\repos\code` (the user's real workspace, 14 children) walks the entire tree end-to-end with zero "pack manifest not found" errors.
4. `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns `1.1.0` only.
5. `cargo fmt --all -- --check` clean.
6. `cargo clippy --workspace --all-targets -- -D warnings` clean.
7. `mdbook build grex-doc/` exits 0 and `mdbook-linkcheck` reports zero broken links.
8. `cargo-dist`'s `dist plan` (release-plan CI job) green at `1.1.0`.
9. **Grep gate**: `rg -n '\.grex[/\\]workspace' crates/grex-core/src/` returns zero matches.
10. `cargo run -p xtask -- gen-man` man-drift gate green (no CLI surface change beyond `--workspace` help text).
11. Validator rejects `path: ../escape`, `path: foo/bar`, `path: foo\bar`, `path: .`, `path: ..`, `path: ""`, `path: /abs` with a `ChildPathInvalid` error citing the offending child name and the rejected string.
12. The `man-drift` job stays green (no CLI verb / flag added or removed).

## Source-of-truth links

- [`grex-doc/src/semver.md`](../../../grex-doc/src/semver.md) — versioning policy; justifies MINOR (default child resolution behaviour changes; wire-level break for the `~0` users on the previous default).
- [`man/concepts/pack-spec.md`](../../../man/concepts/pack-spec.md) §"Validation rules" line 176 — declares the bare-name rule that this PR finally enforces.
- [`grex-doc/src/guides/migration.md`](../../../grex-doc/src/guides/migration.md) — describes the intended flat-sibling workflow already; the runtime catches up.
- [PR #49](https://github.com/egoisth777/grex/pull/49) — immediate predecessor (doc-site 404 fix + workflow decouple); v1.1.0 branches off post-merge `main`.
- `memory/grex_positioning.md` — locked tagline ("nested meta-repo manager") that motivates the layout choice.
- [`openspec/changes/feat-v1.0.1-doc-site/proposal.md`](../feat-v1.0.1-doc-site/proposal.md) — format / tone reference for this proposal.

## Justification for MINOR (not PATCH, not MAJOR)

Per [`grex-doc/src/semver.md`](../../../grex-doc/src/semver.md):

- **Why not PATCH**: default child resolution changes — packs that previously synced to `.grex/workspace/<child>` will now sync to `./<child>`. That is an observable wire-level difference even if no `pack.yaml` field changes. PATCH is reserved for bug-fixes-with-no-behaviour-shift; this is a deliberate behaviour shift.
- **Why not MAJOR**: no public API breaks (Rust signatures unchanged, `pack.yaml` schema unchanged, CLI verbs / flags unchanged). MAJOR is reserved for source-incompatible changes; nothing here is source-incompatible.
- **MINOR is the correct slot**: behaviour change at runtime + zero schema/API break + spec was always documented this way. Real-world impact: v1.0.0 published 2026-04-23 (3 days before this proposal); the population of users who deliberately built `.grex/workspace/`-rooted layouts is approximately 0; the change brings the runtime into alignment with the published spec.
