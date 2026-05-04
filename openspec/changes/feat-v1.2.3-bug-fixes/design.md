# feat-v1.2.3 — design

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/walker.md` §"Cycle detection" (canonical algorithm reference) · `proof/Grex/Walker.lean` (Rule 8 obligation)

## Why

(B3 dropped post-draft: verified non-bug — CLI already uses Display via display_cycle_detected at error.rs:162-171.)

v1.2.2 shipped `sync_meta` cycle detection, but the post-merge review pass found three narrow defects in code that had been touched (or visibly adjacent to code that had been touched) by v1.2.0–v1.2.2. Each defect is independently small; together they justify a single PATCH ship instead of six separate point releases. None of them rises to an architecture concern.

The defects fall into two buckets: correctness (B1) and operator-visible diagnostics (B2/B4). The three new tests (T1–T3) cover topology shapes the v1.2.2 unit suite did not exercise.

## B1 — Depth-cap masks cycle at the recurse edge

**Site**: `crates/grex-core/src/tree/walker.rs:1043-1048` (current `phase3_recurse` body).

**Bug**. The current shape of `phase3_recurse` is:

```rust
let next_depth = depth + 1;
if let Some(cap) = opts.max_depth {
    if next_depth > cap {
        return Ok(());                 // <-- early return
    }
}
let outcomes: Vec<Phase3ChildOutcome> = pool.install(|| {
    manifest.children.par_iter()
        .map(|child| phase3_handle_child(meta_dir, child, backend, loader, opts, next_depth, visited))
        .collect()
});
```

The cycle check lives inside `phase3_handle_child` (`walker.rs:998-1012`) — it runs ONLY when `phase3_handle_child` is invoked. The early `return Ok(())` at line 1046 short-circuits the entire parallel pass, so a cyclic manifest with cycle length greater than `opts.max_depth` truncates silently and the operator sees `Ok(report)` instead of `Err(CycleDetected)`. This is a correctness regression: depth-capping is a UX feature (bound recursion for sane operator output); it should not mask a safety-critical detection.

**Fix**. Inside `phase3_handle_child` itself, the cycle check is unconditional and already runs before the recursive `sync_meta_inner` call. The fix is to ensure the depth-cap check does not bypass the cycle check at the recurse edge. Two equivalent shapes:

(a) Move the cycle check OUT of `phase3_handle_child` and INTO `phase3_recurse`'s loop body, before the depth-cap early return. The depth-cap check then comes after the cycle check.

(b) Keep the cycle check inside `phase3_handle_child` but move the depth-cap test there too, so each child's `phase3_handle_child` runs the cycle check first and only then decides whether to recurse vs. truncate.

Option (b) preserves the current call-site shape of `phase3_recurse` and keeps the per-child unit (cycle check + recurse decision) co-located. Pseudocode for (b):

```rust
fn phase3_handle_child(
    /* ...existing args..., */
    depth: usize,
    cap: Option<usize>,
    visited: &[String],
) -> Phase3ChildOutcome {
    let dest = meta_dir.join(child.effective_path());
    if !dest.join(".grex").join("pack.yaml").is_file() {
        return Phase3ChildOutcome::Skipped;
    }
    // Cycle check first — must fire regardless of depth cap.
    let id = pack_identity_for_child(child);
    if visited.iter().any(|v| v == &id) {
        let mut chain = visited.to_vec();
        chain.push(id);
        return Phase3ChildOutcome::Failed(TreeError::CycleDetected { chain });
    }
    // Depth cap check — applies AFTER cycle detection.
    let next_depth = depth + 1;
    if let Some(c) = cap {
        if next_depth > c {
            return Phase3ChildOutcome::Skipped;   // Ok-Truncated
        }
    }
    let mut child_visited = visited.to_vec();
    child_visited.push(id);
    match sync_meta_inner(&dest, backend, loader, opts, &[], next_depth, &child_visited) {
        Ok(sub) => Phase3ChildOutcome::Recursed(sub),
        Err(e)  => Phase3ChildOutcome::Failed(e),
    }
}
```

Implementation note: the existing `phase3_recurse` early-return at `walker.rs:1043-1048` MUST be deleted — that is what currently masks the check. The cap parameter is then forwarded to each per-child call.

**Lean obligation**. B1 requires no Lean model change. Cycle check ordering is enforced by Lean construction (the `if id ∈ visited` clause is the first action in `syncMetaChildren`). Bounded recursion is a prefix of unbounded recursion; theorem holds for any runtime `max_depth`.

## B2 — Empty-ref Display trailing `@`

**Site**: `crates/grex-core/src/tree/walker.rs:323-326`.

**Bug**. Current implementation:

```rust
fn pack_identity_for_child(child: &ChildRef) -> String {
    let rref = child.r#ref.as_deref().unwrap_or("");
    format!("url:{}@{}", child.url, rref)
}
```

When `child.r#ref` is `None` or `Some("")`, the formatted identity is `url:https://example.com/repo.git@` with a dangling trailing `@`. This appears in:

1. `TreeError::CycleDetected { chain }` user-facing output (rendered by `display_cycle_detected` at `error.rs:162-171` as `cycle detected in pack graph: url:foo@ → url:bar@`).
2. Internal HashSet/Vec membership checks for cycle detection.
3. Any logger / audit emitter that captures the chain.

The trailing `@` is purely cosmetic but operator-visible.

**Fix**. Conditional formatting:

```rust
fn pack_identity_for_child(child: &ChildRef) -> String {
    match child.r#ref.as_deref() {
        Some(r) if !r.is_empty() => format!("url:{}@{}", child.url, r),
        _                        => format!("url:{}", child.url),
    }
}
```

**Membership equivalence argument**. Identity comparison is string-equality only (`visited.iter().any(|v| v == &id)` at `walker.rs:1008`). Since both sides of every comparison go through the same `pack_identity_for_child` helper, the format change is a uniform substitution — every occurrence of `url:<u>@` becomes `url:<u>` simultaneously, equality semantics are preserved.

The positive case `two_refs_same_url_not_cycle` (v1.2.2 test) — `A@v1` vs `A@v2` — is unaffected: both have non-empty refs, both render with the `@<ref>` suffix, both remain distinct.

**Edge case to verify**. A ChildRef with `r#ref: Some("")` (explicit empty string) and a ChildRef with `r#ref: None` now both produce `url:<url>` — they collide as a single identity. This is the correct behaviour: an empty ref string is semantically equivalent to "default branch" (the same thing `None` means), and treating them as identical prevents a manifest-author-controlled false negative where the same logical pack is declared twice with `ref: ""` and no ref.

## B4 — Root identity missing from chain

**Site**: `crates/grex-core/src/tree/walker.rs:643-655` (the public `sync_meta` entry).

**Bug**. Current impl:

```rust
pub fn sync_meta(...) -> Result<SyncMetaReport, TreeError> {
    sync_meta_inner(meta_dir, backend, loader, opts, prune_candidates, /* depth */ 0, &[])
                                                                                          ^^
}
```

Initial `visited` is `&[]`. The cycle chain therefore never includes the root pack identity — operators see chains like `url:A → url:B → url:A` without the `path:<root>` prefix, so the chain does not anchor to the cwd-meta location where the cycle was found.

The v1.2.2 design doc (`design.md` §"Algorithm") shows the intent was `let visited = vec![pack_identity_for_root(meta_dir)];` — that initial seed was elided in the impl.

**Fix**. Seed `visited` at the public entry:

```rust
pub fn sync_meta(...) -> Result<SyncMetaReport, TreeError> {
    let initial = vec![pack_identity_for_root(meta_dir)];
    sync_meta_inner(meta_dir, backend, loader, opts, prune_candidates, 0, &initial)
}
```

**Lean implication**. B4 is discharged by instantiating the generalized theorem with `visited = [pack_identity_for_root(root)]`. No new theorem needed.

## T1–T3 test fixtures

All three live in `walker::tests` (the existing `#[cfg(test)] mod tests` block at the bottom of `walker.rs`).

**T1 — Diamond, no cycle**.

Topology: root has children A and B; A has child C; B has child C.

```
root
├── A (ref v1)
│   └── C (ref v1)
└── B (ref v1)
    └── C (ref v1)
```

Both branches produce identity `url:<C-url>@v1`, but they live in disjoint visited sets (per-child clone discipline from v1.2.2). Walker completes `Ok(report)` with no `CycleDetected` raised, no error in `report.errors`, and `metas_visited == 5` (root + A + C-via-A + B + C-via-B).

This locks the per-child clone-of-visited contract — a regression where siblings share visited would surface here as a false positive.

**T2 — 4-node cycle**.

Topology: root → A → B → C → D → A.

```
root → A → B → C → D → A   (cycle on A re-entry from D)
```

Detection fires at the recurse edge from D into A, where `id = pack_identity_for_child(A)` is already in `visited` (which by then is `[path:root, url:A, url:B, url:C, url:D]`). The error chain has length 6: the inherited visited of length 5 plus the recurring `url:A`.

Test assertion: `Err(TreeError::CycleDetected { chain })` with `chain.len() >= 6` and `chain.first() == Some(&format!("path:{}", root.display()))` and `chain.last() == chain[1]` (root + A + B + C + D + A; first non-root entry equals last entry).

**T3 — Nested-prefix cycle**.

Topology: root → A → B → C, with B's manifest also referencing C, and C's manifest referencing B.

```
root → A → B → C → B   (inner cycle B-C-B; outer arm root→A acyclic)
```

Detection fires at the recurse edge from C into B, where `id = pack_identity_for_child(B)` is in `visited == [path:root, url:A, url:B, url:C]`. Chain has length 5: root + A + B + C + B.

Test assertion: `Err(TreeError::CycleDetected { chain })` with `chain.len() == 5`, last element is `url:B@<ref>`, and the matching entry earlier in the chain is at index 2 (B's first appearance).

This locks detection at non-top-level cycles. The v1.2.2 self-loop test fires at depth 1; the 3-node cycle fires at depth 3 from the root. T3 explicitly covers the case where the prefix path is acyclic and the cycle is buried deeper.

## Lean spec — extension

No Lean model changes required. The existing `sync_meta_inner_model` and `acyclic_path` definitions cover both B1 and B4:

- **B1**: depth cap is runtime-only; the unbounded-recursion theorem still holds for any bounded prefix of execution. Cycle-check-before-depth-check ordering is enforced by Lean construction (the existing `if id ∈ visited` clause is the first action in `syncMetaChildren`).
- **B4**: discharged by instantiating the existing v1.2.2 theorem with `visited = [pack_identity_for_root(root)]`. The theorem already quantifies over arbitrary `visited : List String`.

Bridge axiom counts unchanged: 9 in `Bridge.lean`, 4 in `Types.lean`. CI gate at `.github/workflows/ci.yml:218-265` stays green by construction.

## Files touched

**Rust (production):**

- `crates/grex-core/src/tree/walker.rs` — B1 (move depth-cap check inside `phase3_handle_child`, after the cycle check), B2 (`pack_identity_for_child` empty-ref omit), B4 (`sync_meta` seeds `visited` with `pack_identity_for_root`). Add the three T1–T3 unit tests + B1 unit test (`cycle_under_depth_cap_still_aborts`) + B2 unit test (`identity_omits_trailing_at_when_ref_empty`) inside `mod tests`. Extend the existing `cycle_self_loop_aborts` test to assert chain[0] is the root `path:` identity (B4 coverage).

**Lean:**

- `proof/Grex/Walker.lean` — B2 fix only:
  - `ChildRef.identity` def updated to match new Rust format (omit trailing `@` on empty ref).
  - Added caller-obligation doc paragraph on `sync_meta_no_cycle_infinite_clone` theorem (visited ∩ descendants(t) = ∅; satisfied by Rust via disjoint `path:` vs `url:` prefixes).
  - Theorem statement + proof body unchanged (proof reasons over membership, format change transparent).
  - lake build green; axiom counts unchanged (Bridge=9, Types=4, Other=0); kernel deps `[propext]` only.
- B1: no Lean change (depth cap is runtime-only; theorem holds for any `max_depth` since bounded recursion is prefix of unbounded).
- B4: no new theorem (instantiate generalized theorem with `visited = [pack_identity_for_root(root)]`).

**Versioning:**

- `Cargo.toml` (workspace root): `version = "1.2.2"` → `"1.2.3"`.
- `crates/xtask/Cargo.toml`: `grex-cli = { path = "../grex", version = "1.2.2" }` → `"1.2.3"`.
- `crates/xtask/tests/version_test.rs`: `EXPECTED_WORKSPACE_VERSION` constant → `"1.2.3"`.

**Manpages:**

- `cargo xtask gen-man` to regenerate. No new flags in v1.2.3, so the diff should be limited to version-string updates inside the manpages.

**Changelog/history:**

- `CHANGELOG.md` — promote `[1.2.2] - pending` to its release date and append a new `[1.2.3] - 2026-05-XX` section. Update the `[Unreleased]` and tag-link footnotes.
- SSOT `.omne/history.md` — append v1.2.3 entry (separate repo per Rule 7, ships through SSOT).

## Acceptance criteria

1. `cd proof && lake build` exits 0; zero `sorry`, zero `admit`. New `sync_meta_inner_model_capped` def + `sync_meta_no_cycle_under_depth_cap` theorem present. Original v1.2.2 theorem unchanged or factored as a corollary.
2. Axiom-policy CI step at `.github/workflows/ci.yml:218-265` stays green: `^axiom\b` count = 9 in `Bridge.lean`, = 4 in `Types.lean`, zero elsewhere.
3. New unit tests pass under `cargo test -p grex-core walker::tests`:
   - `cycle_under_depth_cap_still_aborts` (B1)
   - `identity_omits_trailing_at_when_ref_empty` (B2)
   - `diamond_no_cycle` (T1)
   - `cycle_four_node_aborts` (T2)
   - `cycle_nested_prefix_aborts` (T3)
4. Existing v1.2.2 tests still pass: `cycle_self_loop_aborts` (extended with B4 chain[0] assertion), `cycle_three_node_aborts`, `two_refs_same_url_not_cycle`, `e2e_cycle_aborts`.
5. `cargo fmt --all -- --check` clean.
6. `cargo doc --no-deps --workspace` clean under `RUSTDOCFLAGS=-D warnings`.
7. `cargo clippy --workspace --all-targets -- -D warnings` clean.
8. `cargo test --workspace` green.
9. Workspace version 1.2.3 across root + xtask + version_test.
10. Man pages regenerated; diff is version strings only.

## Migration note for changelog

```
## [1.2.3] — 2026-05-XX

### Fixed

- Walker Phase 3 depth cap (`opts.max_depth`) no longer masks cycle
  detection. A cyclic manifest with cycle length greater than the
  configured cap now surfaces `TreeError::CycleDetected` instead of
  truncating silently as `Ok`. Cycle check fires at the recurse edge
  before the depth-cap check; depth-cap remains an Ok-Truncated
  outcome for acyclic input only.
- `pack_identity_for_child` no longer emits a trailing `@` when the
  child's `ref:` is unset or empty. Identities now render as
  `url:<url>` instead of `url:<url>@`. Cycle-chain output and
  diagnostic logs are correspondingly cleaner.
- `sync_meta` cycle chain now includes the root pack identity as its
  first element. Operators inspecting `TreeError::CycleDetected.chain`
  for a root-level cycle now see `[path:<root>, url:..., url:...]`
  instead of `[url:..., url:...]` — the cycle is anchored to the
  cwd-meta location where it was detected.

### Notes

- No API change. `TreeError::CycleDetected` shape unchanged. The
  `chain` field's content is now strictly more informative; existing
  consumers that string-match the chain may see additional prefix
  elements (root identity) and slightly shorter element strings (no
  trailing `@`). No fixed-length guarantee was ever documented for
  the chain.
- New regression coverage: diamond-no-cycle (T1), 4-node cycle (T2),
  nested-prefix cycle (T3).
```
