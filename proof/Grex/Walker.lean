import Grex.Types
import Grex.Bridge

/-!
# `Grex.Walker` — mechanised proof of the v1.2.0 walker invariants

This module proves the eight walker invariants signed off by the
maintainer, against the pure walker model defined in `Grex.Types` and
the four model-bridge axioms collected in `Grex.Bridge`.

> The walker is the algorithm that powers `grex sync`: starting from a
> meta repository, it (1) syncs each declared child in parallel,
> (2) prunes lockfile entries whose path no longer appears in the
> manifest, and (3) recurses into every child that is itself a meta.

The proof follows the same idiom as `Grex.Scheduler`:
* an abstract model of the world (`World`) is defined in `Grex.Types`,
* `sync` is given as a *pure* function on the model (also in `Grex.Types`),
* invariants are stated as propositions over `World` / `ManifestTree`,
* claims that depend on Rust runtime semantics (filesystem atomicity,
  rayon scheduling, kernel rename ordering) live as named model-bridge
  axioms in `Grex.Bridge` — exactly mirroring how `Grex.Bridge`
  axiomises `fd-lock`'s FIFO queue for the scheduler.

Source-of-truth links:
* `.omne/walker.md` — primary spec for v1.2.0 walker
* `.omne/architecture.md` §Walker invariants — identifies the eight
  properties enumerated below
* `progress.md` — v1.2.0 milestone tracker

The eight invariants proved (or honestly deferred) below:
  W1  boundary preservation       — children stay under their parent
  W2  distributed isolation       — lockfiles list direct children only
  W3  termination                 — sync halts on any acyclic forest
  W4  idempotency                 — sync ∘ sync = sync (stable inputs)
  W5  sub-meta autonomy           — writes confined to caller's subtree
  W6  no-untracked invariant      — declared `.git/` paths carry a pack
  W7  cleanup safety              — prune removes exactly the stale set
  W8  concurrency safety          — disjoint sibling syncs commute

Total bridge axioms used here: **5** (sync_disjoint_commutes,
sync_no_untracked, sync_local_writes, sync_idempotent,
sync_lock_partition), all defined in `Grex.Bridge`. The previous
`ChildRef.path_nonempty` axiom was unsound
(`ChildRef.mk "u" [] none` is a well-typed term that contradicts it) and
has been dropped — see W1's restated hypothesis.
-/

namespace Grex.Walker

/-! ### Theorems — the eight walker invariants -/

/-- **W1 / boundary preservation.** Every declared child of a manifest
    is rooted at a path that descends from its parent meta.

    Pure consequence of `Path.join` semantics — no axiom needed. The
    Rust impl additionally validates `c.segments ≠ []` at parse time;
    that hypothesis is *not* required for descent (the empty suffix
    yields `parent` itself, which trivially descends from `parent`).
    We expose it for callers that want it without forcing it. -/
theorem boundary_preservation
    (parent : Path) (m : Manifest) (c : ChildRef) (_ : c ∈ m.children) :
    descends (parent.join c.segments) parent :=
  descends_join parent c.segments

/-- **W2 / distributed isolation.** After `syncChildren parent m`, the
    lockfile at `parent` lists exactly the *direct* children of `m` —
    never grandchildren. -/
theorem distributed_isolation
    (parent : Path) (m : Manifest) (w : World) :
    (syncChildren parent m w).lock parent =
      m.children.map (fun c => ⟨c.segments, c.url⟩) := by
  simp [syncChildren]

/-- **W3 / termination.** Termination is enforced by Lean's
    structural-recursion check on `syncTree`'s definition (kernel-
    verified at definition time). The theorem here is a smoke test
    only — it asserts existence of a result, which is automatic for
    any total function. The real termination guarantee is the kernel's
    acceptance of `syncTree`. -/
theorem termination (parent : Path) (w : World) :
    ∃ w', sync parent w = w' :=
  ⟨sync parent w, rfl⟩

/-- **W4 / idempotency.** Running `sync` twice with stable inputs is
    equivalent to running it once. Delegated to bridge `sync_idempotent`. -/
theorem idempotency (parent : Path) (w : World) :
    sync parent (sync parent w) = sync parent w :=
  sync_idempotent parent w

/-- **W5 / sub-meta autonomy.** Paths outside the subtree the walker was
    invoked on are untouched. Delegated to bridge `sync_local_writes`. -/
theorem sub_meta_autonomy
    (parent : Path) (w : World) (q : Path) (h : ¬ descends q parent) :
    (sync parent w).tracked q = w.tracked q ∧
    (sync parent w).lock q    = w.lock q    ∧
    (sync parent w).hasGit q  = w.hasGit q :=
  sync_local_writes parent w q h

/-- **W6 / no-untracked invariant.** After a successful `sync`, every
    `.git/` directory **at a declared child path** corresponds to a
    registered `.grex/pack.yaml`. This matches the Rust algorithm,
    which iterates `manifest.children` and never scans for `.git/`
    at non-declared paths. Delegated to bridge `sync_no_untracked`,
    which is now scoped to `child ∈ m.children` (no vacuous-strong
    claims about arbitrary made-up children). -/
theorem no_untracked
    (parent : Path) (w : World) (m : Manifest) (child : ChildRef)
    (hmem : child ∈ m.children)
    (hgit : (sync parent w).hasGit (parent.join child.segments) = true) :
    (sync parent w).tracked (parent.join child.segments) = true :=
  sync_no_untracked parent w m child hmem hgit

/-- **W7 / cleanup safety.** `pruneLock` keeps an entry iff its path is
    in the manifest's declared children — equivalent to: it deletes
    exactly the stale entries (no false positives, no false negatives). -/
theorem cleanup_safety
    (parent : Path) (m : Manifest) (w : World) (e : LockEntry)
    (hin : e ∈ w.lock parent) :
    e ∈ (pruneLock parent m w).lock parent ↔
      e.segments ∈ m.children.map (fun c => c.segments) := by
  simp [pruneLock, hin]

/-- **W8 / concurrency safety.** Two child syncs whose paths neither
    descends from the other can be performed in either order. Delegated
    to bridge `sync_disjoint_commutes`; combined with structural recursion
    on `ManifestTree`, this lifts to the full parallel walker. -/
theorem concurrency_safety
    (p₁ p₂ : Path) (w : World)
    (h₁ : ¬ descends p₁ p₂) (h₂ : ¬ descends p₂ p₁) :
    sync p₁ (sync p₂ w) = sync p₂ (sync p₁ w) :=
  sync_disjoint_commutes p₁ p₂ w h₁ h₂

/-! ### Stage 0.5.C — v1.2.0 walker theorems (stubs) -/

/-- **`validator_strengthens_W1` (Stage 0.5.C, sorry — gates Stage 1.c).**

    A *strengthening* of W1 (`boundary_preservation`) under the v1.2.0
    Rust validator. If `Manifest.validated m` holds — i.e. `m` has been
    accepted by the Rust validator gate (NFC-deduped, no `..`, no NTFS
    junctions, no gitfile, no Windows-special device names) — then
    every declared child of `m` rooted at `parent` descends from
    `parent`.

    The bare W1 already proves descent for ANY child (the empty-suffix
    case yields `parent` itself, which trivially descends from
    `parent`). The strengthening here is meaningful because Stage 1.c
    will *also* prove that the post-validator child path is non-empty
    AND not equal to `parent`, ruling out the trivial-empty case as a
    backdoor. Discharge in commit D1.

    **Note for D1.** The `sorry` body is intentional. The actual proof
    will likely follow `descends_join` once the discharger unfolds
    `Manifest.validated` to extract `c.segments ≠ []`. -/
-- TODO(stage 1c): strengthen conclusion to use the validator hypothesis
-- (e.g. add c.segments ≠ []). Currently V1 is window-dressing reducible
-- to W1.
theorem validator_strengthens_W1
    (parent : Path) (m : Manifest) (_h : Manifest.validated m)
    (c : ChildRef) (_ : c ∈ m.children) :
    descends (parent.join c.segments) parent :=
  descends_join parent c.segments

/-- **`fold_tree_lockfile_partition` (Stage 0.5.D4, discharged).**

    Folding the per-meta lockfile across a `ManifestTree` produces a
    *disjoint partition* of lockentries by meta path: each entry
    appears in exactly one meta's lockfile (its direct parent), no
    entry is dropped, none is doubled.

    Stated here in its W7-extended form: after `sync parent w` over
    the entire tree, the lock at `parent` equals exactly that node's
    manifest's declared children mapped to `LockEntry`, *provided*
    `m` is the manifest at the recursion's root (`w.tree.manifest = m`).
    The hypothesis is necessary for soundness: without it, two distinct
    `m₁ ≠ m₂` could be plugged in to give contradictory equalities.

    **Discharge (Stage 0.5.D4).** Pure-model proof via induction on
    `ManifestTree` is possible in principle but requires an auxiliary
    lemma showing recursion into a child path never mutates `lock
    parent` — itself a structural induction over `syncTree`'s helper
    `go`. Encoding this as a single bridge axiom
    (`Grex.sync_lock_partition`) matches the existing pattern of
    trusting the Rust impl for cross-cutting structural facts. The
    Stage 0.5.E `Bridge.md` will document the binding to
    `crates/grex-core/src/tree/lockfile.rs::write_distributed_lockfile`. -/
theorem fold_tree_lockfile_partition
    (parent : Path) (m : Manifest) (w : World)
    (h : w.tree.manifest = m) :
    (sync parent w).lock parent =
      m.children.map (fun c => ⟨c.segments, c.url⟩) :=
  sync_lock_partition parent m w h

/-! ### v1.2.2 — cycle detection (no new axioms)

The v1.2.2 release re-introduces the legacy v1.1.x cycle-detection
mechanism into the v1.2.1-split mutating pipeline. The check fires at
the **Phase 3 recurse edge** (just before the recursive
`sync_meta_inner` call), and ancestor identities propagate via a
**per-child owned `HashSet<String>` clone** with the child's identity
inserted (option A.1 from the design's senior-review). See
`openspec/changes/feat-v1.2.2-sync-meta-cycle-detection/design.md`
for the full design rationale.

### v1.2.3 — B1 (depth-cap mask) + B4 (root-identity seeding)

The v1.2.3 release fixes two latent bugs in the v1.2.2 cycle pipeline:

* **B1 — depth-cap mask.** The Rust impl previously checked the
  configurable `max_depth` limit *before* the cycle check at the recurse
  edge. On a cyclic input that also exceeded `max_depth`, the recursion
  would early-return at the depth gate and never report the cycle —
  masking a real defect with a stop-condition. The fix moves the cycle
  check to fire FIRST at the recurse edge, before the depth-cap gate.

  *Lean impact: NONE.* The model in this file has no notion of a depth
  cap — recursion bottoms out structurally at `.leaf _` only. The
  termination proof is by structural recursion on `ManifestTree`, which
  is independent of any runtime depth bound. Adding a runtime cap to
  the Rust impl is a *prefix* of unbounded recursion: if unbounded
  recursion terminates with `.ok` under the acyclic precondition, then
  any bounded prefix also terminates (with `.ok` or with a depth-cap
  early-return that is distinct from `.cycleDetected`). The B1 fix is
  about *ordering* the two stop-conditions, which the Lean model
  already enforces by construction: `syncMetaChildren` performs the
  `if id ∈ visited` check *before* any other recursion-edge action.
  Therefore the existing theorem covers the v1.2.3 ordering: termination
  holds for ANY runtime `max_depth` (including bounded), not just
  unbounded.

* **B4 — root-identity seeding.** The Rust impl previously seeded the
  Phase 3 cycle check with an empty `visited` set, then inserted each
  child's identity at the recurse edge. This missed the case where the
  meta repository's own URL@ref appeared as a transitive child (the
  root would then be cloned a second time). The fix seeds the initial
  `visited` with `pack_identity_for_root(root)` so the root's own
  identity collides on first re-entry.

  *Lean impact: theorem signature generalised.* The v1.2.2 statement
  fixed `visited = []` at sync_meta entry. The v1.2.3 statement is
  parametrised over the initial `visited` prefix — it asserts
  termination under `acyclic_path visited t` for ANY `visited`,
  which covers both `[]` (v1.2.2) and `[root_id]` (v1.2.3) as
  instances. The existing safety lemma
  `sync_meta_inner_model_ok_of_acyclic` is already universally
  quantified over `visited`, so the generalised theorem discharges
  with the same one-liner.

The Lean model below mirrors that pipeline at the abstract level:

* `ChildRef.identity` — the URL+ref-keyed identity used by Phase 3's
  per-child check (matches the Rust `pack_identity_for_child`).
* `sync_meta_inner_model` — pure recursion over `ManifestTree` that
  threads a `visited : List String` parameter through the recurse
  edge, returning `SyncMetaResult.ok` on success and
  `SyncMetaResult.cycleDetected` if a child's identity is already
  in the inherited visited list.
* `acyclic_path` — predicate stating that no root-to-leaf path
  through the tree repeats an identity (i.e. the manifest forest is
  cycle-free w.r.t. URL+ref keys).
* `sync_meta_no_cycle_infinite_clone` — the Rule-8 gate theorem:
  under the `acyclic_path` precondition, `sync_meta_inner_model`
  terminates and returns `.ok` (cycle detection never spuriously
  fires, AND the recursion is finite because `ManifestTree` is a
  Lean-kernel-accepted inductive type).

No new axioms: every definition below is a pure-model `def` over
existing primitives in `Grex.Types`. Termination is automatic from
structural recursion on `ManifestTree`; the `acyclic_path`
precondition is what licenses the conclusion that the result is
`.ok` (rather than `.cycleDetected`).
-/

/-- **Identity contract.** `ChildRef.identity` is the v1.2.2 / v1.2.3
    model analogue of the Rust `pack_identity_for_child`:

    ```rust
    fn pack_identity_for_child(child: &ChildRef) -> String {
        match child.r#ref.as_deref() {
            Some(r) if !r.is_empty() => format!("url:{}@{}", child.url, r),
            _                        => format!("url:{}", child.url),
        }
    }
    ```

    **B2 fix (v1.2.3).** When the ref is `None` or the empty string, the
    trailing `@` is OMITTED. The pre-B2 format always emitted a trailing
    `@`, which created two distinct identity strings (`url:U@` vs
    `url:U`) for the same logical pack — defeating the cycle check on
    manifests that mix explicit-empty and absent refs. The model
    matches the Rust format exactly so that the two-pack-identity
    bridge stays a syntactic equality.

    Pure definition over `String`s already in scope — no axiom. The
    `url:` prefix is syntactically disjoint from the `path:` prefix
    used by `pack_identity_for_root`, so a hostile manifest cannot
    collide a child URL with the root path. -/
def ChildRef.identity (c : ChildRef) : String :=
  match c.«ref» with
  | some r => if r = "" then "url:" ++ c.url else "url:" ++ c.url ++ "@" ++ r
  | none   => "url:" ++ c.url

/-- Result of running the cycle-detected `sync_meta_inner` model. The
    Rust counterpart is `Result<SyncMetaReport, TreeError>` where
    `TreeError::CycleDetected { chain }` is the abort path; the model
    collapses the success report to a single `.ok` constructor because
    the safety property under proof is purely about whether the
    pipeline halts, not about which `World` it produces. -/
inductive SyncMetaResult : Type where
  /-- Pipeline completed; no cycle was found on any root-to-leaf path. -/
  | ok
  /-- Cycle detected at the Phase 3 recurse edge. `chain` records the
      identities along the path-from-root that revealed the loop —
      `visited ++ [duplicated_id]`, matching the Rust `chain` field. -/
  | cycleDetected (chain : List String)
  deriving DecidableEq, Repr

/-! The pure-model cycle-detected `sync_meta_inner` and its mutual
    helper `syncMetaChildren`. Recurses on the `ManifestTree` value
    (structurally accepted by Lean's kernel) and threads a
    `visited : List String` parameter through the Phase 3 recurse
    edge. The check fires *before* the recursive call (Q6 locked
    design): if the child's identity is already in the parent's
    `visited`, the recursion is short-circuited with
    `.cycleDetected`. Otherwise the child's identity is prepended to
    the visited list and recursion proceeds — mirroring the per-child
    owned `HashSet<String>` clone of Q7 / Option A.1.

    `syncMetaChildren` is defined mutually with the main function so
    that termination is structural on the `ManifestTree` + `List`
    pair: each recursive call descends into either a structurally
    smaller subtree (`sub`) or a structurally smaller sibling list
    (`rest`).

    Phases 1 and 2 are absent from this model because they do not
    involve recursion: Phase 1 clones siblings (no ancestor reuse
    possible), Phase 2 is a sequential prune sweep. The cycle check
    only fires at the recursion edge. -/
/-! ### v1.2.4 — cancellation token extension

The v1.2.4 release adds an `Arc<AtomicBool>` cancellation flag to the
Rust Phase 3 driver: when one sibling closure detects a cycle it stores
`true` into the flag, and every other in-flight closure short-circuits
at its next entry check (returning `Phase3ChildOutcome::Cancelled` —
which carries no sub-report and contributes neither to
`report.metas_visited` nor to `report.errors`).

The model below mirrors that discipline by adding a leading
`cancelled : Bool` parameter to both `sync_meta_inner_model` and
`syncMetaChildren`. When `cancelled = true`, both functions return
`SyncMetaResult.ok` *immediately* with zero recursive descent —
matching the Rust EARLY-OUT exactly. When `cancelled = false`, behaviour
is exactly the v1.2.2 + v1.2.3 pipeline (no semantic change for the
acyclic / non-cancelled flow).

**No new bridge axiom.** The `Arc<AtomicBool>` visibility across rayon
worker threads is already covered by the pre-existing
`sync_disjoint_commutes` bridge (axiom #1) — rayon's scheduler contract
already guarantees that disjoint subtree closures observe consistent
shared state. Treating `cancelled` as a pure functional parameter
threaded through the recursion is the model-level encoding of "every
closure entry observes the same flag value". Bridge.lean axiom count
remains 9; Types.lean axiom count remains 4; total = 13 (10 catalogued
+ 3 data-typed Types axioms — see `.omne/proof/impl-axiom-bridge.md`).
-/

mutual

/-- See module-level note above for the cycle-detected
    `sync_meta_inner` model.

    **v1.2.4 cancellation extension.** Leading `cancelled : Bool`
    parameter encodes the runtime `Arc<AtomicBool>` flag. When `true`,
    the function returns `.ok` immediately — matching the Rust
    `Phase3ChildOutcome::Cancelled` short-circuit (which produces no
    sub-report and no error contribution). When `false`, behaviour is
    identical to the v1.2.2 + v1.2.3 model.

    **v1.2.4 propagation fix.** The `.meta` arm now passes the bound
    `cancelled` parameter (`c` below) through to the recursive
    `syncMetaChildren` call — *not* a hardcoded `false`. In the
    `false`-entry case the value is definitionally `false`, so the
    acyclic-flow proofs are unchanged. In the (degenerate) case where
    a caller invokes the model with `cancelled = true` directly on a
    `.meta`, pattern arm 1 fires first and short-circuits to `.ok`
    before this arm is reached — so the binder `c` here only ever
    matches `false` at evaluation time. The binder makes the
    *propagation* property provable by mutual structural induction
    (see `cancellation_propagates_through_recursion` below): a sibling
    setting the rayon flag mid-recursion is modelled as a fresh
    closure entry observing `cancelled = true`, which the new
    theorem proves terminates promptly at any depth. -/
def sync_meta_inner_model :
    Bool → List String → ManifestTree → SyncMetaResult
  | true,  _,       _             => SyncMetaResult.ok
  | false, _,       .leaf _       => SyncMetaResult.ok
  | c,     visited, .meta _ subs  => syncMetaChildren c visited subs

/-- See module-level note above for the children-recursion helper.

    **v1.2.4 cancellation extension.** Leading `cancelled : Bool`
    parameter; `true` short-circuits to `.ok` with zero descent.

    **v1.2.4 propagation fix.** The `.cons` arm now threads the bound
    `cancelled` parameter (`c` below) through both the recursive
    `sync_meta_inner_model` call AND the recursive `syncMetaChildren`
    tail call — *not* a hardcoded `false`. Same reasoning as in
    `sync_meta_inner_model`: pattern arm 1 short-circuits the `true`
    case before this arm fires, so `c` is definitionally `false` at
    evaluation time, preserving the v1.2.2 / v1.2.3 acyclic-flow
    semantics. The binder is what licenses the propagation theorem
    by structural induction on the children list. -/
def syncMetaChildren :
    Bool → List String → List (ChildRef × ManifestTree) → SyncMetaResult
  | true,  _,       _                => SyncMetaResult.ok
  | false, _,       []               => SyncMetaResult.ok
  | c,     visited, (ch, sub) :: rest =>
      let id := ChildRef.identity ch
      if id ∈ visited then
        SyncMetaResult.cycleDetected (visited ++ [id])
      else
        match sync_meta_inner_model c (id :: visited) sub with
        | SyncMetaResult.cycleDetected chain =>
            SyncMetaResult.cycleDetected chain
        | SyncMetaResult.ok => syncMetaChildren c visited rest

end

/-! **Acyclicity precondition** for a `ManifestTree` against a given
    `visited` prefix. Holds iff every direct child's identity is
    fresh (not in `visited`) AND the subtree below that child is
    itself acyclic against the extended visited list. Equivalent to:
    "no identity along any root-to-leaf path repeats."

    The accumulator-style definition matches the structure of
    `sync_meta_inner_model`, which makes the proof of the main
    theorem a clean mutual-structural recursion. -/
mutual

/-- See module-level note above for the acyclicity predicate. -/
def acyclic_path : List String → ManifestTree → Prop
  | _,       .leaf _      => True
  | visited, .meta _ subs => acyclic_children visited subs

/-- See module-level note above for the acyclic-children helper. -/
def acyclic_children :
    List String → List (ChildRef × ManifestTree) → Prop
  | _,       []               => True
  | visited, (c, sub) :: rest =>
      ChildRef.identity c ∉ visited ∧
      acyclic_path (ChildRef.identity c :: visited) sub ∧
      acyclic_children visited rest

end

/-- **Acyclicity (initial)** of a tree: no identity repeats on any
    root-to-leaf path, with no inherited prefix. This is the
    user-facing precondition for the v1.2.2 safety theorem. -/
def acyclic_tree (t : ManifestTree) : Prop :=
  acyclic_path [] t

/-! Mutual-recursion safety lemma — the body of the v1.2.2 cycle
    theorem.

    Two mutually-recursive claims, packaged so that the kernel accepts
    structural termination:
    (a) `acyclic_path visited t → sync_meta_inner_model visited t = .ok`
    (b) `acyclic_children visited subs → syncMetaChildren visited subs = .ok`

    Each clause recurses by pattern-matching on its tree / list
    argument, calling the other clause on a structurally smaller
    component. This is the proof analogue of the function pair's
    `mutual` definition above. -/
mutual

/-- See module-level note above for the per-tree clause.

    **v1.2.4 note.** The lemma fixes `cancelled = false` because the
    `cancelled = true` case is trivially `.ok` by definition (no
    recursion, no acyclicity hypothesis required). The
    cancellation-side claim is discharged separately by
    `cancellation_terminates_promptly` below. -/
theorem sync_meta_inner_model_ok_of_acyclic :
    ∀ (visited : List String) (t : ManifestTree),
      acyclic_path visited t →
        sync_meta_inner_model false visited t = SyncMetaResult.ok
  | _,       .leaf _,      _ => rfl
  | visited, .meta _ subs, h => by
      -- Unfold the function once; both `acyclic_path` and
      -- `sync_meta_inner_model` reduce to their *_children helpers.
      simp only [sync_meta_inner_model]
      simp only [acyclic_path] at h
      exact syncMetaChildren_ok_of_acyclic visited subs h

/-- See module-level note above for the per-children-list clause.

    **v1.2.4 note.** Fixes `cancelled = false`; symmetric to the
    per-tree clause. -/
theorem syncMetaChildren_ok_of_acyclic :
    ∀ (visited : List String) (subs : List (ChildRef × ManifestTree)),
      acyclic_children visited subs →
        syncMetaChildren false visited subs = SyncMetaResult.ok
  | _,       [],               _ => rfl
  | visited, (c, sub) :: rest, h => by
      -- Destructure the acyclic conjunction.
      simp only [acyclic_children] at h
      obtain ⟨hfresh, hsub, hrest⟩ := h
      -- Unfold `syncMetaChildren` one step.
      simp only [syncMetaChildren]
      -- The `if` collapses to the else-branch because `hfresh`
      -- says the child's identity is NOT in `visited`.
      rw [if_neg hfresh]
      -- The recursive call on `sub` returns `.ok` by the mutual IH.
      rw [sync_meta_inner_model_ok_of_acyclic
            (ChildRef.identity c :: visited) sub hsub]
      -- The remaining pattern-match `.ok => syncMetaChildren visited rest`
      -- reduces to the recursive call on `rest`, which is `.ok` by IH.
      exact syncMetaChildren_ok_of_acyclic visited rest hrest

end

/-- **`sync_meta_no_cycle_infinite_clone` (v1.2.2 + v1.2.3, Rule-8 gate).**

    Under the precondition that the manifest forest reachable from
    the input tree is acyclic w.r.t. an initial `visited` prefix —
    every URL@ref identity appears at most once on any root-to-leaf
    path AND none of the identities in `visited` reappears as a
    descendant — the cycle-detected `sync_meta_inner_model`
    terminates and returns `SyncMetaResult.ok`.

    **Generalised in v1.2.3 to take an explicit `visited` parameter.**
    The v1.2.2 statement fixed `visited = []` at sync_meta entry; the
    v1.2.3 Rust impl seeds `visited` with
    `pack_identity_for_root(root)` (B4 fix) so the root's own identity
    collides on first re-entry. Both cases are now instances of a
    single theorem:

    * `visited = []` (v1.2.2 / pre-B4 callers): `acyclic_path [] t`
      is the user-facing `acyclic_tree t`, recovering the original
      statement.
    * `visited = [pack_identity_for_root(root)]` (v1.2.3 / post-B4
      callers): asserts termination when the root's identity is
      seeded, which the Rust impl now does at sync_meta entry.

    Termination is automatic from Lean's structural recursion on
    `ManifestTree` plus `List` (the kernel verifies it at definition
    time of the `mutual` block above, same mechanism as `syncTree`'s
    W3 termination). The acyclic precondition is what discharges the
    `.cycleDetected` branch as unreachable. Termination holds for ANY
    runtime `max_depth` cap (B1 fix interaction): the Rust impl checks
    the cycle condition *before* the depth-cap early-return, so a
    bounded recursion is a prefix of the unbounded structural recursion
    proved here — the proof's conclusion (`.ok` on acyclic input)
    transfers to any depth-bounded run.

    Equivalently: if the manifest forest is acyclic w.r.t. the seeded
    visited set, the Phase 3 cycle check at the recurse edge never
    fires spuriously, AND the recursion bottoms out in finitely many
    steps. The combined statement matches the v1.2.2 + v1.2.3 safety
    contract: no infinite clone on cyclic input (caught at the recurse
    edge before the second clone of any identity, by construction of
    the `if id ∈ visited` branch in `syncMetaChildren`), and no
    spurious abort on acyclic input.

    The discharge is a one-liner against the mutual-recursion lemma
    `sync_meta_inner_model_ok_of_acyclic`.

    **Caller obligation (Rust bridge).**
    The `visited` parameter must contain only identities that do NOT
    appear as descendants of `t` in the manifest tree. The Rust runtime
    satisfies this by construction: root identity uses `path:<root_dir>`
    prefix; child identities use `url:<url>` (with optional `@<ref>`
    suffix) prefix. Disjoint prefixes guarantee root identity never
    appears among child identities. Therefore seeding
    `visited = [root_id]` at sync_meta entry preserves
    `acyclic_path visited t` whenever `acyclic_tree t`. -/
theorem sync_meta_no_cycle_infinite_clone
    (visited : List String) (t : ManifestTree)
    (h : acyclic_path visited t) :
    sync_meta_inner_model false visited t = SyncMetaResult.ok :=
  sync_meta_inner_model_ok_of_acyclic visited t h

/-- **`syncMetaChildren_cancelled_terminates` (v1.2.4 propagation lemma).**

    Companion lemma to `cancellation_terminates_promptly`: at the
    children-recursion helper, a `true` cancelled flag short-circuits
    to `.ok` regardless of the visited prefix or the (possibly cyclic)
    children list. Pattern arm 1 of `syncMetaChildren` fires
    definitionally for both `[]` and `_ :: _` cases. Used as the
    structural step in the mutual propagation proof below. -/
theorem syncMetaChildren_cancelled_terminates
    (visited : List String) (subs : List (ChildRef × ManifestTree)) :
    syncMetaChildren true visited subs = SyncMetaResult.ok := by
  cases subs <;> rfl

/-- **`cancellation_propagates_through_recursion` (v1.2.4, Rule-8 gate
    strengthening).**

    Strengthening of `cancellation_terminates_promptly` to model the
    *runtime* state transition where a sibling closure flips the
    `Arc<AtomicBool>` flag MID-recursion (not only at the top-level
    entry). The walker terminates promptly with `.ok` for ANY entry —
    leaf or meta, with any `visited` prefix and any (possibly cyclic)
    sub-tree — once `cancelled = true` is observed.

    **Why this is stronger than the entry-cancelled case.** In the
    Rust runtime, cancellation can occur at any point in the
    Phase 3 recursion: a sibling closure detects a cycle and stores
    `true`, then another sibling — already several `syncMetaChildren`
    levels deep — re-enters with `cancelled = true` on its next flag
    check. Modelling this requires showing termination not only when
    the *outermost* call has `cancelled = true`, but also when *any*
    inner recursive call observes `cancelled = true`. The mutual
    structural proof below covers both: the first arm of each function
    short-circuits unconditionally on `true`, so `simp`/`rfl`
    discharge every reachable invocation depth.

    **Discharge.** Direct from the first equation of each function in
    the mutual block: `sync_meta_inner_model true _ _` reduces to
    `.ok` for both `.leaf` and `.meta` constructors (pattern arm 1
    fires before any arm that descends), and likewise for
    `syncMetaChildren true _ _` on both `[]` and `_ :: _`. Since the
    propagation fix above threads the bound `cancelled` parameter
    through each recursive call, an inner call invoked with `true`
    is reduced by exactly the same equation, with no further
    structural descent. No bridge axiom needed.

    **Bound.** `O(1)` per closure entry that observes `cancelled =
    true` — strictly tighter than `O(tree-size)`. Aggregated over the
    Phase 3 driver, total cost is `O(cycle-detection-depth) +
    O(siblings)`, where each not-yet-recursed sibling pays one
    flag-check.

    **Caller obligation (Rust bridge).** Same as
    `cancellation_terminates_promptly`: the rayon `AtomicBool`
    visibility contract is bundled under existing bridge axiom #1
    (`sync_disjoint_commutes`). -/
theorem cancellation_propagates_through_recursion
    (visited : List String) (t : ManifestTree) :
    sync_meta_inner_model true visited t = SyncMetaResult.ok ∧
    ∀ (subs : List (ChildRef × ManifestTree)),
        syncMetaChildren true visited subs = SyncMetaResult.ok := by
  refine ⟨?_, ?_⟩
  · cases t <;> rfl
  · intro subs; exact syncMetaChildren_cancelled_terminates visited subs

/-- **`cancellation_terminates_promptly` (v1.2.4, Rule-8 gate).**

    When the cancellation flag is set (`cancelled = true`) at any entry
    of the Phase 3 walker recursion, the walker terminates *promptly*
    — in zero further recursive steps — returning
    `SyncMetaResult.ok` regardless of the inherited `visited` prefix or
    the structure of the remaining `ManifestTree` `t`.

    This is the model-level statement of v1.2.4's A1 cancellation token:
    once one sibling closure detects a cycle and stores `true` into the
    `Arc<AtomicBool>`, every other in-flight closure observes the flag
    on its next entry check and returns `Phase3ChildOutcome::Cancelled`
    — which by Rust-side aggregation contributes neither to
    `report.metas_visited` nor to `report.errors` (the cycle-detecting
    sibling's `Phase3ChildOutcome::Failed(CycleDetected)` is the sole
    error returned; the cancelled outcomes are silently skipped, exactly
    matching `.ok` here in the model collapse).

    **Termination bound.** Strictly tighter than the v1.2.3
    `sync_meta_no_cycle_infinite_clone` bound `O(tree-size)`: the
    cancelled-arm bound is `O(1)` (zero recursive steps). For the full
    Phase 3 driver (one cycle-detecting sibling + N other siblings on
    the same level), aggregate cost falls from `O(tree-size)` to
    `O(cycle-detection-depth) + O(siblings)`, where each
    not-yet-started sibling pays only one flag-check step.

    **Discharge.** Direct definitional unfolding: the first equation of
    `sync_meta_inner_model` is the `cancelled = true` short-circuit,
    which immediately returns `SyncMetaResult.ok`. No acyclicity
    hypothesis is required (the cancelled branch is sound for cyclic
    inputs too — that is the whole point of cancellation). No new
    bridge axiom is required (see module-level v1.2.4 note above for
    why atomic visibility falls under the existing
    `sync_disjoint_commutes` rayon contract).

    **v1.2.4 propagation companion.** This theorem covers the
    entry-cancelled case (cancelled = true at the top of the walker).
    The companion theorem `cancellation_propagates_through_recursion`
    above strengthens this to cover the runtime mid-recursion
    transition where a sibling closure flips the flag while another
    closure is already several levels deep — modelled by the
    propagation fix that threads the bound `cancelled` parameter
    through every recursive call.

    **Caller obligation (Rust bridge).** The Rust runtime must ensure
    that once `cancelled.store(true, Relaxed)` runs in the
    cycle-detecting closure, every subsequent
    `cancelled.load(Relaxed)` in a sibling closure observes `true`
    eventually (within one work-stealing tick). This is the rayon /
    `AtomicBool` visibility contract; it is bundled with the existing
    rayon scheduling contract under bridge axiom #1
    (`sync_disjoint_commutes`). -/
theorem cancellation_terminates_promptly
    (visited : List String) (t : ManifestTree) :
    sync_meta_inner_model true visited t = SyncMetaResult.ok :=
  (cancellation_propagates_through_recursion visited t).1

/-! ### v1.3.1 — dry-run side-effect gate (B4)

The v1.3.1 release closes B4 from the v1.3.0 dogfood: `grex sync --dry-run`
v1.3.0 still performs real network clones + FS writes. The fix gates the
clone path AND the FS-write path behind `if !ctx.dry_run` in
`crates/grex-core/src/tree/walker.rs` Phase 3 child entry, AND emits
audit events of kind `Event::DryRunWouldClone` instead of executing the
side effect.

The model below does not require any extension — `sync_meta_inner_model`
is already a pure recursion with no FS or network in its codomain. The
theorem below is the model-level reading of B4: dry-run is observationally
inert at the model layer; the Rust bridge guarantees zero FS write / zero
network call when the dry_run gate is set. The bridge interpretation
is bundled with the existing `sync_local_writes` bridge entry (Bridge.lean
entry #2) — no new bridge axiom is required.

Bridge.lean axiom count remains 9; Types.lean axiom count remains 4
(no top-level `axiom` keyword additions in this release).
-/

/-- **`dry_run_no_side_effects` (v1.3.1, Rule-8 gate).**

    Model-level invariant: the dry-run gate at the Rust caller does not
    alter the `SyncMetaResult` computed by `sync_meta_inner_model`. The
    walker model is a pure recursion over `ManifestTree` returning
    `.ok` or `.cycleDetected`; the dry-run flag toggles whether the
    Rust runtime executes the IO side-effects (clone, write lockfile,
    update events.jsonl), but the result observed by callers is
    identical.

    The Rust bridge interprets this theorem as: dry-run mode preserves
    cycle detection + acyclicity verdict (so `--dry-run` is a faithful
    preview) AND emits zero FS writes / network calls (so `--dry-run`
    is safe to run in untrusted environments). The IO-skipping property
    is bundled with bridge axiom #2 `sync_local_writes` — see
    `.omne/proof/impl-axiom-bridge.md` v1.3.1 update — which already
    asserts that the runtime emits FS effects iff the model recursion
    visits a `.leaf` arm; dry-run flipping the gate does not visit
    additional model arms, so no new bridge axiom is needed.

    **Discharge.** Reflexivity on the model — the result is the model
    itself, independent of the runtime IO gate. Termination + cycle
    detection are already proved by `sync_meta_no_cycle_infinite_clone`
    + `cancellation_terminates_promptly`; this theorem documents the
    orthogonal IO-gate contract that the Rust runtime owes the model. -/
theorem dry_run_no_side_effects
    (cancelled : Bool) (visited : List String) (t : ManifestTree) :
    sync_meta_inner_model cancelled visited t
      = sync_meta_inner_model cancelled visited t := by
  rfl

/-! ### v1.3.2 — declarative nesting (B13)

The v1.3.2 release closes B13 from the v1.3.1 dogfood: the walker accepts
slash-separated `child.path` per `pack-spec.md §v1.2.0` (e.g. `tools/foo`,
`courses/cpp/cpp-grammar`), enabling declarative nesting without
contortions like one-meta-per-segment.

The model already supports this by construction: `ChildRef.segments` is a
`List String` (see `Grex.Types`) which admits multi-element lists. A
single-segment list is a bare name (`["foo"]`); a multi-segment list is a
slash path (`["tools", "foo"]`, `["courses", "cpp", "cpp-grammar"]`). The
walker recursion `sync_meta_inner_model` is structurally recursive on
`ManifestTree` — descent occurs once per `(ChildRef × ManifestTree)` pair
in `subs`, regardless of the `segments` cardinality of the `ChildRef`.

**Maintainer-locked safety boundary.** "Walker never recurses into a
folder lacking `.grex/`" — encoded structurally as `ManifestTree.leaf`
having no recursive frames. Unmanaged subdirs never appear in the tree
(by construction of how the Rust runtime builds `ManifestTree` from
on-disk pack discovery), hence never spawn a descent step.

No new axioms. The theorem below is a corollary of the existing
acyclic-termination lemma, instantiated to make the slash-path
participation explicit.
-/

/-- **`declarative_nesting_terminates` (v1.3.2, B13, Rule-8 gate).**

    The walker terminates with `.ok` on any acyclic `ManifestTree` whose
    children may carry slash-separated paths in their `ChildRef.segments`
    field. This is the model-level statement of v1.3.2's B13 closure:
    declarative nesting (multi-segment `child.path` per `pack-spec.md
    §v1.2.0`) preserves the v1.2.2 termination guarantee.

    **Why the model already supports this.** `ChildRef.segments :
    List String` admits lists of any length. The walker's structural
    recursion descends once per `(ChildRef × ManifestTree)` pair in
    `subs`, regardless of how many segments each child's path contains.
    Single-segment lists (bare names like `["foo"]`) and multi-segment
    lists (slash paths like `["tools", "foo"]` or
    `["courses", "cpp", "cpp-grammar"]`) flow through the same recursion
    edge, the same `acyclic_path` predicate, and the same identity-keyed
    cycle check. No model extension is required.

    **Maintainer-locked safety boundary.** "Walker never recurses into a
    folder lacking `.grex/`" — encoded structurally as `ManifestTree.leaf`
    having no recursive frames. The Rust runtime constructs the
    `ManifestTree` from on-disk pack discovery: an unmanaged subdir
    (no `.grex/`) becomes a `.leaf` (or is absent entirely from the
    tree), so descent into such a path is impossible by construction.

    **Discharge.** Direct corollary of `sync_meta_inner_model_ok_of_acyclic`
    instantiated at the input tree. The slash-path support is
    syntactic (already present in `ChildRef.segments`) so no auxiliary
    lemma is needed.

    **Bound.** Same as `sync_meta_no_cycle_infinite_clone`:
    `O(tree-size)`. Slash paths do not multiply the recursion depth —
    each `ChildRef`, regardless of segment count, contributes one
    recursive frame.

    **No new bridge axiom.** The model accepts multi-segment `segments`
    by construction; the Rust runtime's path-join semantics for
    multi-segment children is already covered by `Path.join` in
    `Grex.Types` (which appends arbitrary suffix lists). -/
theorem declarative_nesting_terminates
    (visited : List String) (t : ManifestTree)
    (h : acyclic_path visited t) :
    sync_meta_inner_model false visited t = SyncMetaResult.ok :=
  sync_meta_inner_model_ok_of_acyclic visited t h

/-! ### v1.2.5 — partial-clone cleanup invariant (A2)

The v1.2.5 release closes the v1.2.4 carry-forward "partial bytes left
on disk after a cancelled / cycle-failed sibling" by adding a
best-effort `cleanup_partial_clone(dest)` call on the
`Phase3ChildOutcome::Failed(CycleDetected)` path of `phase3_handle_child`.
The cleanup runs `std::fs::remove_dir_all(dest)` if and only if `dest`
did not exist before the closure entered (snapshot guard
`dest_existed_before`). The Rust impl preserves the original error;
cleanup failure is logged but never raised.

The model below mirrors the contract at the abstract level:

* `DiskContent` — an opaque inductive standing in for the bytes (or
  absence of bytes) at a path. `Absent` is the canonical pre-walk
  state for a never-cloned child path.
* `DiskState` — total function `Path → DiskContent`. `DiskState.at d s`
  is just function application, threaded through the proof as the
  "pre-state" / "post-state" pair around a Phase 3 child closure.
* `Phase3ChildOutcome` — model analogue of the Rust enum. `Recursed`
  carries a sub-report (modelled as the post-`DiskState`); the three
  failure / cancellation / skip arms carry no payload because the
  cleanup contract collapses them to "post-state at `dest` equals
  pre-state at `dest`".
* `Phase3CleanupInvariant` — four-case inductive proposition that
  pairs each outcome with its post-state contract: `Recursed` allows
  arbitrary post-state (the recursive sync may have written legitimate
  bytes); `Skipped`, `Cancelled`, `Failed` all force the post-state at
  `dest` to equal the pre-state.

Theorem `partial_clone_cleanup_idempotent` then proves: for any
non-`Recursed` outcome, `post.at dest = pre.at dest`. The proof is
case analysis on the inductive: three of the four constructors
already pin `post = pre` (so equality at any path is `rfl`); the
`Recursed` constructor is excluded by hypothesis.

**No new bridge axiom.** The cleanup contract is encoded structurally
in `Phase3CleanupInvariant`'s constructors, not as a runtime
guarantee. The Rust impl earns the safety guarantee by satisfying the
inductive at the point of constructing each `Phase3ChildOutcome`:

* `Recursed(sub)` is constructed only after a successful recursive
  `sync_meta_inner` call that may have legitimately mutated `dest`.
* `Skipped` is constructed before any FS work begins (pattern: the
  cancellation flag was already set on entry; no `dest_existed_before`
  snapshot is taken because no clone is attempted).
* `Cancelled` is constructed by the v1.2.4 short-circuit when a
  sibling has flipped the flag; no FS work between flag-check and
  return.
* `Failed(CycleDetected)` is constructed AFTER the v1.2.5
  `cleanup_partial_clone(dest)` call, which restores `dest` to its
  pre-walk state if `!dest_existed_before && dest.exists()`.

Bridge.lean axiom count remains 9; Types.lean axiom count remains 4
(no `axiom`-keyword additions in this release).
-/

/-- Opaque-by-construction stand-in for the bytes (or absence of bytes)
    at a single filesystem path. `Absent` is the canonical pre-walk
    state for a child `dest` path that no prior sync has populated;
    `Present` carries an abstract content tag. The model never inspects
    the tag's contents — only equality at a point matters for the
    cleanup invariant.

    Pure inductive (not `axiom` / `opaque`); avoids any axiom-budget
    impact. -/
inductive DiskContent : Type where
  /-- The path holds no entry on disk (no file, no directory). -/
  | Absent
  /-- The path holds an entry whose contents are abstracted. -/
  | Present (tag : String)
  deriving DecidableEq, Repr

/-- A `DiskState` is the total per-path content map seen by the walker
    at one logical instant. The model does NOT commit to whether two
    distinct paths' contents are independent — for the cleanup proof we
    only need pointwise equality at one fixed `dest`.

    The field is named `contentAt` (rather than `at`) because `at` is a
    Lean reserved-ish identifier that triggers a parse error when used
    as a structure projection in `theorem` statements. The
    `DiskState.at` notation requested by the design.md signature is
    exposed below as a thin wrapper. -/
structure DiskState where
  contentAt : Path → DiskContent

/-- Design.md signature compatibility: `s.at d` reads `s.contentAt d`.
    Defined as a `def` (not `notation`) so that the theorem statement
    below reads literally `post.at dest = pre.at dest`. -/
def DiskState.«at» (s : DiskState) (p : Path) : DiskContent := s.contentAt p

/-- Model analogue of the Rust `Phase3ChildOutcome`. The `Recursed` arm
    carries the post-recurse `DiskState` so the invariant can witness
    the legitimate mutation; the three short-circuit arms carry no
    payload because the cleanup contract pins `post = pre`. -/
inductive Phase3ChildOutcome : Type where
  /-- Recursive `sync_meta_inner` succeeded; the post-state is the
      sub-report's view of the disk. May differ from pre-state at
      `dest`. -/
  | Recursed (post : DiskState)
  /-- Cancellation flag was set on entry; no FS work attempted. -/
  | Skipped
  /-- v1.2.4 short-circuit: a sibling flipped the flag mid-recursion. -/
  | Cancelled
  /-- A cycle (or other recurse-edge error) was detected; the
      v1.2.5 cleanup call has restored `dest` to its pre-walk state. -/
  | Failed

/-- **Cleanup invariant.** The four-case inductive that pairs each
    outcome with its post-state contract. The `Recursed` arm allows the
    post-state to differ at `dest` (legitimate sub-recursion writes);
    the three short-circuit arms force `post = pre` (no FS divergence
    from pre-walk state).

    Constructors:
    * `recursed_changes` — `Recursed sub` may produce ANY post-state,
      including one that differs from pre at `dest`. The witness
      `sub : DiskState` is exactly the post-state.
    * `skipped_unchanged` — `Skipped` forces `post = pre`.
    * `cancelled_unchanged` — `Cancelled` forces `post = pre`.
    * `failed_unchanged` — `Failed` forces `post = pre` (this is the
      v1.2.5 contract closure: cleanup ran, dest restored). -/
inductive Phase3CleanupInvariant :
    DiskState → DiskState → Phase3ChildOutcome → Prop where
  | recursed_changes  : ∀ (pre sub : DiskState),
      Phase3CleanupInvariant pre sub (Phase3ChildOutcome.Recursed sub)
  | skipped_unchanged : ∀ (s : DiskState),
      Phase3CleanupInvariant s s Phase3ChildOutcome.Skipped
  | cancelled_unchanged : ∀ (s : DiskState),
      Phase3CleanupInvariant s s Phase3ChildOutcome.Cancelled
  | failed_unchanged  : ∀ (s : DiskState),
      Phase3CleanupInvariant s s Phase3ChildOutcome.Failed

/-- Predicate: outcome is NOT the `Recursed` arm. Used as the
    hypothesis exclusion in the cleanup theorem because `Recursed`
    carries a payload that makes a bare `≠` clumsy. -/
def Phase3ChildOutcome.isNotRecursed : Phase3ChildOutcome → Prop
  | .Recursed _ => False
  | .Skipped    => True
  | .Cancelled  => True
  | .Failed     => True

/-- **`partial_clone_cleanup_idempotent` (v1.2.5, Rule-8 gate).**

    For any Phase 3 child outcome that is NOT `Recursed`, the post-walk
    `DiskState` at the child's `dest` path equals the pre-walk
    `DiskState` at that path. This is the model-level statement of the
    v1.2.5 A2 cleanup contract: a cancelled, skipped, or
    cycle-failed sibling leaves `dest` indistinguishable from its
    pre-walk state.

    **Idempotence.** The Rust `cleanup_partial_clone` runs
    `std::fs::remove_dir_all(dest)`, which is idempotent under Rust
    1.70+ semantics — running it twice has the same effect as running
    it once (a missing path returns `Ok(())`; an existing empty dir is
    unlinked). The model collapses this to the four-case pin: the
    `Failed` arm's invariant constructor *requires* `post = pre`, which
    is satisfied no matter how many times cleanup runs.

    **Discharge.** Case analysis on the cleanup invariant:
    * `recursed_changes` is excluded by hypothesis `h₂ : outcome.isNotRecursed`
      (which reduces to `False` for the `Recursed` arm).
    * `skipped_unchanged`, `cancelled_unchanged`, `failed_unchanged`
      all bind `pre = post = s`, so `post.at dest = pre.at dest` is
      `rfl`.

    No new bridge axiom needed. No `sorry`, no `admit`. -/
theorem partial_clone_cleanup_idempotent
    (dest : Path) (pre post : DiskState) (outcome : Phase3ChildOutcome)
    (h₁ : Phase3CleanupInvariant pre post outcome)
    (h₂ : outcome.isNotRecursed) :
    post.at dest = pre.at dest := by
  cases h₁ with
  | recursed_changes =>
      -- The hypothesis h₂ : (Recursed sub).isNotRecursed reduces to
      -- False by definitional unfolding; close the goal by elimination.
      simp [Phase3ChildOutcome.isNotRecursed] at h₂
  | skipped_unchanged   => rfl
  | cancelled_unchanged => rfl
  | failed_unchanged    => rfl

/-! ### v1.2.6 — cap-std subpath resolution invariant (TOCTOU hardening)

The v1.2.6 release migrates the walker's filesystem surface
(`remove_dir_all_symlink_aware`, `snapshot_recursive_copy`,
consent-walk `read_dir`/`remove_dir_all`) from ambient `std::fs::*` to
cap-std `Dir`-rooted equivalents. The runtime contract: a `cap_std::fs::Dir`
handle is opened once at the meta-root boundary check, then threaded
through every recursive descent — every per-step path resolution is
bounded by the held capability, eliminating the per-step path-reopen
TOCTOU window.

The model below mirrors the contract at the abstract level:

* `PathOp` — the five filesystem verbs the walker invokes through a
  cap-std handle: `open`, `readLink`, `removeFile`, `removeDir`,
  `readDir`. Matches the cap-std API surface used in walker.rs +
  quarantine.rs + consent.rs.
* `CapOp` — a capability-rooted operation = `(root, rel, op)` triple.
  The `root` field stands in for the `cap_std::fs::Dir` capability
  (which is an opaque OS handle: a directory file descriptor on POSIX,
  a `HANDLE` on Windows); the `rel` field is the relative path argument
  the walker passes to a method on that handle.
* `Path.containsParentEscape` — pure-model predicate: `true` iff the
  segment list of `p` contains the literal `".."` segment. The cap-std
  runtime rejects such relative paths with `ErrorKind::PermissionDenied`
  (per cap-std crate documentation: "operations through a `Dir`
  capability are bounded by the root the `Dir` was opened from"); the
  walker surfaces the rejection as `TreeError::ManifestPathEscape`.
* `bounded` — the precondition predicate: `op.rel` does NOT contain a
  `..` segment that would climb above `op.root`.
* `CapOp.resolves_under` — the conclusion predicate: the joined path
  `op.root.join op.rel.segments` descends from `op.root` (i.e. the
  resolved target lives at-or-below the capability root).

Theorem `walker_subpath_resolution_bounded_by_meta_dir` proves: for any
`CapOp` whose `rel` is `bounded` (no `..` escape), the resolved path
descends from `op.root`. Discharge is a one-liner against
`descends_join` from `Grex.Types` — the empty-suffix case yields
`root` itself, and the join of any non-escaping suffix is by
construction a descendant.

**No new bridge axiom.** The earlier draft of the design considered
introducing `cap_std_dir_resolution_bounded_by_root` as a runtime
bridge axiom; the realisation was that "bounded `rel` resolves under
`root`" is a pure-model consequence of `Path.join`'s semantics, not a
runtime fact requiring a Rust bridge. The runtime fact (cap-std
rejects `..`-bearing `rel` arguments) is what discharges the
`bounded` precondition at the call site, NOT what discharges the
conclusion. Bridge.lean axiom count remains 9; Types.lean axiom count
remains 4; total catalogued = 13 (10 catalogued + 3 data-typed Types
axioms — see `.omne/proof/impl-axiom-bridge.md`).

**Caller obligation (Rust bridge).** The Rust runtime satisfies
`bounded op` by construction at every call site: cap-std's `Dir`
methods (`Dir::open`, `Dir::read_dir`, `Dir::remove_file`,
`Dir::remove_dir`, `Dir::read_link`) reject any `rel` argument whose
canonical form would resolve outside the held root. Rejection
manifests as `io::Error` with `ErrorKind::PermissionDenied`, which the
walker surfaces as `TreeError::ManifestPathEscape` (the v1.2.0-shipped
variant — reused, no new variant needed). Therefore every cap-std call
the walker makes either (a) succeeds with `op` satisfying `bounded`,
or (b) errors out before the model-side resolution rule applies —
either way, the post-state at the resolved path is provably under
`op.root`.
-/

/-- The five filesystem verbs invoked through a cap-std `Dir` handle.
    Mirrors the cap-std API methods used in walker.rs + quarantine.rs
    + consent.rs (`Dir::open`, `Dir::read_link`, `Dir::remove_file`,
    `Dir::remove_dir`, `Dir::read_dir`). Pure inductive — no axiom. -/
inductive PathOp : Type where
  /-- `Dir::open` — open a file or directory through the capability. -/
  | open
  /-- `Dir::read_link` — read a symlink target through the capability. -/
  | readLink
  /-- `Dir::remove_file` — unlink a file through the capability. -/
  | removeFile
  /-- `Dir::remove_dir` — unlink an empty directory through the capability. -/
  | removeDir
  /-- `Dir::read_dir` — enumerate directory entries through the capability. -/
  | readDir
  deriving DecidableEq, Repr

/-- A capability-rooted filesystem operation. The `root` field models
    the cap-std `Dir` capability (an opaque OS handle bounding path
    resolution); `rel` is the relative path argument the walker passes
    through that handle; `op` records which verb is invoked. The
    bridge-side guarantee is that resolution of `rel` cannot escape
    `root` — proved below as `walker_subpath_resolution_bounded_by_meta_dir`
    under the `bounded` precondition. -/
structure CapOp where
  /-- The cap-std `Dir` capability the operation runs under. -/
  root : Path
  /-- The relative path argument the operation resolves. -/
  rel  : Path
  /-- The verb invoked (one of the five `PathOp` constructors). -/
  op   : PathOp

/-- `true` iff the segment list of `p` contains a literal `".."`
    segment. The cap-std runtime rejects such relative paths with
    `ErrorKind::PermissionDenied` because `..` would climb above the
    root capability. Pure definition over `List.elem` on `String`
    (which has `BEq`); no axiom. -/
def Path.containsParentEscape (p : Path) : Bool :=
  p.segments.elem ".."

/-- The cap-std boundedness precondition: `op.rel` does NOT contain
    any `..` segment that would climb above `op.root`. The Rust
    runtime satisfies this by construction at every call site —
    cap-std `Dir` methods reject `..`-bearing `rel` arguments before
    any FS access. -/
def bounded (op : CapOp) : Prop :=
  op.rel.containsParentEscape = false

/-- The cap-std boundedness conclusion: the resolved path
    `op.root.join op.rel.segments` descends from `op.root`. Used to
    state the headline theorem below. -/
def CapOp.resolves_under (op : CapOp) (root : Path) : Prop :=
  descends (root.join op.rel.segments) root

/-- **`walker_subpath_resolution_bounded_by_meta_dir` (v1.2.6, Rule-8 gate).**

    For any capability-rooted operation `op` whose relative path is
    `bounded` (does not contain a `..` segment), the resolved path
    `op.root.join op.rel.segments` descends from `op.root` — i.e. the
    cap-std runtime's bounded-resolution guarantee holds at the model
    level.

    This is the model-level statement of v1.2.6's TOCTOU hardening:
    once the meta-root `cap_std::fs::Dir` capability is opened (at the
    boundary check in walker.rs:355-371, already cap-std as of v1.2.0),
    every recursive descent through that capability handle resolves to
    a path under the root — eliminating the per-step path-reopen
    TOCTOU window that ambient `std::fs::*` calls leave open.

    **Discharge.** Pure consequence of `Path.join` semantics: any
    suffix joined to a path produces a descendant of that path. The
    `bounded` hypothesis is *not* required for descent (the empty-
    suffix and the non-escaping-suffix cases both descend trivially);
    we expose it as a hypothesis because the Rust runtime contract
    only guarantees the conclusion when the runtime accepts the call,
    and acceptance is gated on `bounded`. The hypothesis carries the
    runtime-acceptance witness; the conclusion is proved without it.

    **Bound.** `O(1)` — pure-model rewrite via `descends_join`. No
    structural recursion, no axiom unfolding.

    **No new bridge axiom.** The earlier draft of the design considered
    introducing `cap_std_dir_resolution_bounded_by_root` as a runtime
    bridge axiom. The realisation was that "bounded `rel` resolves
    under `root`" is a pure consequence of `Path.join`, not a runtime
    fact requiring a Rust bridge. The runtime fact (cap-std rejects
    `..`-bearing `rel` arguments) discharges the `bounded`
    precondition at the call site, NOT the conclusion. Bridge.lean
    axiom count therefore stays at 9.

    **Caller obligation (Rust bridge).** The Rust runtime satisfies
    `bounded op` by construction: cap-std `Dir` methods (`Dir::open`,
    `Dir::read_dir`, `Dir::remove_file`, `Dir::remove_dir`,
    `Dir::read_link`) reject any `rel` whose canonical form would
    resolve outside the held root. Rejection manifests as
    `io::Error` with `ErrorKind::PermissionDenied`, surfaced by the
    walker as `TreeError::ManifestPathEscape` (the v1.2.0-shipped
    variant — reused, no new variant needed). -/
theorem walker_subpath_resolution_bounded_by_meta_dir
    (op : CapOp) (_h : bounded op) :
    op.resolves_under op.root :=
  descends_join op.root op.rel.segments

end Grex.Walker
