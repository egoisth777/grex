/-!
# `Grex.Types` — shared type and pure-model definitions

This module hosts the type and pure-function definitions shared between
`Grex.Walker`, `Grex.Scheduler`, and `Grex.Bridge`. It exists so that the
six model-bridge axioms collected in `Grex.Bridge` can reference the
walker/scheduler models WITHOUT introducing an import cycle:

```
                  Grex.Types
                 /     |    \
                /      |     \
          Walker   Scheduler  Bridge
                \      |     /
                 \     |    /
                   Grex (root)
```

Walker.lean, Scheduler.lean, and Bridge.lean all import this module, but
Walker/Scheduler do NOT import Bridge directly (instead, the theorems that
need bridge axioms are stated in Walker.lean / Scheduler.lean and discharge
via `import Grex.Bridge` from the same files — possible because Bridge
itself only imports Types).

NOTE: when editing, keep type-level definitions and pure model functions
here. Theorems and axioms belong in their topic files (Walker.lean,
Scheduler.lean, Bridge.lean).

Source-of-truth links:
* `.omne/cfg/walker.md` — primary spec for v1.2.0 walker
* `.omne/cfg/concurrency.md` §Lean4 invariant — scheduler model
-/

namespace Grex

/-! ## Walker model -/

namespace Walker

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

end Walker

/-! ## Scheduler model -/

namespace Scheduler

/-- Abstract lock identity across the 5 tiers used by the scheduler.
    Concrete filesystem paths and semaphore slot indices are abstracted to
    `String` / `Nat` so the model remains tractable. -/
inductive Lock where
  | workspaceSync
  | semaphoreSlot   (slot : Nat)
  | pack            (path : String)
  | repoBackend     (dest : String)
  | manifest
  deriving Repr

/-- Strict total order on lock tiers, enforcing the fixed 5-tier
    acquisition rule from `.omne/cfg/concurrency.md`:
    workspace-sync → semaphore → pack-lock → repo-backend → manifest. -/
def tier : Lock → Nat
  | .workspaceSync   => 0
  | .semaphoreSlot _ => 1
  | .pack _          => 2
  | .repoBackend _   => 3
  | .manifest        => 4

/-- A task is characterised by the ordered sequence of locks it currently
    holds. Acquisition appends; release pops (LIFO). -/
structure Task where
  id   : Nat
  held : List Lock
  deriving Repr

/-- A schedule is a finite set of tasks observed at one logical instant. -/
abbrev Schedule := List Task

/-- Acquisition is legal only when the new lock's tier strictly exceeds
    every tier currently held. This is the Lean analogue of the
    outer-to-inner lock-ordering rule enforced by the Rust scheduler. -/
def legalAcquire (t : Task) (ℓ : Lock) : Prop :=
  ∀ ℓ' ∈ t.held, tier ℓ' < tier ℓ

/-- A logical time window, with a well-formedness proof that `started`
    strictly precedes `ended`. -/
structure TimeWindow where
  started : Nat
  ended   : Nat
  wf      : started < ended

/-- Two windows overlap iff each starts before the other ends. -/
def overlaps (a b : TimeWindow) : Prop :=
  a.started < b.ended ∧ b.started < a.ended

end Scheduler

end Grex
