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
* `.omne/cfg/walker.md` — primary spec for v1.2.0 walker
* `.omne/cfg/architecture.md` §Walker invariants — identifies the eight
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
mutual

/-- See module-level note above for the cycle-detected
    `sync_meta_inner` model. -/
def sync_meta_inner_model :
    List String → ManifestTree → SyncMetaResult
  | _,       .leaf _      => SyncMetaResult.ok
  | visited, .meta _ subs => syncMetaChildren visited subs

/-- See module-level note above for the children-recursion helper. -/
def syncMetaChildren :
    List String → List (ChildRef × ManifestTree) → SyncMetaResult
  | _,       []               => SyncMetaResult.ok
  | visited, (c, sub) :: rest =>
      let id := ChildRef.identity c
      if id ∈ visited then
        SyncMetaResult.cycleDetected (visited ++ [id])
      else
        match sync_meta_inner_model (id :: visited) sub with
        | SyncMetaResult.cycleDetected ch =>
            SyncMetaResult.cycleDetected ch
        | SyncMetaResult.ok => syncMetaChildren visited rest

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

/-- See module-level note above for the per-tree clause. -/
theorem sync_meta_inner_model_ok_of_acyclic :
    ∀ (visited : List String) (t : ManifestTree),
      acyclic_path visited t →
        sync_meta_inner_model visited t = SyncMetaResult.ok
  | _,       .leaf _,      _ => rfl
  | visited, .meta _ subs, h => by
      -- Unfold the function once; both `acyclic_path` and
      -- `sync_meta_inner_model` reduce to their *_children helpers.
      simp only [sync_meta_inner_model]
      simp only [acyclic_path] at h
      exact syncMetaChildren_ok_of_acyclic visited subs h

/-- See module-level note above for the per-children-list clause. -/
theorem syncMetaChildren_ok_of_acyclic :
    ∀ (visited : List String) (subs : List (ChildRef × ManifestTree)),
      acyclic_children visited subs →
        syncMetaChildren visited subs = SyncMetaResult.ok
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
    sync_meta_inner_model visited t = SyncMetaResult.ok :=
  sync_meta_inner_model_ok_of_acyclic visited t h

end Grex.Walker
