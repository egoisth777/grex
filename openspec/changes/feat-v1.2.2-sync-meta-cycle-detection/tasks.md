# feat-v1.2.2 — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: `.omne/cfg/walker.md` §"Cycle detection" · `proof/Grex/Walker.lean`
**Branch**: `feat/v1.2.2` (cut from `main @ 2c23c6f`)
**Order of operations** (Rule 8 hard gate): Lean theorem proved BEFORE any Rust impl change.

---

## Stage 0 — openspec PR (this branch)

- [ ] 0.1 Land openspec triplet at `openspec/changes/feat-v1.2.2-sync-meta-cycle-detection/` (proposal + design + tasks).
- [ ] 0.2 Update `progress.md` with a v1.2.2 endpoint entry referencing this triplet + locked decisions (Option A, no new axiom, PATCH).
- [ ] 0.3 Confirm `cargo fmt --check` and `cargo doc -D warnings` baseline green at HEAD before any code changes.
- [ ] 0.4 PR body cites the locked decisions: scope = sync_meta cycle detection only; algorithm = Option A (visited stack threaded through `sync_meta_inner`); Lean theorem only, no new axiom; SemVer PATCH.

---

## Stage 1 — Lean4 proof (HARD GATE per Rule 8)

This stage MUST complete before any Rust file is modified.

- [ ] 1.1 Open `proof/Grex/Walker.lean`. Add the new theorem `sync_meta_no_cycle_infinite_clone` after `fold_tree_lockfile_partition` (currently the last theorem in the file). Statement per [`design.md`](./design.md) §"Lean4 spec".
- [ ] 1.2 Verify `manifest_forest_acyclic` is either present in `proof/Grex/Types.lean` or definable from existing primitives. If missing, define in `Types.lean` (definition only — NOT an axiom). Acyclicity = ∀ root-to-leaf path P, identities along P are pairwise distinct.
- [ ] 1.3 Verify `terminates_in_finite_steps` is similarly available or definable. If a fuel-based encoding is needed, add it as a pure `def` in `Types.lean`.
- [ ] 1.4 Discharge the theorem body. **Zero `sorry`, zero `admit`.** Prefer reusing the existing W3 `termination` proof's structural-recursion argument — extend it with the visited-stack hypothesis as the inductive invariant.
- [ ] 1.5 Run `cd proof && lake build`. Exit code 0, zero warnings, zero `sorry`/`admit` matches under the strict pattern (`grep -rE ':= by sorry|:= sorry|:= by admit|:= admit' proof/Grex/` returns no matches).
- [ ] 1.6 Confirm the axiom-policy CI step at `.github/workflows/ci.yml:218-265` still passes locally if reproducible — counts MUST remain `^axiom\b` count = 9 in `Bridge.lean` and = 4 in `Types.lean`. **No new axiom.** No `^axiom\b` declarations outside those two files.
- [ ] 1.7 Commit + push the Lean-only change as the FIRST commit on `feat/v1.2.2`. Commit message: `proof(walker): theorem sync_meta_no_cycle_infinite_clone (v1.2.2)`. CI must go green on this commit before Stage 2 starts.

**Gate**: Stage 2 cannot begin until Stage 1 commits CI run is green.

---

## Stage 2 — Rust impl (after Stage 1 green)

### 2a — thread visited stack through `sync_meta_inner`

- [ ] 2a.1 Open `crates/grex-core/src/tree/walker.rs`. Add `visited: &mut Vec<String>` parameter to `sync_meta_inner` (currently `:653-684`). Update the public `sync_meta` entry (`:643-651`) to construct the initial stack: `let mut visited = vec![pack_identity_for_root(meta_dir)];`.
- [ ] 2a.2 Add `visited: &mut Vec<String>` parameter to `phase3_recurse` (`:965-1010`). Thread the cycle check at the recursion edge — see [`design.md`](./design.md) §"Algorithm — Option A" for the exact pseudocode shape.
- [ ] 2a.3 Inside the `phase3_recurse` parallel closure: clone the visited stack per child (Option A.1 from design), push the child's `pack_identity_for_child` onto the local clone before descending, return `Phase3ChildOutcome::Failed(TreeError::CycleDetected { chain })` when the identity is already present.
- [ ] 2a.4 Confirm `pack_identity_for_root` (`:315-317`) and `pack_identity_for_child` (`:323-326`) are reused verbatim. No new identity helper.
- [ ] 2a.5 `cargo build -p grex-core` clean.
- [ ] 2a.6 `cargo clippy -p grex-core -- -D warnings` clean.

### 2b — re-enable `e2e_cycle_aborts`

- [ ] 2b.1 Open `crates/grex/tests/sync_e2e.rs:258`. Remove the `#[ignore = "v1.2.0 sync_meta lacks cycle detection ..."]` annotation.
- [ ] 2b.2 Update or remove the NOTE comment block at `:249-257` — the v1.2.0 gap is closed; the comment should now describe that v1.2.2 added the check at the recursion edge (or be deleted entirely if the test name + assertion are self-documenting).
- [ ] 2b.3 Run `cargo test -p grex --test sync_e2e e2e_cycle_aborts -- --nocapture`. Test must pass — the cyclic fixture surfaces `SyncError::Tree(TreeError::CycleDetected { .. })` instead of looping.

### 2c — new unit tests in `walker.rs::tests`

- [ ] 2c.1 In the existing `#[cfg(test)] mod tests` block at the bottom of `crates/grex-core/src/tree/walker.rs`, add `cycle_self_loop_aborts` — fixture: a meta whose `children[0]` references its own bare repo URL. Assert `sync_meta` returns `TreeError::CycleDetected` (or surfaces it in `report.errors`, matching the existing failure surfacing convention used by Phase 3).
- [ ] 2c.2 Add `cycle_three_node_aborts` — fixture: A→B→C→A. Assert detection fires on the C-to-A edge with the expected chain length.
- [ ] 2c.3 Add `two_refs_same_url_not_cycle` — POSITIVE test: parent declares `A@v1` and `A@v2` as siblings, both with their own dests. Both descend cleanly. No `CycleDetected` raised. Locks the URL+ref identity contract (a regression here would block legitimate diamond dependencies on different tags).
- [ ] 2c.4 `cargo test -p grex-core walker::tests` — all three new tests pass. Existing tests still pass.

### 2d — workspace version bump

- [ ] 2d.1 Bump root `Cargo.toml` `version = "1.2.1"` → `"1.2.2"` (line 12).
- [ ] 2d.2 Audit each member `Cargo.toml` for any non-workspace version pin. Bump those that pin to `1.2.1`. Members: `crates/grex/Cargo.toml`, `crates/grex-core/Cargo.toml`, `crates/grex-mcp/Cargo.toml`, `crates/grex-plugins-builtin/Cargo.toml`, `crates/xtask/Cargo.toml`.
- [ ] 2d.3 If `crates/xtask/tests/version_test.rs` exists, update its expected version to `1.2.2`.
- [ ] 2d.4 `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns exactly `1.2.2` for every grex-owned package.

### 2e — CHANGELOG / history.md note

- [ ] 2e.1 If `CHANGELOG.md` exists at repo root, prepend a `[1.2.2] - 2026-05-XX` section using the migration note from [`design.md`](./design.md) §"Migration note for changelog".
- [ ] 2e.2 Append the equivalent entry to SSOT `.omne/cfg/history.md` — note this commits to the SSOT repo separately (Rule 7), NOT to grex's git history.

### 2f — local gates

- [ ] 2f.1 `cargo fmt --all -- --check` clean.
- [ ] 2f.2 `cargo doc --no-deps --workspace` clean (run with `RUSTDOCFLAGS="-D warnings"` env var; matches CI gate).
- [ ] 2f.3 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] 2f.4 `cargo test --workspace` green (existing + v1.2.2 additions).
- [ ] 2f.5 `cd proof && lake build` still green (Stage 1 work persists).
- [ ] 2f.6 Local axiom-policy script — verify the regex anchors from `.github/workflows/ci.yml:218-265` reproduce locally: `^axiom\b` count = 9 in `Bridge.lean`, = 4 in `Types.lean`, zero elsewhere under `proof/Grex/`. (No standalone script file exists at this time; the policy lives inline in `ci.yml`. If a future commit extracts it, update this task to invoke that script.)
- [ ] 2f.7 `typos` clean.

### 2g — commit + PR

- [ ] 2g.1 Commit Rust changes in logical chunks: (i) walker.rs visited-stack threading, (ii) e2e_cycle_aborts re-enable, (iii) new unit tests, (iv) version bump, (v) CHANGELOG.
- [ ] 2g.2 Push `feat/v1.2.2` to origin.
- [ ] 2g.3 Open PR `feat/v1.2.2 → main`. Title: `v1.2.2: sync_meta cycle detection (PATCH)`.
- [ ] 2g.4 PR description checklist explicitly lists: `cargo fmt --check`, `cargo doc -D warnings`, axiom-policy CI step (counts unchanged), Lean theorem proved with zero `sorry`/`admit`, e2e_cycle_aborts re-enabled, 3 new unit tests added.
- [ ] 2g.5 Wait for CI green on all required gates.

---

## Stage 3 — ship

- [ ] 3.1 Squash-merge PR to `main`.
- [ ] 3.2 Tag `v1.2.2` (annotated, on the squash commit). Push tag.
- [ ] 3.3 Wait for `release.yml` (cargo-dist) to publish the GitHub Release.
- [ ] 3.4 Publish 4 crates topologically: `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`. Wait for index propagation between each.
- [ ] 3.5 Verify `crates.io` `max_version: 1.2.2` for all 4.
- [ ] 3.6 `cargo install grex-cli --force --version 1.2.2`; verify the published binary handles a cyclic fixture without looping.
- [ ] 3.7 Update `progress.md` with v1.2.2 SHIPPED endpoint + refresh "Where we are" block.
- [ ] 3.8 Append v1.2.2 to SSOT `.omne/cfg/history.md` (separate SSOT commit per Rule 7).
