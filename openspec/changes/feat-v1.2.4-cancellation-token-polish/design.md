---
slug: feat-v1.2.4-design
type: design
status: active
last_updated: 2026-05-02
---

# feat-v1.2.4 — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/walker.md` §"Cancellation token" (canonical algorithm reference) · `proof/Grex/Walker.lean` (Rule 8 obligation)

## Why

v1.2.2 shipped the `sync_meta` cycle detector and the rayon-based Phase 3 parallel pass. v1.2.3 closed three narrow defects in that detector (depth-cap masking, empty-ref formatting, root-identity seeding). The post-merge reviews from those two cycles converged on one missing piece: rayon's `par_iter().map(...).collect()` has no built-in cancellation contract — once a cycle is detected on one branch, every other in-flight sibling continues to completion (potentially cloning, fetching, walking deeper into its own subtree). The reviewer findings that drove this scope:

- **v1.2.2 R#4 HIGH** — `par_iter` no-cancel: a cyclic manifest produces `Err(CycleDetected)` only after every sibling clone has finished, leaving partial-clone artifacts on disk and amplifying multi-cycle drop time.
- **v1.2.2 R#2 MED** — `par_iter().collect()` semantics: even when one task fails, rayon runs every other task to completion before the `collect` returns. The "first error wins" propagation is a UI illusion — work is not halted.
- **v1.2.3 R#3 MAJOR** — proptest coverage gap: the v1.2.3 unit tests (T1–T3) lock specific topologies but do not exhaust the input space. A property-based generator over random DAGs ± injected back-edges is the missing safety net.
- **v1.2.3 R#4 LOW** — axiom drift detection: `Bridge.lean` axiom counts (9 → 10 in v1.2.4) are checked numerically by CI, but `#print axioms` of the headline theorems is not enforced. A theorem could silently grow a new axiom dep without tripping the count gate.

Each item is independently small. Together they justify a single PATCH ship: A1 closes the headline correctness gap; six polish items clean dead code and rename legacy walker symbols; three new tests (1 unit + 1 spot-check extension + 1 proptest) raise coverage; one CI gate locks axiom dependency stability.

## Architectural context

Walker Phase 3 (`phase3_recurse` in `crates/grex-core/src/tree/walker.rs:996+`, post-v1.2.3) recurses into each child manifest in parallel using a rayon thread pool. The v1.2.3 fix put the cycle check at the recurse edge inside `phase3_handle_child` — when `pack_identity_for_child(child)` is already present in the inherited `visited` slice, the per-child closure returns `Phase3ChildOutcome::Failed(TreeError::CycleDetected { chain })`. The collected outcomes are aggregated in `phase3_recurse`; the first `Failed` is converted to `Err` per the v1.2.2 F2 fail-loud policy.

The rayon contract that v1.2.4 has to work around: `par_iter().map(f).collect()` and `par_iter().try_for_each(f)` both propagate the first error to the caller, but neither cancels in-flight tasks. Each task must voluntarily check shared state to self-cancel. v1.2.4 introduces an `Arc<AtomicBool>` cancellation flag, constructed at the entry of each `phase3_recurse` call and passed through to every per-child closure. Each closure checks the flag at entry (the EARLY-OUT) and sets the flag on cycle detection (the SIGNAL).

The flag is per-`phase3_recurse`-call (not global) so that disjoint subtrees do not cross-cancel. A cycle in one subtree of the manifest tree must not abort cloning in a sibling subtree that happens to be processed by the same thread pool — the cancellation domain is exactly the set of siblings being co-iterated.

## A1 — Cancellation token algorithm

Pseudocode:

```rust
fn phase3_recurse(
    pool: &ThreadPool,
    ancestors: &[String],          // renamed from `visited` per P1
    children: &[ChildRef],
    /* ...other args... */
) -> Result<SyncMetaReport, TreeError> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let outcomes: Vec<Phase3ChildOutcome> = pool.install(|| {
        children
            .par_iter()
            .map(|child| {
                // EARLY-OUT: another sibling already detected a cycle.
                if cancelled.load(Ordering::Relaxed) {
                    return Phase3ChildOutcome::Cancelled;
                }
                phase3_handle_child(child, ancestors, &cancelled, /* ... */)
            })
            .collect()
    });
    // Aggregate cycles into report.errors; return first cycle as Err
    // per v1.2.2 F2 fail-loud policy (preserved unchanged).
    /* ...existing aggregation... */
}

fn phase3_handle_child(
    child: &ChildRef,
    ancestors: &[String],
    cancelled: &AtomicBool,        // NEW
    /* ...other args... */
) -> Phase3ChildOutcome {
    let dest = meta_dir.join(child.effective_path());
    if !dest.join(".grex").join("pack.yaml").is_file() {
        return Phase3ChildOutcome::Skipped;
    }
    let id = pack_identity_for_child(child);
    if ancestors.iter().any(|a| a == &id) {
        cancelled.store(true, Ordering::Relaxed); // NEW: signal siblings
        let mut chain = ancestors.to_vec();
        chain.push(id);
        return Phase3ChildOutcome::Failed(TreeError::CycleDetected { chain });
    }
    // ... depth-cap check (v1.2.3 B1), then recurse ...
}
```

### Edge cases

- **Best-effort semantics**. Rayon work-stealing means a sibling already mid-`phase3_handle_child` may complete its current step (one clone, one manifest read) before re-checking the flag. That is acceptable: Phase 3 is bounded by manifest size, and the flag is checked at every closure entry. Worst case is one extra child completes; in practice the amplification factor drops from "every sibling" to "at most one sibling per worker".
- **Cancelled outcome aggregation**. A `Phase3ChildOutcome::Cancelled` outcome carries no sub-report. The aggregation step skips merging for cancelled outcomes — they do not contribute to `report.metas_visited` or `report.errors`. The cycle that triggered cancellation is the only error reported.
- **Atomic ordering**. `Ordering::Relaxed` is sufficient for both load and store: we need eventual visibility, not strict happens-before ordering against any other memory operation. Each closure is independent; the flag is the only shared mutable state.
- **Per-call scoping**. The `Arc<AtomicBool>` is constructed inside `phase3_recurse` and dropped when the call returns. Recursive sub-calls into deeper `phase3_recurse` invocations (one per non-cycling child) get their own flag — a cycle two levels deep does not cancel siblings at level one.

### What v1.2.4 does NOT do (defer to v1.2.5)

- **A2 partial-clone cleanup**. When a sibling is cancelled mid-clone, any bytes it already wrote to disk stay on disk. v1.2.5 will route these through the existing `<meta>/.grex/trash/` quarantine that v1.2.1 introduced for its doctor-scan flow. v1.2.4 ships the cancellation signal only; on-disk hygiene is a follow-up.

## Polish (6 items)

| #  | Item                                                                                                                       | File                                          | LOC |
|----|----------------------------------------------------------------------------------------------------------------------------|-----------------------------------------------|-----|
| P1 | Rename `visited` → `ancestors` (parameter name + all call sites; reflects "stack of in-progress ancestors", not "set of seen nodes") | walker.rs + graph_build.rs                    | ~50 |
| P2 | Doc-comment cleanup on `sync_meta` (strip impl-detail leakage like "Phase 3", "A.1", "rayon" from public-facing rustdoc)   | walker.rs                                     | ~10 |
| P3 | Delete unused `PackLock::acquire` synchronous variant (m7 carry-forward; only async variant is reachable)                  | scheduler / pack-lock module                  | ~30 |
| P4 | Delete unused `Scheduler::permits()` accessor (m7 carry-forward; no caller after rayon migration)                          | scheduler                                     | ~10 |
| P5 | Inline `DEFAULT_MANAGED_GITIGNORE_PATTERNS` (single use site; the named const adds indirection without reuse)              | wherever defined (likely manifest.rs)         | ~20 |
| P6 | Rename `OwnCycleGuard` → `VisitedInsertGuard` (legacy walker name predates v1.2.2 cycle-detection redesign; new name reflects RAII insert/remove of the visited entry, not a cycle-prevention guard) | walker.rs (or wherever guard type lives)      | ~15 |

P3, P4, P5 are dead-code removal carried forward from the m7 scope cleanup. P1 and P6 are renames — `visited` and `OwnCycleGuard` were both inherited from earlier walker iterations and no longer match the current semantics. P2 is documentation hygiene: the public `sync_meta` rustdoc currently leaks the internal phase taxonomy and the rayon implementation choice; neither is part of the API contract.

None of the six items is behaviour-changing. P3–P5 reduce the public surface; P1 and P6 are local renames; P2 rewrites prose. All six land in the same PR as A1.

## Tests (3 new)

All three live in `walker::tests` (the existing `#[cfg(test)] mod tests` block at the bottom of `walker.rs`), with the proptest case in `crates/grex-core/tests/property.rs` (new file or existing if proptest harness is already wired).

**T-cancel — sibling cancellation under cycle**.

Topology: 4-node cycle root → A → B → C → D → A, with root having additional non-cyclic siblings X, Y, Z that each contain expensive-to-walk subtrees (deep manifest, many children).

```
root
├── A → B → C → D → A           (cycle arm)
├── X → X1, X2, X3, ..., Xn     (acyclic, expensive)
├── Y → Y1, Y2, ..., Yn         (acyclic, expensive)
└── Z → Z1, Z2, ..., Zn         (acyclic, expensive)
```

Use a shared `AtomicUsize` counter incremented in a test-only side hook at the entry of each `phase3_handle_child` for X/Y/Z's children. Assert: after the walker returns `Err(CycleDetected)`, the counter is strictly less than the total descendant count of X+Y+Z. Equivalently, assert that at least one of X/Y/Z's per-child closures returned `Phase3ChildOutcome::Cancelled` (requires test-visible outcome inspection or counter instrumentation).

The test must be deterministic across reruns. Achieve determinism by sizing X/Y/Z's subtrees large enough that the cycle on the A-arm is guaranteed to fire before sibling subtrees finish, even on a single-threaded rayon pool. The cycle-detection latency is O(cycle-length) which is small; sibling subtree walk is O(tree-size) which is tunable.

**T1-spot-check — diamond extension**.

The existing v1.2.3 `cycle_diamond_shared_descendant_no_cycle` test asserts `metas_visited == 5` (root + A + C-via-A + B + C-via-B). Extend it to assert that C is genuinely walked through both arms — not just counted twice. Add a side-effect-visible per-meta-walk hook (e.g. a `Vec<PathBuf>` of `dest` paths visited, captured by the test backend) and assert that two distinct `dest` paths corresponding to C-via-A and C-via-B both appear.

This catches a regression where a future optimization (e.g. memoization of pack identities) might collapse the second walk into a no-op while still incrementing the counter.

**T-proptest — generator dichotomy**.

Use a `proptest` generator producing two flavours of input:

1. Random DAG: build a topologically-sorted random graph of N nodes (N ∈ [2, 16]) with edge density p ∈ [0.1, 0.5]. By construction acyclic.
2. Random graph with injected back-edge: same generator, then add a single edge from a deeper node to a strictly shallower ancestor. By construction cyclic.

For each generated input, materialize the manifest tree on a tmpdir (or use the existing in-memory test backend), run `sync_meta`, and assert the dichotomy:

- Every flavour-1 input → `Ok(report)` with `report.errors.is_empty()`.
- Every flavour-2 input → `Err(TreeError::CycleDetected { chain })` with `chain.len() ≥ 2`.

Configure `proptest` with a seed-stable config (`ProptestConfig { failure_persistence: Some(FileFailurePersistence::WithSource("regressions")), ..Default::default() }`) so failing seeds are committed and replayable. Cap test count to keep CI runtime bounded (e.g. `cases: 64`).

## CI gate (`#print axioms` smoke check)

Add a step to `.github/workflows/ci.yml` (the Lean4 proof-gate job confirmed at lines 218–265 per the v1.2.3 review). The step runs after the existing axiom-count check:

```bash
echo 'import Grex.Walker' > /tmp/check.lean
echo '#print axioms Grex.Walker.sync_meta_no_cycle_infinite_clone' >> /tmp/check.lean
echo '#print axioms Grex.Walker.cancellation_terminates_promptly' >> /tmp/check.lean
output=$(cd proof && lake env lean /tmp/check.lean)
if ! echo "$output" | grep -Eq "depends on axioms: \[propext(, cancellation_token_correctly_propagates)?\]"; then
  echo "AXIOM DRIFT detected: theorem deps changed from [propext] (or [propext, cancellation_token_correctly_propagates])" >&2
  echo "$output" >&2
  exit 1
fi
```

The grep target accepts the literal string `depends on axioms: [propext]` — if a future code change adds a non-`propext` axiom dependency to either headline theorem, the gate fails loudly and the PR is blocked. The check is a smoke-level guard; the existing numeric `^axiom\b` count check in `Bridge.lean` and `Types.lean` remains the per-file ceiling. Together they catch both "new axiom in a wrap module" and "headline theorem grew a new dep silently".

If `cancellation_terminates_promptly` legitimately requires a new bridge axiom (atomic visibility — see Lean section below), the CI gate string is updated in the same PR to match the new axiom name.

## Lean spec extension

New theorem `Grex.Walker.cancellation_terminates_promptly` in `proof/Grex/Walker.lean`:

- **Statement**. For any cyclic manifest tree `t` and any cancellation token state, the walker terminates in finite steps and does NOT recurse into descendants of cancelled siblings.
- **Precondition**. `cancellation_token_correctly_propagates` — bridge axiom asserting that the Rust runtime's `Arc<AtomicBool>` provides eventual visibility across rayon worker threads. This is the bridge between the Lean model's pure-functional cancellation parameter and the runtime's mutable shared atomic. Likely the only new axiom in this release (Bridge.lean: 9 → 10).
- **Postcondition**. Termination bound is `min(cycle-detection-depth, tree-depth)` — strictly tighter than the v1.2.3 `sync_meta_no_cycle_infinite_clone` theorem, which proved termination but not promptness (v1.2.3 allowed "terminates after walking every sibling").

**Approach (recommended: A1 — extend existing model)**.

The Lean model's `sync_meta_inner_model` already takes `visited : List String` as a parameter. Add a `cancelled : Bool` parameter; when `cancelled = true` the model returns immediately (matching the Rust EARLY-OUT). The recursive call passes a cancellation flag computed from "any prior sibling in the children list detected a cycle". Theorem: existence of a `cancelled := true` flip during recursion strictly shrinks the termination bound from `O(tree-size)` to `O(cycle-detection-depth)`.

Proof sketch (in Lean):
1. Show `sync_meta_inner_model` with `cancelled = true` returns in zero recursive steps.
2. Show that for a cyclic tree, the "first sibling that detects a cycle" runs in `O(cycle-detection-depth)` steps.
3. Compose: total steps = (cycle-detection arm) + (each subsequent sibling: 1 step to check flag and return) = `O(cycle-detection-depth) + O(siblings)`.
4. Compare to v1.2.3 bound `O(tree-size)` — strictly tighter when siblings have nontrivial subtrees.

The proof reuses `acyclic_path` and `sync_meta_no_cycle_infinite_clone` as lemmas. The new axiom (if needed) lives in `Bridge.lean` and is enumerated in the SSOT axiom budget. If atomic visibility can be modeled without a new axiom (e.g. by treating the flag as a pure functional parameter threaded through recursion), Bridge.lean stays at 9.

## Files touched

**Rust (production):**

- `crates/grex-core/src/tree/walker.rs` — A1 (cancellation token construction + per-child early-out + signal on cycle), P1 (`visited` → `ancestors` parameter rename), P2 (rustdoc cleanup on `sync_meta`), P6 (`OwnCycleGuard` → `VisitedInsertGuard`), T-cancel test, T1-spot-check extension to existing diamond test.
- `crates/grex-core/src/tree/graph_build.rs` — P1 mirror rename in any helper that takes the same parameter.
- `crates/grex-core/src/scheduler.rs` (or wherever `PackLock` and `Scheduler` live) — P3 (delete `PackLock::acquire` sync variant), P4 (delete `Scheduler::permits()` accessor).
- `crates/grex-core/src/manifest.rs` (or wherever `DEFAULT_MANAGED_GITIGNORE_PATTERNS` is defined) — P5 (inline the constant at its single use site).

**Rust (tests):**

- `crates/grex-core/tests/property.rs` — T-proptest generator + dichotomy assertion. New file unless proptest harness already exists.

**Lean:**

- `proof/Grex/Walker.lean` — extend `sync_meta_inner_model` with `cancelled : Bool` parameter; new theorem `cancellation_terminates_promptly`. Existing `sync_meta_no_cycle_infinite_clone` unchanged or instantiated with `cancelled = false` as a corollary.
- `proof/Grex/Bridge.lean` — possible 1 new axiom `cancellation_token_correctly_propagates` (Bridge axiom count 9 → 10). If the cancellation flag can be modelled as a pure parameter without a runtime bridge, no change.

**CI:**

- `.github/workflows/ci.yml` — append the `#print axioms` smoke check after the existing axiom-count step (around line 265).

**Versioning:**

- Cargo.toml workspace + 3 internal path-deps (grex-core, grex-mcp, grex-plugins-builtin) + crates/xtask/Cargo.toml grex-cli path-dep + crates/xtask/tests/version_test.rs EXPECTED_WORKSPACE_VERSION
- `Cargo.toml` (workspace root): `version = "1.2.3"` → `"1.2.4"`.
- `crates/xtask/Cargo.toml`: `grex-cli = { path = "../grex", version = "1.2.3" }` → `"1.2.4"`.
- `crates/xtask/tests/version_test.rs`: `EXPECTED_WORKSPACE_VERSION` constant → `"1.2.4"`.

**Manpages:**

- `cargo xtask gen-man` to regenerate `man/grex.1`. No new flags in v1.2.4, so the diff should be limited to version-string updates.

**Changelog/history:**

- `CHANGELOG.md` — append a new `[1.2.4] - 2026-05-XX` section. Update the `[Unreleased]` and tag-link footnotes.
- `.omne/history.md` — append v1.2.4 entry (separate repo per Rule 7, ships through SSOT).

## Acceptance criteria

1. `cd proof && lake build` exits 0; zero `sorry`, zero `admit`. Axiom counts: Bridge ≤ 10, Types = 4, Other = 0.
2. `#print axioms Grex.Walker.cancellation_terminates_promptly` shows `[propext]` only (or `[propext, cancellation_token_correctly_propagates]` if the bridge axiom is introduced — CI gate string updated to match).
3. `cargo build --workspace`, `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace` (with `RUSTDOCFLAGS=-D warnings`), and `cargo clippy --workspace --all-targets -- -D warnings` all exit 0. Note: `dispatch_parallel.rs` integration test is excluded from `cargo test --workspace` per pre-existing Windows UAC os error 740 (M6 #24, since v1.2.0). Not a v1.2.4 regression.
4. The three new tests pass: `cancellation_aborts_siblings` (T-cancel), the extended `cycle_diamond_shared_descendant_no_cycle` (T1 spot-check), and the proptest dichotomy case (T-proptest). The existing 380+ lib tests still pass; the v1.2.3 `e2e_cycle_aborts` integration test still passes.
5. T-cancel is deterministic across reruns: no flake on 100 consecutive `cargo test cancellation_aborts_siblings` invocations.
6. CI smoke check at `.github/workflows/ci.yml` enforces axiom dependency stability for both headline theorems on every PR.
7. SemVer label: PATCH (1.2.3 → 1.2.4). Per Rule 6 the maintainer has the call; the technical reasoning supports PATCH because the cancellation flag is internal to `phase3_recurse` (no public API change), the polish renames are all on private symbols, and the dead-code removal targets unreachable items only.
8. v1.3.0 readiness: existing end-to-end test `grex sync` on meta-pack + 1 sub-pack (acyclic) MUST pass. Asserts: returns `Ok`, `report.errors.is_empty()`, `metas_visited >= 2`, no `CycleDetected` variant in any error path. Add this as `e2e_v1_3_0_readiness_smoke` in `crates/grex/tests/sync_e2e.rs` if not already covered by an existing e2e test.

## Migration note for changelog

```
## [1.2.4] — 2026-05-XX

### Changed

- Walker Phase 3 now propagates cycle detection across sibling tasks
  via an internal cancellation token. When one sibling detects a cycle,
  other in-flight siblings observe the signal at their next closure
  entry and return without further recursion. Cyclic-manifest behaviour
  changes from "every sibling completes before Err is returned" to
  "remaining siblings short-circuit". Acyclic-manifest behaviour is
  unchanged.

### Internal

- Renamed walker parameter `visited` → `ancestors` to reflect its
  stack-of-in-progress semantics.
- Renamed `OwnCycleGuard` → `VisitedInsertGuard` to reflect its RAII
  insert/remove role.
- Removed unused `PackLock::acquire` synchronous variant and
  `Scheduler::permits()` accessor (m7 dead-code carry-forward).
- Inlined `DEFAULT_MANAGED_GITIGNORE_PATTERNS` at its single use site.
- Cleaned up implementation-detail leakage from `sync_meta` rustdoc.

### Tests

- New unit test `cancellation_aborts_siblings` (4-node cycle with
  expensive sibling subtrees; asserts cancellation propagation).
- Extended `cycle_diamond_shared_descendant_no_cycle` to assert C is
  walked via both arms (not just counted twice).
- New proptest `random_dag_dichotomy` generating random DAGs and
  random graphs with injected back-edges; asserts the acyclic →
  Ok / cyclic → Err dichotomy.

### Notes

- No public API change. `TreeError::CycleDetected` shape unchanged.
  The cancellation flag is an internal Phase 3 implementation detail.
- Partial-clone artifacts left behind by cancelled siblings are NOT
  cleaned up in v1.2.4 — that ships in v1.2.5 via the existing
  `<meta>/.grex/trash/` quarantine flow.
```
