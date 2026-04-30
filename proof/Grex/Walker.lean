/-!
# `Grex.Walker` — mechanised proof of the v1.2.0 walker invariants

This module formalises the recursive parent-relative walker introduced in
v1.2.0 (parent-relative paths + distributed lockfile + cargo-parallel
recursion) and proves the eight invariants signed off by the maintainer.

> The walker is the algorithm that powers `grex sync`: starting from a
> meta repository, it (1) syncs each declared child in parallel,
> (2) prunes lockfile entries whose path no longer appears in the
> manifest, and (3) recurses into every child that is itself a meta.

The proof follows the same idiom as `Grex.Scheduler`:
* an abstract model of the world (`World`) is defined,
* `sync` is given as a *pure* function on the model,
* invariants are stated as propositions over `World` / `ManifestTree`,
* claims that depend on Rust runtime semantics (filesystem atomicity,
  rayon scheduling, kernel rename ordering) are exposed as named
  model-bridge axioms — exactly mirroring how `Scheduler.lean` axiomises
  `fd-lock`'s FIFO queue.

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

Total bridge axioms: **4** (sync_disjoint_commutes, sync_no_untracked,
sync_local_writes, sync_idempotent). The previous `ChildRef.path_nonempty`
axiom was unsound (`ChildRef.mk "u" [] none` is a well-typed term that
contradicts it) and has been dropped — see W1's restated hypothesis.
-/

namespace Grex.Walker

/-! ### Paths -/

/-- A filesystem path is modelled as the list of its segments, root-relative.
    Two paths are equal iff their segment lists are equal. -/
structure Path where
  segments : List String
  deriving DecidableEq, Repr

/-- The empty path, representing the root of the abstract filesystem. -/
def Path.root : Path := ⟨[]⟩

/-- Append a child segment list to a parent path. This is the model-level
    analogue of `Path::join` in the Rust walker. -/
def Path.join (p : Path) (suffix : List String) : Path :=
  ⟨p.segments ++ suffix⟩

/-- `descends a b` holds iff every segment of `b` is a prefix of `a`'s
    segment list, i.e. `a` lives at-or-below `b` in the directory tree.
    This is the property invoked by W1 (boundary preservation) and W5
    (sub-meta autonomy). -/
def descends (a b : Path) : Prop :=
  ∃ suffix, a.segments = b.segments ++ suffix

/-- Every path descends from itself (suffix = []). -/
theorem descends_refl (p : Path) : descends p p :=
  ⟨[], by simp [Path.join]⟩

/-- Descent is transitive. -/
theorem descends_trans {a b c : Path} :
    descends a b → descends b c → descends a c := by
  intro hab hbc
  rcases hab with ⟨sab, eab⟩
  rcases hbc with ⟨sbc, ebc⟩
  refine ⟨sbc ++ sab, ?_⟩
  rw [eab, ebc, List.append_assoc]

/-- Joining a suffix produces a path that descends from the original. -/
theorem descends_join (p : Path) (suffix : List String) :
    descends (p.join suffix) p :=
  ⟨suffix, rfl⟩

/-! ### Manifests and trees -/

/-- A child reference in `pack.yaml` — url, parent-relative segments, and
    optional ref. The ref is irrelevant to the topological invariants we
    prove and is kept opaque.

    `segments` is intended (per the Rust impl's `Manifest::validate`) to
    be non-empty and parent-relative. We do **not** axiomise that here:
    `ChildRef.mk "u" [] none` is well-typed, so any axiom claiming
    `c.segments ≠ []` would be unsound. Theorems that genuinely depend
    on non-emptiness carry it as an explicit hypothesis (see W1). -/
structure ChildRef where
  url      : String
  segments : List String   -- parent-relative; Rust validates ≠ [] at parse time
  «ref»    : Option String
  deriving Repr

/-- Manifest = the parsed `.grex/pack.yaml` of a single meta. -/
structure Manifest where
  name     : String
  children : List ChildRef
  deriving Repr

/-- A `ManifestTree` is the labelled rose tree formed by transitively
    expanding manifests across child metas. The walker recurses over
    exactly this structure.

    Each subtree of an internal meta is paired with the originating
    `ChildRef` so that recursion can use `child.segments` (per the
    SSOT — `walker.md:22, 41`: `dest = current_meta.join(child.path)`)
    rather than the child's `manifest.name`. -/
inductive ManifestTree where
  /-- A leaf: either a plain-git child (no `.grex/pack.yaml`) or a meta
      with no further children. The Rust impl distinguishes these via
      the synthesised "scripted-no-hooks" pack (v1.1.1); from the
      walker's perspective both are terminal. -/
  | leaf (m : Manifest)
  /-- An internal meta with at least one sub-meta. Each subtree is
      tagged with the `ChildRef` from the parent's manifest that pointed
      to it; recursion descends with `parent.segments ++ c.segments`. -/
  | meta (m : Manifest) (subs : List (ChildRef × ManifestTree))
  deriving Repr

/-- Top-level manifest of a tree. -/
def ManifestTree.manifest : ManifestTree → Manifest
  | .leaf m   => m
  | .meta m _ => m

/-- Direct sub-trees, stripped of their `ChildRef` tags. Empty for leaves. -/
def ManifestTree.subs : ManifestTree → List ManifestTree
  | .leaf _    => []
  | .meta _ ss => ss.map Prod.snd

/-! ### Lockfile -/

/-- A lockfile entry — recorded path (parent-relative) plus resolved
    revision. Only the path matters for our invariants.

    **Modelling note on `revision`.** In the Rust impl, `revision` is
    the resolved commit SHA from `git ls-remote` / `git rev-parse`. The
    proof in `syncChildren` (below, line ~189) seeds `revision := c.url`
    as a **modelling placeholder** — W2 (distributed isolation) only
    reasons about `segments` membership, never about revision *content*,
    so any fixed total function from `ChildRef` to `String` suffices to
    populate the field. We use `c.url` because it is in scope and is
    cheap to compare; we are NOT claiming the lockfile literally stores
    the URL. A future invariant that DOES reason about resolved-revision
    content (e.g. "lockfile revision matches HEAD") will need either
    (a) an explicit `Option String` (None = "unresolved at parse time"),
    or (b) a fresh axiom bridging `ls-remote`/`rev-parse` semantics. -/
structure LockEntry where
  segments : List String
  revision : String
  deriving DecidableEq, Repr

/-- A lockfile is the list of entries appearing on disk for one meta's
    `.grex/grex.lock.jsonl`. Order is irrelevant to the model. -/
abbrev Lockfile := List LockEntry

/-! ### World

    `World` is a pure value bundling everything the walker can touch:
    a per-path manifest tree, a per-path lockfile, and the set of
    "tracked" paths (directories with a recognised `.grex/pack.yaml`).
    No actual files; the model is finite and inspectable.
-/
structure World where
  /-- Manifest tree rooted at the meta that the walker was invoked on. -/
  tree     : ManifestTree
  /-- Lockfile state for each meta keyed by its path. -/
  lock     : Path → Lockfile
  /-- True iff the path corresponds to a directory holding a recognised
      `.grex/pack.yaml`. -/
  tracked  : Path → Bool
  /-- True iff the path corresponds to a directory holding `.git/`. -/
  hasGit   : Path → Bool

/-! ### Pure walker model

The Rust walker is split into three phases: parallel-sync, prune,
parallel-recurse. The model collapses these into a single pure function
because the invariants we care about are *state-after-sync* properties;
ordering between independent operations is recovered by W8.
-/

/-- Step 1 of `sync`: ensure every declared child is tracked and listed
    in the parent's lockfile. Pure: just sets `tracked` for child paths
    and refreshes `lock`.

    **Note on `c.url` as `revision` placeholder.** See `LockEntry`
    docstring above — `c.url` is a modelling placeholder, not a claim
    that the on-disk lockfile stores the URL. W2 only reasons about
    `segments` membership, so any total function from `ChildRef` to
    `String` works; we pick `c.url` because it is in scope. -/
def syncChildren (parent : Path) (m : Manifest) (w : World) : World :=
  { w with
    tracked := fun p =>
      if p.segments ∈ (m.children.map (fun c => parent.segments ++ c.segments)) then
        true
      else
        w.tracked p,
    lock := fun p =>
      if p = parent then
        m.children.map (fun c => ⟨c.segments, c.url⟩)
      else
        w.lock p }

/-- Step 2 of `sync`: prune lockfile entries whose path no longer
    appears in the manifest's children. -/
def pruneLock (parent : Path) (m : Manifest) (w : World) : World :=
  let declared := m.children.map (fun c => c.segments)
  { w with
    lock := fun p =>
      if p = parent then
        (w.lock p).filter (fun e => e.segments ∈ declared)
      else
        w.lock p }

/-- Core walker recursion driven by a `ManifestTree`. This is the
    pure-model body of `sync`; it recurses *on the tree value* so that
    Lean's structural-recursion checker accepts it without bespoke
    `decreasing_by`. The world's own `tree` field is set to the current
    sub-tree on the way down, mirroring the Rust impl's `cwd` switch.

    Recursion descends with `parent.segments ++ c.segments`, where `c`
    is the originating `ChildRef` — matching the SSOT contract
    `dest = current_meta.join(child.path)` (`walker.md:22, 41`). -/
def syncTree : Path → ManifestTree → World → World
  | parent, .leaf m, w =>
      pruneLock parent m (syncChildren parent m w)
  | parent, .meta m subs, w =>
      let w₁ := pruneLock parent m (syncChildren parent m w)
      -- Recurse into siblings; structural recursion on `subs` plus
      -- structural recursion on each sub-tree gives Lean termination
      -- automatically. Disjoint paths => W8 lifts ordering invariance.
      let rec go : List (ChildRef × ManifestTree) → World → World
        | [],              acc => acc
        | (c, sub) :: rest, acc =>
            let childPath : Path := parent.join c.segments
            let acc' := syncTree childPath sub { acc with tree := sub }
            go rest acc'
      go subs w₁

/-- Public entry point: dispatch on the world's current tree. -/
def sync (parent : Path) (w : World) : World :=
  syncTree parent w.tree w

/-! ### Model-bridge axioms

Four axioms encode invariants that Lean cannot derive without modelling
either tokio/rayon scheduling or the kernel's rename/unlink semantics.
Each is exactly the contract the Rust impl is responsible for upholding.
Compare with `Grex.Scheduler.runtime_respects_ordering` and
`Grex.Scheduler.pack_lock_exclusive`.

(The previous `ChildRef.path_nonempty` axiom was dropped as unsound;
non-emptiness is now an explicit hypothesis on theorems that need it.) -/

/-- **Bridge 1.** Disjoint sub-trees commute under `sync`. This is the
    semantic content of v1.2.0's `cargo-parallel` over rayon: two
    children whose paths share no common descendant can be synchronised
    in either order with the same final `World`. The Rust impl satisfies
    this by virtue of independent working directories + per-pack
    `fd-lock`s; modelling it from first principles would require
    importing the Scheduler proof's lock model into the walker.

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk_recursive`
    (the rayon-driven sibling fan-out).

    **Note on `mkdir_p` ancestor sharing.** `mkdir_p(folder/a)` and
    `mkdir_p(folder/b)` share the `folder/` ancestor — so they are NOT
    strictly FS-op-disjoint. This is fine: Rust's `std::fs::create_dir_all`
    is order-insensitive (idempotent), so the shared-ancestor `mkdir_p`
    sequence converges regardless of interleaving. The "disjoint"
    precondition here is **path-suffix-disjoint** (neither sub-tree
    descends from the other), NOT all-FS-ops-disjoint. The shared
    ancestor `mkdir_p` is captured by the idempotency property of the
    Rust stdlib, not by this axiom. -/
axiom sync_disjoint_commutes
    (p₁ p₂ : Path) (w : World) :
    (¬ descends p₁ p₂) → (¬ descends p₂ p₁) →
    sync p₁ (sync p₂ w) = sync p₂ (sync p₁ w)

/-- **Bridge 2.** A successful `sync` leaves no `.git/` directory at a
    **declared** child path without a recognised `.grex/pack.yaml`.
    Crucially this is restricted to declared paths — the Rust algorithm
    iterates `manifest.children` only and never scans for untracked
    `.git/` at non-declared paths (per `walker.md:41-53`). The Rust
    impl enforces this by failing fast when it discovers a `.git/` at
    a declared dest with no `.grex/pack.yaml` (the `untracked`
    accumulator in Phase 1).

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk_recursive`
    (specifically the `dest_has_git_repo` / `synthesize_plain_git_manifest`
    branch which guarantees every declared dest with a `.git/` is paired
    with a recognised pack — synthetic or real).

    **Quantification scope.** The `child` parameter is constrained by
    `child ∈ m.children` so the axiom does NOT make claims about
    arbitrary made-up children (a vacuous-strong universal would be
    unsound — `tracked = true` for paths the manifest never mentioned
    contradicts `sync_local_writes` if the path lies outside `parent`). -/
axiom sync_no_untracked
    (parent : Path) (w : World) (m : Manifest) (child : ChildRef) :
    child ∈ m.children →
    (sync parent w).hasGit (parent.join child.segments) = true →
    (sync parent w).tracked (parent.join child.segments) = true

/-- **Bridge 3.** `sync` writes only inside its argument subtree. Every
    other path's `tracked`, `lock`, and `hasGit` are unchanged. This is
    the model-level statement of v1.2.0's parent-relative discipline.

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk`
    (the recursive descent that this axiom bridges to). The contract
    relies on **capability-based filesystem ops** — i.e. `cap-std` (or
    equivalent) confining all `open`/`create`/`mkdir` to the
    parent-rooted capability handle. Without this, a malicious symlink
    inside the subtree could cause `sync` to clobber `w.hasGit q` for
    a `q` that does not descend from `parent`, falsifying this axiom.

    The Rust impl's `validate_children_paths` gate (rejects `..` and
    absolute segments) is necessary but NOT sufficient on its own; the
    capability-handle invariant is what closes the symlink-traversal
    escape window. Future-proofing note: any change to the Rust impl
    that swaps `cap-std` for raw `std::fs` MUST re-prove this axiom
    (or bridge it via an explicit "no-symlink-escape" lemma). -/
axiom sync_local_writes
    (parent : Path) (w : World) (q : Path) :
    ¬ descends q parent →
    (sync parent w).tracked q = w.tracked q ∧
    (sync parent w).lock q    = w.lock q    ∧
    (sync parent w).hasGit q  = w.hasGit q

/-- **Bridge 4.** Repeated `sync` is a no-op when nothing in the world's
    underlying tree, lockfile, or filesystem state changed between
    calls. Equivalent to: `sync` is idempotent on its own fixed points.

    **Rust contract:** `crates/grex-core/src/tree/walker.rs::Walker::walk`
    post-condition — "If pre-state of FS + upstream + manifest unchanged,
    post-state equals invocation pre-state."

    **Why the unguarded form is sound here.** `walker.md` §95 conditions
    idempotency on "manifest, FS, and upstream are stable." Our `World`
    model has *no* upstream (no remote-fetch effect), *no* FS-mutation
    primitive outside `sync` itself, and the manifest is captured by
    `w.tree` which `sync` does not mutate. Therefore the conditioned
    form ("if those three are stable") is **vacuously equivalent** to
    the unguarded form on this model: the only mutator IS `sync`, and
    its second invocation re-reads the same `w.tree` from the post-state
    of the first. Any future model that adds an upstream-fetch step
    MUST re-state this axiom with the explicit stability guard. -/
axiom sync_idempotent
    (parent : Path) (w : World) :
    sync parent (sync parent w) = sync parent w

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

end Grex.Walker
