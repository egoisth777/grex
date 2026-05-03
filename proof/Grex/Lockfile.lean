import Grex.Types

/-!
# `Grex.Lockfile` — model + theorems for the v1.3.1 lockfile branch fix

This module formalizes the v1.3.1 fix for **B14** (dogfood finding,
v1.3.0): `grex sync` writes lockfile entries with `branch: ""` for
every child despite the parent manifest specifying `ref: <name>` on
each `ChildRef`. The Rust patch in
`crates/grex-core/src/lockfile/writer.rs` carries
`child.manifest_ref` into the new `LockEntry.branch` field. This
module proves — at the model level — that any pure writer obeying
the same data-flow contract emits a `branch` value equal to the
child's manifest `ref`.

## Why a fresh `LockEntry` model here

`Grex.Types` already declares a `LockEntry` (with fields `segments`
and `revision`) used by the v1.2.0 walker invariants (W2 lockfile
partition). That existing record does **not** carry a `branch`
field — it predates the v1.3.0 lockfile schema bump that introduced
the explicit `branch` slot. Per CLAUDE.md rule 5 (surgical changes)
and the Rule-8 gate's "no Types.lean edits in worker scope", we
introduce a fresh `LockEntryV131` record and a v1.3.1-specific
writer here, **without** touching `Grex.Types`. The v1.3.1
LockEntry model can be related to the existing `Grex.Walker.LockEntry`
in a future bridge if a follow-up theorem needs it; for the B14 fix
the standalone record suffices.

## What the theorem says

`lockfile_branch_mirrors_manifest_ref` — for every child reference
in a manifest, the lockfile entry produced by `write_lockfile` has
`branch` equal to the child's `ChildRef.ref` (lifted out of the
`Option String` via the same default-empty rule the Rust impl uses:
`None` ↦ `""`, `Some s` ↦ `s`). This is a **pure data-transform
fidelity** statement — no acyclicity, no concurrency, no runtime
hypothesis required.

## Source-of-truth links

* `.omne/cfg/lockfile.md` — primary spec for the lockfile schema
* `.omne/cfg/dogfood-findings-v1.3.0.md` §B14 — bug report
* `crates/grex-core/src/lockfile/writer.rs` — Rust v1.3.1 implementation

## Axiom budget (CI gate)

Zero new axioms. The model-level `write_entry` / `write_lockfile`
are pure `def`s; the theorem discharges by structural reflexivity
plus `simp` over `List.map` membership. CI axiom audit must show
`[propext]` only or empty.
-/

namespace Grex.Lockfile

open Grex.Walker  -- for `ChildRef`

/-! ## v1.3.1 lockfile entry model -/

/-- Model-level lockfile entry for v1.3.1. Pure data:
    `id` (= folder/repo identifier — child path segments joined by `/`),
    `url` (clone source carried verbatim from `ChildRef.url`),
    `branch` (= manifest ref, empty string when the manifest omits it).

    Distinct from `Grex.Walker.LockEntry` (which carries `segments` +
    `revision`) — this record models the v1.3.0+ schema slot that
    the B14 bug left blank. A future bridge between the two records
    is possible but not required by B14. -/
structure LockEntryV131 where
  id     : String
  url    : String
  branch : String
deriving DecidableEq, Repr

/-- Model-level lockfile for v1.3.1: ordered list of entries. -/
structure LockfileV131 where
  entries : List LockEntryV131
deriving Repr

/-! ## v1.3.1 writer model -/

/-- Lift `ChildRef.ref : Option String` into the `String` field of
    `LockEntryV131.branch`. Mirrors the Rust impl's
    `child.manifest_ref.unwrap_or_default()` — `None` ↦ `""`,
    `Some s` ↦ `s`. Defining this as a top-level `def` (rather than
    inlining `Option.getD r ""`) makes the theorem statement read
    as a literal mirror of the Rust line. -/
def branchOf (r : Option String) : String :=
  r.getD ""

/-- Build the lockfile-entry id for a child. The Rust impl uses the
    parent-relative path joined by `/`; for the model we capture the
    same structure: `String.intercalate "/" c.segments`. The exact
    joiner is irrelevant to the B14 contract (which is about `branch`),
    so we keep this as a single-line projection. -/
def idOf (c : ChildRef) : String :=
  String.intercalate "/" c.segments

/-- Model-level lockfile writer for a single child.

    Carries the `ChildRef.ref` value into `LockEntryV131.branch` —
    exactly the contract the v1.3.0 impl violated by emitting `""`
    unconditionally. The v1.3.1 Rust patch in
    `crates/grex-core/src/lockfile/writer.rs` matches this model
    line-for-line:

    ```rust
    LockEntry {
        id: child.path_str(),
        url: child.url.clone(),
        branch: child.manifest_ref.clone().unwrap_or_default(),
    }
    ```

    No new bridge axiom is required: data-transform fidelity falls
    under the existing `lockfile_partition` bridge in
    `Grex.Bridge` (axiom #5), which witnesses that the Rust writer
    realises whatever pure model the Lean side declares. -/
def write_entry (c : ChildRef) : LockEntryV131 :=
  { id     := idOf c,
    url    := c.url,
    branch := branchOf c.ref }

/-- Model-level lockfile writer for a whole list of children
    (typically `Manifest.children`). Maps `write_entry` over the list
    and packages the result. -/
def write_lockfile (children : List ChildRef) : LockfileV131 :=
  { entries := children.map write_entry }

/-! ## Main theorem (v1.3.1, B14, Rule-8 gate) -/

/-- **`write_entry_branch` — single-child fidelity.**

    The branch field of `write_entry c` equals `branchOf c.ref` by
    definitional unfolding. Stated as a standalone lemma so the main
    theorem can rewrite with it. -/
theorem write_entry_branch (c : ChildRef) :
    (write_entry c).branch = branchOf c.ref := by
  rfl

/-- **`write_entry_id` — single-child id fidelity.** Symmetric helper
    used by the main theorem; established by `rfl`. -/
theorem write_entry_id (c : ChildRef) :
    (write_entry c).id = idOf c := by
  rfl

/-- Helper: `write_entry c ∈ children.map write_entry` whenever
    `c ∈ children`. Direct list-induction proof — avoids
    `List.mem_map_of_mem` from the standard library (which transitively
    depends on `Quot.sound`) so the main theorem's axiom audit stays
    at `[propext]` only per the CI gate. -/
private theorem mem_map_write_entry
    (children : List ChildRef) (c : ChildRef) (hc : c ∈ children) :
    write_entry c ∈ children.map write_entry := by
  induction children with
  | nil => cases hc
  | cons head tail ih =>
    rcases List.mem_cons.mp hc with heq | hmem
    · -- c = head: write_entry c is the head of the mapped list.
      rw [heq]
      exact List.mem_cons_self (write_entry head) (tail.map write_entry)
    · -- c ∈ tail: recurse and lift via cons.
      exact List.mem_cons_of_mem (write_entry head) (ih hmem)

/-- **`lockfile_branch_mirrors_manifest_ref` (v1.3.1 B14, Rule-8 gate).**

    For every child reference in any manifest's child list, the
    lockfile produced by `write_lockfile` contains an entry whose
    `id` equals `idOf c` and whose `branch` equals `branchOf c.ref`.
    This formalizes the data-flow contract violated by v1.3.0 (which
    emitted `branch: ""` for every entry) and restored by v1.3.1
    (which threads `child.manifest_ref` through the writer).

    Pure data-transform property: discharged by the helper
    `mem_map_write_entry` (direct list induction) plus the two
    single-child fidelity lemmas above. No acyclicity, no
    concurrency, no runtime hypothesis. The Rust bridge contract is
    documented inline at `write_entry`; no new axiom required. -/
theorem lockfile_branch_mirrors_manifest_ref
    (children : List ChildRef) :
    ∀ c ∈ children,
      ∃ e ∈ (write_lockfile children).entries,
        e.id = idOf c ∧ e.branch = branchOf c.ref := by
  intro c hc
  refine ⟨write_entry c, ?_, ?_, ?_⟩
  · -- membership in the mapped list, proved by direct induction in
    -- `mem_map_write_entry` to keep the axiom audit at [propext] only.
    show write_entry c ∈ (write_lockfile children).entries
    exact mem_map_write_entry children c hc
  · exact write_entry_id c
  · exact write_entry_branch c

/-- **Corollary: every entry in the writer's output has the
    fidelity invariant on its source child.** A useful "for-all"
    flip of the existential theorem above; lets downstream tests
    (or the Rust unit-test parity in `lockfile/writer.rs`) walk
    the output list and check each entry's `branch` against the
    originating `ChildRef`. -/
theorem write_lockfile_entries_mirror
    (children : List ChildRef) :
    ∀ e ∈ (write_lockfile children).entries,
      ∃ c ∈ children, e.id = idOf c ∧ e.branch = branchOf c.ref := by
  -- Direct list-induction to avoid `simp`/`simpa` (which pulls in
  -- `Quot.sound`) — keeps axiom audit at `[propext]` only per CI gate.
  intro e he
  -- (write_lockfile children).entries reduces definitionally to
  -- children.map write_entry, so `he` is already a proof of
  -- `e ∈ children.map write_entry`. We induct on `children`.
  have he' : e ∈ children.map write_entry := he
  clear he
  induction children with
  | nil => cases he'
  | cons head tail ih =>
    -- e ∈ write_entry head :: tail.map write_entry
    rcases List.mem_cons.mp he' with heq | hmem
    · -- e = write_entry head — pick c := head.
      refine ⟨head, List.mem_cons_self head tail, ?_, ?_⟩
      · rw [heq]; exact write_entry_id head
      · rw [heq]; exact write_entry_branch head
    · -- e ∈ tail.map write_entry — recurse via ih.
      rcases ih hmem with ⟨c, hc, hid, hbr⟩
      exact ⟨c, List.mem_cons_of_mem head hc, hid, hbr⟩

end Grex.Lockfile
