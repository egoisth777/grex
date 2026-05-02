# feat-v1.2.3 — tasks

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`design.md`](./design.md)
**SSOT**: `.omne/cfg/walker.md` §"Cycle detection" · `proof/Grex/Walker.lean`
**Branch**: `feat/v1.2.3` (cut from `main @ 92ec7fd`, post-v1.2.2 ship)
**Order of operations** (Rule 8 hard gate): Lean theorem extension proved BEFORE any Rust impl change.

(B3 dropped post-draft: verified non-bug — CLI already uses Display via display_cycle_detected at error.rs:162-171.)

---

## Stage 0 — openspec PR (this branch)

- [ ] 0.1 Land openspec triplet at `openspec/changes/feat-v1.2.3-bug-fixes/` (proposal + design + tasks).
- [ ] 0.2 Update `progress.md` with a v1.2.3 endpoint entry referencing this triplet + the three bugs (B1, B2, B4) + three tests (T1-T3).
- [ ] 0.3 Confirm `cargo fmt --check`, `cargo doc -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cd proof && lake build` baseline green at HEAD before any code changes.
- [ ] 0.4 PR body cites the locked decisions: scope = bug fixes only (B1, B2, B4 + T1-T3); Lean = extend existing theorem to cover depth-cap + root-seeded visited; no new axiom; SemVer PATCH.

---

## Stage 1 — Lean4 proof extension (HARD GATE per Rule 8)

This stage MUST complete before any Rust file is modified.

- [ ] 1.1 Open `proof/Grex/Walker.lean`. Add a depth-aware mirror of `sync_meta_inner_model` named `sync_meta_inner_model_capped : Option Nat → List String → ManifestTree → SyncMetaResult`. Cycle check fires unconditionally; depth check fires AFTER cycle check; cap exhaustion returns `SyncMetaResult.ok` (truncate-as-ok semantics — matches B1 fix shape).
- [ ] 1.2 Add an accompanying `acyclic_path_capped` predicate if the cap interacts with the inductive structure. If the cap is purely arithmetic (does not affect freshness), reuse the existing `acyclic_path` predicate.
- [ ] 1.3 Prove `sync_meta_no_cycle_under_depth_cap`: for any `cap : Option Nat`, `t : ManifestTree`, `visited : List String`, `acyclic_path visited t → sync_meta_inner_model_capped cap visited t = SyncMetaResult.ok`. Discharge by structural recursion on `t` reusing the existing `sync_meta_inner_model_ok_of_acyclic` mutual lemma at `Walker.lean:356-388`.
- [ ] 1.4 Add corollary `sync_meta_no_cycle_infinite_clone_with_root` for the B4 root-seeded case: `(root_id : String) (t : ManifestTree) → acyclic_path [root_id] t → sync_meta_inner_model [root_id] t = SyncMetaResult.ok`. One-line application of the mutual lemma.
- [ ] 1.5 Verify the original v1.2.2 theorem `sync_meta_no_cycle_infinite_clone` (`Walker.lean:417-420`) still type-checks. It can either stay verbatim or be re-stated as a corollary of `sync_meta_no_cycle_under_depth_cap` with `cap = none, visited = []`.
- [ ] 1.6 Run `cd proof && lake build`. Exit code 0, zero warnings, zero `sorry`/`admit` matches under the strict pattern (`grep -rE ':= by sorry|:= sorry|:= by admit|:= admit' proof/Grex/` returns no matches).
- [ ] 1.7 Confirm the axiom-policy CI step at `.github/workflows/ci.yml:218-265` still passes locally — counts MUST remain `^axiom\b` count = 9 in `Bridge.lean` and = 4 in `Types.lean`. **No new axiom.** No `^axiom\b` declarations outside those two files.
- [ ] 1.8 Commit + push the Lean-only change as the FIRST commit on `feat/v1.2.3`. Commit message: `proof(walker): cycle detection theorem extended for depth cap + root seed (v1.2.3)`. CI must go green on this commit before Stage 2 starts.

**Gate**: Stage 2 cannot begin until Stage 1 commits CI run is green.

---

## Stage 2 — Rust impl (after Stage 1 green)

### 2a — B1: cycle check before depth-cap check

- [ ] 2a.1 Open `crates/grex-core/src/tree/walker.rs`. Move the depth-cap check from `phase3_recurse` (`:1043-1048`) into `phase3_handle_child` (`:985-1026`), positioned AFTER the existing `pack.yaml` existence check and the cycle check, BEFORE the recursive `sync_meta_inner` dispatch.
- [ ] 2a.2 Add `cap: Option<usize>` parameter to `phase3_handle_child` (sourced from `opts.max_depth`). Update `phase3_recurse` to forward `opts.max_depth` and to drop the early-return at `:1043-1048`.
- [ ] 2a.3 Confirm a child that exceeds the depth cap returns `Phase3ChildOutcome::Skipped` (matches the prior Ok-Truncated semantics — silent, no error in `report.errors`). A child whose identity is in `visited` returns `Phase3ChildOutcome::Failed(TreeError::CycleDetected { .. })` regardless of depth.
- [ ] 2a.4 Add unit test `cycle_under_depth_cap_still_aborts` in `mod tests`: a 3-node cycle A→B→C→A with `opts.max_depth: Some(1)`. Asserts `Err(CycleDetected)` (NOT silent `Ok`).

### 2b — B2: omit trailing `@` in `pack_identity_for_child`

- [ ] 2b.1 Edit `pack_identity_for_child` at `walker.rs:323-326`. Render `format!("url:{}", child.url)` when `child.r#ref` is `None` or `Some("")`; preserve the `format!("url:{}@{}", child.url, r)` form when ref is `Some(r)` with non-empty `r`.
- [ ] 2b.2 Add unit test `identity_omits_trailing_at_when_ref_empty` in `mod tests`: three `ChildRef` values with `r#ref: None`, `r#ref: Some(String::new())`, `r#ref: Some("v1".into())`. Assert the first two render `url:<url>` and the third renders `url:<url>@v1`.
- [ ] 2b.3 Audit the existing v1.2.2 test `two_refs_same_url_not_cycle` to confirm both refs (`v1`, `v2`) are non-empty — it should be unaffected by B2 (test passes unchanged).

### 2d — B4: seed root identity in `visited`

- [ ] 2d.1 Edit the public `sync_meta` entry at `walker.rs:643-655`. Replace the `&[]` initial visited with `let initial = vec![pack_identity_for_root(meta_dir)]; sync_meta_inner(..., &initial)`.
- [ ] 2d.2 Extend the existing v1.2.2 unit test `cycle_self_loop_aborts` (in `mod tests`) to assert `chain.first()` matches `format!("path:{}", root.display())`. The test already asserts the cycle fires; this strengthens it to verify the chain's anchor at root.
- [ ] 2d.3 Confirm no existing test asserts `chain.len() == N` for a fixed N that would break under the new prefix length. If any such test exists, update its expected length by +1.

### 2e — T1-T3 new tests

- [ ] 2e.1 Add `diamond_no_cycle` in `mod tests`. Topology: root → A, root → B, A → C, B → C (all on the same ref). Walker completes `Ok(report)`. Assert `report.errors.is_empty()` and `report.metas_visited == 5` (root + A + C + B + C — each branch visits its own C). Locks the per-child clone-of-visited contract.
- [ ] 2e.2 Add `cycle_four_node_aborts` in `mod tests`. Topology: root → A → B → C → D → A. Assert `Err(TreeError::CycleDetected { chain })` with `chain.len() >= 6`, `chain.first()` is the root `path:` identity, and the recurring `url:A@..` appears at chain[1] and chain.last().
- [ ] 2e.3 Add `cycle_nested_prefix_aborts` in `mod tests`. Topology: root → A → B → C, with B → C → B forming the inner cycle. Assert `Err(TreeError::CycleDetected { chain })` with `chain.len() == 5`, last element is `url:B@<ref>`, matching prior entry at chain index 2.
- [ ] 2e.4 Run `cargo test -p grex-core walker::tests` — all five new tests (2a.4 + 2b.2 + 2e.1-3) pass; existing v1.2.2 tests (`cycle_self_loop_aborts` extended, `cycle_three_node_aborts`, `two_refs_same_url_not_cycle`) still pass.

### 2f — workspace version bump

- [ ] 2f.1 Bump root `Cargo.toml` `version = "1.2.2"` → `"1.2.3"` (line 12).
- [ ] 2f.2 Bump `crates/xtask/Cargo.toml` path-dep pin: `grex-cli = { path = "../grex", version = "1.2.2" }` → `"1.2.3"` (line 21).
- [ ] 2f.3 Bump `crates/xtask/tests/version_test.rs` `EXPECTED_WORKSPACE_VERSION` constant → `"1.2.3"` (line 10).
- [ ] 2f.4 `cargo metadata --format-version 1 --no-deps | jq -r '.packages[].version' | sort -u` returns exactly `1.2.3` for every grex-owned package.

### 2g — regen man pages

- [ ] 2g.1 Run `cargo xtask gen-man`. Confirm the diff is limited to version strings inside the rendered manpages (no flag changes shipped in v1.2.3).
- [ ] 2g.2 Stage the regenerated manpage files alongside the version bump commit.

### 2h — CHANGELOG / history.md note

- [ ] 2h.1 Promote the existing `[1.2.2] - pending` section in `CHANGELOG.md` to its actual release date (verify against `git log --oneline | grep v1.2.2`). Prepend a new `[1.2.3] - 2026-05-XX` section using the migration note from [`design.md`](./design.md) §"Migration note for changelog".
- [ ] 2h.2 Update the `[Unreleased]` and `[1.2.3]` link footnotes at the bottom of CHANGELOG.md.
- [ ] 2h.3 Append the equivalent v1.2.3 entry to SSOT `.omne/cfg/history.md` — note this commits to the SSOT repo separately (Rule 7), NOT to grex's git history.

### 2i — local gates

- [ ] 2i.1 `cargo fmt --all -- --check` clean.
- [ ] 2i.2 `cargo doc --no-deps --workspace` clean (run with `RUSTDOCFLAGS="-D warnings"` env var; matches CI gate).
- [ ] 2i.3 `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [ ] 2i.4 `cargo test --workspace` green (existing + v1.2.3 additions).
- [ ] 2i.5 `cd proof && lake build` still green (Stage 1 work persists).
- [ ] 2i.6 Local axiom-policy verify: `^axiom\b` count = 9 in `Bridge.lean`, = 4 in `Types.lean`, zero elsewhere under `proof/Grex/`. (Reproduces `.github/workflows/ci.yml:218-265`.)
- [ ] 2i.7 `typos` clean (if available locally; CI runs it regardless).

### 2j — commit + PR

- [ ] 2j.1 Commit Rust changes in logical chunks: (i) B1 walker.rs depth/cycle reordering, (ii) B2 identity format, (iii) B4 root-identity seed + extended cycle_self_loop_aborts, (iv) T1-T3 new tests, (v) version bump + manpages, (vi) CHANGELOG.
- [ ] 2j.2 Push `feat/v1.2.3` to origin.
- [ ] 2j.3 Open PR `feat/v1.2.3 → main`. Title: `v1.2.3: bug fixes (B1, B2, B4 + T1-T3) (PATCH)`.
- [ ] 2j.4 PR description checklist explicitly lists: `cargo fmt --check`, `cargo doc -D warnings`, axiom-policy CI step (counts unchanged 9 + 4), Lean theorem extended with zero `sorry`/`admit`, B1/B2/B4 fixes landed, T1-T3 tests added, manpages regenerated.
- [ ] 2j.5 Wait for CI green on all required gates.

---

## Stage 3 — ship

- [ ] 3.1 Squash-merge PR to `main`.
- [ ] 3.2 Tag `v1.2.3` (annotated, on the squash commit). Push tag.
- [ ] 3.3 Wait for `release.yml` (cargo-dist) to publish the GitHub Release.
- [ ] 3.4 Publish 4 crates topologically: `grex-core` → `grex-plugins-builtin` → `grex-mcp` → `grex-cli`. Wait for index propagation between each.
- [ ] 3.5 Verify `crates.io` `max_version: 1.2.3` for all 4.
- [ ] 3.6 `cargo install grex-cli --force --version 1.2.3`; verify a cyclic fixture under a depth cap still surfaces `CycleDetected` (B1 ship-side smoke test).
- [ ] 3.7 Update `progress.md` with v1.2.3 SHIPPED endpoint + refresh "Where we are" block.
- [ ] 3.8 Append v1.2.3 to SSOT `.omne/cfg/history.md` (separate SSOT commit per Rule 7).
