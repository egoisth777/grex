# feat-v1.2.2 — design

**Status**: draft
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/cfg/walker.md` §"Cycle detection" (canonical algorithm reference) · `proof/Grex/Walker.lean` (proof obligation)

## Why

v1.2.1 split the sync pipeline into two passes:

- **mutating pass** — `tree::walker::sync_meta` (Phases 1, 2, 3): clone, fetch, prune, recurse.
- **read pass** — `tree::graph_build::build_graph`: walk the on-disk tree, build `PackGraph`, record cycles.

Cycle detection landed cleanly in the read pass — `graph_build::handle_child` runs a `pack_identity_for_child` stack check at `crates/grex-core/src/tree/graph_build.rs:174-179` and returns `TreeError::CycleDetected { chain }`. The legacy v1.1.x `Walker::walk` carried that same check inline at `walker.rs:180-185` (still visible because the legacy `Walker` impl coexists with `sync_meta` — kept for tests and the read-side caller). When the mutating responsibilities were carved out into `sync_meta`, the cycle check was not threaded through.

A cyclic manifest therefore reaches `build_graph`'s detection only after `sync_meta` has already cloned the same repo into the same logical position once per recursion frame — and Phase 3 recursion has no termination criterion of its own, so the loop is unbounded. The `e2e_cycle_aborts` test at `crates/grex/tests/sync_e2e.rs:260` was disabled with a `#[ignore]` annotation explicitly noting "would clone forever" rather than removed, because it documents the gap.

This design closes the gap by re-introducing the legacy mechanism into `sync_meta_inner`.

## Locked design decisions

Two open questions from the proposal review are now resolved by the maintainer. Both are declarative — no further options are under consideration.

- **Q6 — Cycle detection point: PHASE 3 RECURSE EDGE.** The check fires inside `phase3_recurse`, just before the recursive `sync_meta_inner(child, ...)` call — not at Phase 1 dispatch. Phase 1 only clones siblings, and siblings are not ancestors; the recursion edge is the only point where an ancestor identity could reappear as a descendant. Legacy v1.1.x `Walker::walk` fused clone+recurse, so "before clone" and "before recurse" were the same instant; v1.2.1's split made the distinction observable, and Phase 3 is the correct half of the split.
- **Q7 — Visited state propagation: A.1 (CLONE-PER-CHILD).** Each rayon child branch receives an owned `HashSet<String>` clone of the parent's visited set with the child's identity inserted. Six senior Rust reviewers (10+ years experience, six independent lenses: performance, concurrency safety, correctness, idiomatic Rust, testability + Lean bridge, maintainability) UNANIMOUSLY chose A.1 over A.2 (sequentialise Phase 3 when visited is non-empty) and A.3 (`Arc<Mutex<HashSet>>` shared across siblings). The CORRECTNESS reviewer's load-bearing finding: A.3 is not merely slow — it is **incorrect**. A single shared `HashSet` conflates "ancestors of node X" with "the union of all live branches", which breaks the path-from-root invariant under parallel siblings. Stack discipline (push-on-enter / pop-on-exit) demands per-branch state; a Mutex-shared HashSet produces both false positives (a sibling identity flagged as ancestor) and false negatives (a real cycle missed when the pop-ordering interleave races). A.1 mechanics: O(depth) HashSet clones per recurse fanout. Identities are short Strings (~50–200 bytes); total clones bounded by edge count (~50–500 packs in typical manifests). Microseconds per clone vs. milliseconds-to-seconds per network/git I/O — the cost is negligible against the operations the walker actually performs.

## Architectural context

`sync_meta` is the public entry (`crates/grex-core/src/tree/walker.rs:643-651`). It immediately delegates to `sync_meta_inner` (`walker.rs:653-684`), which performs:

1. `loader.load(meta_dir)` → `manifest`
2. `validate_children_paths(&manifest)` (path-traversal sweep)
3. `build_pool(opts.parallel)` (rayon `ThreadPool` per frame)
4. `phase1_sync_children(...)` — par_iter clone/fetch over `manifest.children`
5. `phase2_prune_orphans(...)` — sequential orphan sweep
6. `phase3_recurse(...)` — par_iter recursion into each child via `sync_meta_inner` again (`walker.rs:996`)

`build_graph` runs as a separate orchestrator step AFTER `sync_meta` returns. By then a cyclic manifest has already exploded the disk.

The fix re-introduces the legacy `Walker::walk` mechanism (still visible at `walker.rs:103-126` for the entry point and `walker.rs:173-237` for the recursive frame), adapted to v1.2.1's split pipeline and rayon-parallel Phase 3. Threading discipline (locked per Q6 + Q7 above):

- caller: `sync_meta` builds `let visited: HashSet<String> = HashSet::from([pack_identity_for_root(meta_dir)]);` and passes it down by reference.
- recursion edge (Phase 3): for each child, clone the parent's `visited` into an owned `child_visited`, insert the child's identity, hand `&child_visited` to `sync_meta_inner`. No push/pop is required because each branch owns its set; the set is dropped naturally when the branch returns.
- check: just before the recursive `sync_meta_inner` call, if the child's identity is already in the parent's `visited`, return `TreeError::CycleDetected { chain }` with the path-from-root identities as the chain.

## Algorithm

Per the locked decisions above: the cycle check fires at the **Phase 3 recurse edge** (Q6), and visited state propagates via **per-child owned clone** (Q7, Option A.1). The pseudocode below is normative.

Pseudocode for the modified `sync_meta_inner`:

```rust
fn sync_meta_inner(
    meta_dir: &Path,
    backend: &dyn GitBackend,
    loader: &dyn PackLoader,
    opts: &SyncMetaOptions,
    prune_candidates: &[PathBuf],
    depth: usize,
    visited: &HashSet<String>,                 // <- NEW: ancestor identities on path-from-root
) -> Result<SyncMetaReport, TreeError> {
    let manifest = loader.load(meta_dir)?;
    validate_children_paths(&manifest)?;

    let mut report = SyncMetaReport { metas_visited: 1, ..SyncMetaReport::default() };
    let pool = build_pool(opts.parallel)?;

    // Phase 1 clones siblings only — siblings are not ancestors, so no cycle
    // check is needed here. Phase 2 is a sequential prune sweep, also
    // ancestor-free.
    phase1_sync_children(&pool, meta_dir, &manifest, backend, opts, &mut report);
    phase2_prune_orphans(meta_dir, prune_candidates, opts, &mut report);

    // Phase 3 is the recursion edge: each child that becomes a sub-meta
    // descends through `sync_meta_inner`. The cycle check fires HERE —
    // immediately before the recursive call — so we never enter a frame
    // whose identity is already on the path-from-root.
    phase3_recurse(&pool, meta_dir, &manifest, backend, loader, opts, depth, visited, &mut report);

    Ok(report)
}
```

Inside `phase3_recurse`'s parallel closure — the recursion edge, where Q6 + Q7 are realised together:

```rust
// Phase 3: recurse into each child's manifest, in parallel.
// Cycle check fires at the recurse edge (not Phase 1 clone).
fn phase3_recurse(parent_visited: &HashSet<String>, children: &[Child]) -> Result<()> {
    children.par_iter().try_for_each(|child| {
        let id = pack_identity_for_child(child);
        if parent_visited.contains(&id) {
            return Err(TreeError::CycleDetected { chain: build_chain(parent_visited, &id) });
        }
        let mut child_visited = parent_visited.clone();   // A.1: per-child owned clone
        child_visited.insert(id);
        sync_meta_inner(child, &child_visited)
    })
}
```

Two properties this realises directly:

- **Detection point (Q6).** The legacy `Walker::walk` did clone+recurse in a single fused frame, so "before clone" and "before recurse" were the same instant. v1.2.0's split moved cloning to Phase 1 and recursion to Phase 3 of the SAME frame. A cyclic manifest still requires Phase 1 to run once for the cyclic child (a clone of the cyclic URL into a fresh dest), but Phase 3 is what would re-enter `sync_meta_inner` against that cloned dest's manifest — and the manifest just cloned re-references the same URL. The check at the Phase 3 recursion edge breaks the loop after exactly one clone of each cyclic node, matching the legacy mechanism's behaviour: detection fires before the SECOND clone of any URL.
- **State propagation (Q7).** Each rayon branch owns its own `HashSet<String>` clone with the child's identity inserted. The branch never observes its siblings' state, so the path-from-root invariant ("`child_visited` = identities along the unique path from the cwd-meta root to this frame's child") holds independently per branch. Per the unanimous senior review, a single shared `Arc<Mutex<HashSet>>` would NOT preserve that invariant — it would conflate ancestors with the union of all live branches and produce both false positives and false negatives under the parallel sibling interleave.

## Identity contract

Identity functions already exist at `crates/grex-core/src/tree/walker.rs:315-326`:

```rust
fn pack_identity_for_root(path: &Path) -> String {
    format!("path:{}", path.display())
}

fn pack_identity_for_child(child: &ChildRef) -> String {
    let rref = child.r#ref.as_deref().unwrap_or("");
    format!("url:{}@{}", child.url, rref)
}
```

These are the SAME identity functions used by `graph_build` (re-exported via the module). v1.2.2 reuses them verbatim — no new identity surface is introduced.

The `url:<url>@<ref>` form means **the same repo at two distinct refs is treated as two distinct nodes** — a diamond dependency on the same upstream at different tags is NOT a cycle. This matches git semantics and avoids false positives for legitimate cross-meta sharing.

The cwd-meta itself uses `pack_identity_for_root` (path-keyed) as the seed of the visited set at the top frame. Its child IDs use `pack_identity_for_child` (URL+ref-keyed). The two namespaces (`path:` vs `url:`) are syntactically disjoint, so a hostile manifest cannot collide a child URL with the root path.

## Lean4 spec

New theorem in `proof/Grex/Walker.lean`:

```lean
/-- **`sync_meta_no_cycle_infinite_clone` (v1.2.2).**

    Under the precondition that the manifest forest reachable from
    `parent` is acyclic — every URL@ref appears at most once on any
    root-to-leaf path — `sync_meta` halts in a finite number of recursion
    frames. The Rust impl threads a `visited: HashSet<String>` of
    path-from-root identities through `sync_meta_inner`'s Phase 3
    recursion edge, cloned per child (Q7 = A.1), mirroring the legacy
    `Walker::walk` mechanism: a child whose `pack_identity_for_child`
    is already in the parent's visited set returns
    `TreeError::CycleDetected` instead of recursing.

    Proof: structural induction on `ManifestTree`. The acyclicity
    precondition discharges the inductive case (no path repeats an
    identity, so the per-branch visited set stays bounded by the tree
    height). The base case (leaf manifest, empty `children:`) is
    immediate — `phase3_recurse` returns without dispatching. No new
    axiom is required: termination follows from `manifest_forest_acyclic`
    plus Lean's structural-recursion check on `syncTree` (already
    accepted by the kernel in v1.2.0). -/
theorem sync_meta_no_cycle_infinite_clone
    (parent : Path) (w : World) (h : manifest_forest_acyclic w.tree) :
    terminates_in_finite_steps (sync parent w) := by
  -- discharge via existing structural-recursion termination + the
  -- acyclic-forest hypothesis. Body to be filled in during Stage 1.
  sorry  -- placeholder during openspec; MUST be discharged before any Rust change lands
```

**No new axiom.** The theorem rides on existing scaffolding in `Grex.Types`:

- `manifest_forest_acyclic` is the precondition predicate (definable as: ∀ root-to-leaf path P in `w.tree`, the URL@ref identities along P are pairwise distinct). Either already present in `Grex.Types` or definable in pure Lean from primitives that are.
- `terminates_in_finite_steps` is a pure-Lean statement (existence of a fuel bound, or equivalently: `sync` is structurally recursive on a well-founded measure).
- The W3 `termination` theorem already proved in `Walker.lean:81-83` covers the case where the kernel accepts `syncTree` as structurally recursive on `ManifestTree`. The new theorem extends that reasoning to the augmented `sync_meta_inner` model with the per-branch visited set — pure model-level argument, no FS bridge needed.

The bridge axiom count (CI-gated at 9 propositional in `Bridge.lean` + 4 model-placeholder in `Types.lean`, see `.github/workflows/ci.yml:218-265`) does NOT change.

## Edge cases

1. **Self-loop A→A.** Pack `A` declares `children[0].url = A.url`. After Phase 1 clones `A` into `<meta_dir>/A`, Phase 3 attempts to recurse into that clone. The clone's manifest re-declares `A.url` as its child — the identity `url:<A.url>@<ref>` is in the inherited visited set. Phase 3 returns `CycleDetected { chain: [path:<root>, url:<A.url>@<ref>, url:<A.url>@<ref>] }`. **Detection point**: the Phase 3 recursion edge inside the SECOND frame (the clone of A), against ITS attempt to descend into the same URL.
2. **3-node cycle A→B→C→A.** Visited set grows `{root, A, B, C}` along the path; the recursion from C into A finds `A` already present; returns `CycleDetected { chain: [..., A] }`. Same mechanism, deeper.
3. **Same repo at two refs (positive case).** `A@v1` and `A@v2` declared as siblings in the same parent. Identities `url:<A>@v1` and `url:<A>@v2` are distinct. Both descend cleanly. No cycle. **Test must include this case to lock the identity contract.**
4. **Same repo at two refs in a chain.** Parent declares `A@v1`; `A@v1`'s manifest declares `A@v2`; `A@v2`'s manifest declares `A@v1`. Visited along the path: `{root, url:<A>@v1, url:<A>@v2}`, then the descent into `A@v1` from the third frame finds `url:<A>@v1` already present. Detected at that recursion edge. (This is technically a cycle by URL but masked across refs at the first transition; identity-based detection catches it on the third hop.)
5. **Diamond on same ref (NOT a cycle).** Parent declares B and C as siblings; both B and C declare D@v1 as their child. D@v1 lands at two distinct dest paths (`<parent>/B/D` and `<parent>/C/D`), each branch carrying its own per-child clone of the visited set per Q7 (A.1). Neither branch observes the other. No cycle. (This is the legitimate use-case for diamond dependencies and must work.)

## Files touched

**Rust (production):**
- `crates/grex-core/src/tree/walker.rs` — thread `visited` through `sync_meta_inner`, `phase3_recurse`. Add cycle check at the recursion edge. Reuse existing `pack_identity_for_root` / `pack_identity_for_child` (lines 315-326). Add unit tests inside the existing `#[cfg(test)] mod tests` block.

**Rust (test-only):**
- `crates/grex/tests/sync_e2e.rs:258` — remove the `#[ignore]` annotation on `e2e_cycle_aborts`. Update or remove the explanatory NOTE comment at `:249-257` (the gap is closed).

**Lean:**
- `proof/Grex/Walker.lean` — add theorem `sync_meta_no_cycle_infinite_clone` with full proof body (no `sorry`, no `admit`).

**Versioning:**
- `Cargo.toml` (workspace root): `version = "1.2.1"` → `"1.2.2"`.
- Workspace member `Cargo.toml`s if any pin a non-workspace version (`crates/grex/Cargo.toml`, `crates/grex-core/Cargo.toml`, `crates/grex-mcp/Cargo.toml`, `crates/grex-plugins-builtin/Cargo.toml`, `crates/xtask/Cargo.toml`).

**Changelog/history:**
- SSOT `.omne/cfg/history.md` — append v1.2.2 entry (separate repo per Rule 7, ships through SSOT).
- `CHANGELOG.md` if grex tracks one (verify during impl).

## Acceptance criteria

1. `proof/Grex/Walker.lean` builds clean: `cd proof && lake build` exits 0; zero `sorry`, zero `admit` (verified by the existing strict pattern grep). The new theorem `sync_meta_no_cycle_infinite_clone` is present.
2. `e2e_cycle_aborts` un-`#[ignore]`'d and passes under `cargo test -p grex --test sync_e2e e2e_cycle_aborts`.
3. New unit tests pass:
   - `walker::tests::cycle_self_loop_aborts`
   - `walker::tests::cycle_three_node_aborts`
   - `walker::tests::two_refs_same_url_not_cycle` (positive case — must succeed without `CycleDetected`)
4. `cargo fmt --all -- --check` clean.
5. `cargo doc --no-deps --workspace` clean under `RUSTDOCFLAGS=-D warnings` (or equivalent CI invocation).
6. CI axiom-policy step at `.github/workflows/ci.yml:218-265` stays green (counts unchanged: 9 + 4).
7. `cargo test --workspace` green (existing + new tests).
8. Workspace version bumped to 1.2.2.

## Migration note for changelog

```
## [1.2.2] — 2026-05-XX

### Fixed

- `grex sync` against a manifest forest with a cyclic child URL now returns
  `TreeError::CycleDetected` immediately instead of cloning the cyclic
  repository in a tight loop until the disk fills (regression introduced
  in v1.2.0 when the sync pipeline was split between `sync_meta` and
  `build_graph` — cycle detection ran only in the read pass after the
  mutating pass had already exploded). The check is mechanically
  equivalent to the legacy v1.1.x `Walker::walk` mechanism, adapted to
  v1.2.1's parallel Phase 3: a per-branch `HashSet<String>` of
  `pack_identity_for_child(child)` (URL@ref) entries, cloned at every
  recursion edge.

### Notes

- No API change. Same `TreeError::CycleDetected` variant the read pass
  already produced; no new error variant, no schema change.
- The same upstream repo at two distinct refs is treated as two distinct
  nodes, matching git semantics — diamond dependencies on different tags
  remain valid and are not flagged.
```
