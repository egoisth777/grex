import Grex.Types
import Grex.Bridge

/-!
# `Grex.Quarantine` — `--quarantine` snapshot-before-delete safety (v1.2.1 Item 5a)

This module hosts the v1.2.1 safety theorem for the `--quarantine` flag
on `--force-prune` (sub-feature 5 of `openspec/feat-v1.2.1/spec.md`):

> **Quarantine ordering invariant.** When `--quarantine` is set, an
> audit-log entry is appended + fsynced AND the dest's subtree is
> recursively copied to `<meta>/.grex/trash/<ts>/<basename>/` BEFORE any
> `unlink` syscall on the original `dest`. If either step fails, the
> `unlink` does NOT fire — the original `dest` remains intact.

The theorem `quarantine_snapshot_precedes_delete` proves this is a
pure consequence of how the pipeline is structured: `delete_licensed`
holds iff the snapshot returned `SnapResult.ok` *and* the audit-log
entry is present in the post-pipeline log. The Rust trust contract
binding the model to the actual `unlink` syscall in
`crates/grex-core/src/tree/quarantine.rs::snapshot_then_rm` (forward
reference — added by v1.2.1 Item 5b, blocked on this proof) is
documented in this module's prose at the bottom — see "Rust-side trust
contract" — but is NOT axiomatised in Lean (the model-side iff is
proved here; the Rust impl earns the safety guarantee by faithfully
realising the pipeline).

Source-of-truth links:
* `openspec/feat-v1.2.1/spec.md` §5 — quarantine layout + ordering spec
* `inst/walker.md` — Phase 2 prune contract (force-prune + quarantine)
* `inst/concurrency.md` §Per-pack `PackLock` — quarantine runs under
  the per-pack lock (so single-writer to `.grex/events.jsonl`)

## Axioms introduced (v1.2.1 Item 5a)

ONE new data-typed model axiom is added by v1.2.1 Item 5a, declared
in `Grex.Types` (per the CI axiom-location policy which restricts
`axiom`-keyword declarations to `Grex.Bridge` and `Grex.Types`):

* `snapshot_recursive` *(data axiom — model placeholder, in `Grex.Types`)*
  — pure-model reference to the cap-std recursive copy + audit-fsync
  primitive. Returns `.ok` iff the entire subtree at `src` was copied
  to `trash` AND the audit-log entry was fsynced first.

Declared `axiom` (rather than `opaque`) because `SnapResult` lacks an
`Inhabited` default we want to commit to (defaulting to `.ok` would
mislead readers about the pipeline's outcome). The audit-log commit
semantics are encoded in the *definition* of `AuditLog.commit` (a
list-snoc, also in `Grex.Types`) so no separate axiom is needed —
Lean's `simp` + `List` lemmas discharge entry-membership facts
directly. The data types `AuditKind`, `AuditEntry`, `AuditLog`, and
`SnapResult` likewise live in `Grex.Types` alongside the axiom; this
module hosts only the pipeline definition and proofs that consume
them.

The Rust trust contract for `--quarantine` (Item 5b's
`crates/grex-core/src/tree/quarantine.rs::snapshot_then_rm`) is
documented in this module's prose but NOT axiomatised here — the safety
theorem is a pure consequence of the pipeline definition + the snapshot
primitive's contract. The Rust impl earns the safety guarantee by
faithfully realising the pipeline (audit-fsync → snapshot → unlink iff
both succeed); reviewers verify this faithfulness against
`inst/proof/impl-axiom-bridge.md` (separate SSOT-repo commit per
Rule 7).
-/

namespace Grex.Walker

/-! ## Quarantine model

The data types (`AuditKind`, `AuditEntry`, `AuditLog`, `SnapResult`),
the `AuditLog.commit` / `AuditLog.contains` definitions, and the
`snapshot_recursive` data-axiom all live in `Grex.Types` (per the CI
policy on `axiom`-keyword location, which restricts such declarations
to `Grex.Bridge` and `Grex.Types`). This module imports them via
`import Grex.Types` and hosts only the pipeline definition,
licensing predicate, and safety theorem. -/

/-! ### Derived lemmas + pipeline -/

/-- Pure-model lemma: the just-committed entry is in the post-commit
    log. Discharged by list-append semantics — no axiom needed because
    `AuditLog.commit` is a `def` (list snoc), not opaque.

    The doc-comment placement here mirrors how `audit_commit_persists`
    *would* read if it were axiomatised; keeping it as a derived lemma
    keeps the new-axiom count at 1 (only `snapshot_recursive`). -/
theorem audit_commit_contains (a : AuditLog) (e : AuditEntry) :
    e ∈ (a.commit e).entries := by
  simp [AuditLog.commit]

/-! ### Quarantine pipeline -/

/-- The pure-model quarantine pipeline. Runs in the order specified by
    `openspec/feat-v1.2.1/spec.md` §5:

    1. Append + fsync the `QuarantineStart` audit entry FIRST.
    2. Then attempt the recursive snapshot copy.
    3. On success: keep the audit log committed (the `Start` entry
       remains; the Rust impl appends a `QuarantineComplete` entry
       after the `unlink` succeeds, which we don't model here because
       it has no bearing on the delete-licensing decision).
    4. On failure: the `Start` entry remains in the log (it was
       fsynced before the failure was observed); a `QuarantineFailed`
       follow-up entry would be appended by the Rust impl, again not
       relevant to the safety theorem.

    The pipeline returns `(snap_result, audit_log_post)`. The Rust
    impl mirrors this control flow in `snapshot_then_rm` — see the
    "Rust-side trust contract" prose at the bottom of this module. -/
noncomputable def quarantine_pipeline
    (dest trash : Path) (ts : String) (a : AuditLog) :
    SnapResult × AuditLog :=
  let entry : AuditEntry := ⟨AuditKind.QuarantineStart, ts, dest, trash⟩
  let a' := a.commit entry
  -- Snapshot reads the post-commit audit log so it can witness that the
  -- `Start` entry was fsynced before any byte was copied.
  let r := snapshot_recursive dest trash a'
  (r, a')

/-- **`delete_licensed dest trash ts a`** — the pure-model predicate
    licensing the `unlink(dest)` syscall. Holds iff:

    1. The `QuarantineStart` entry for `(dest, trash, ts)` is present
       in the audit log `a` (i.e. was fsynced).
    2. AND the `snapshot_recursive` call returned `.ok`.

    The Rust runtime checks this predicate (implicitly, via control
    flow in `snapshot_then_rm`) before issuing the `unlink` — see the
    "Rust-side trust contract" prose at the bottom of this module. -/
def delete_licensed
    (dest trash : Path) (ts : String) (a : AuditLog) : Prop :=
  AuditEntry.mk AuditKind.QuarantineStart ts dest trash ∈ a.entries
  ∧ snapshot_recursive dest trash a = SnapResult.ok

/-! ### Main theorem -/

/-- **`quarantine_snapshot_precedes_delete` (v1.2.1 Item 5a, Rule 8 gate).**

    The safety contract for `--quarantine`: for any `(dest, trash, ts)`
    triple and any pre-pipeline audit log `a`, running the
    `quarantine_pipeline` produces `(r, a')` such that:

    * `r = .ok` (snapshot + audit fsync both succeeded) IFF
      `delete_licensed dest trash ts a'` holds (the `unlink` may fire).
    * `r = .err` (any failure) IFF `¬ delete_licensed dest trash ts a'`
      (the `unlink` MUST NOT fire — original `dest` stays intact).

    Combined with the Rust-side trust contract documented at the bottom
    of this module, this gives end-to-end safety: an `unlink(dest)`
    syscall fires iff the snapshot succeeded AND the audit-log `Start`
    entry was fsynced first.

    **Rust contract (forward reference):**
    `crates/grex-core/src/tree/quarantine.rs::snapshot_then_rm` —
    Item 5b's implementation, BLOCKED on this proof landing per Rule 8.

    The proof is a pure consequence of:
    * `quarantine_pipeline`'s structural definition (audit commit
      precedes snapshot call), and
    * `audit_commit_contains` (the committed entry is in the post-log,
      itself a `simp` lemma over list-snoc).

    No new bridge axiom needed for the theorem itself; the only new
    axiom is the data-typed `snapshot_recursive` placeholder for the
    Rust FS primitive (Bridge 10). -/
theorem quarantine_snapshot_precedes_delete
    (dest trash : Path) (ts : String) (a : AuditLog) :
    let result := quarantine_pipeline dest trash ts a
    let r := result.fst
    let a' := result.snd
    (r = SnapResult.ok ↔ delete_licensed dest trash ts a')
    ∧ (r = SnapResult.err ↔ ¬ delete_licensed dest trash ts a') := by
  -- Unfold the pipeline.
  simp only [quarantine_pipeline, delete_licensed]
  -- After unfolding, the audit log a' = a.commit (Start entry); the
  -- snapshot result r = snapshot_recursive dest trash a'. The
  -- `Start` entry is in a'.entries by `audit_commit_contains`, so the
  -- left conjunct of `delete_licensed` is trivially true; the
  -- whole predicate reduces to `snapshot_recursive _ _ _ = .ok`.
  have hmem :
      AuditEntry.mk AuditKind.QuarantineStart ts dest trash
        ∈ (a.commit ⟨AuditKind.QuarantineStart, ts, dest, trash⟩).entries :=
    audit_commit_contains a ⟨AuditKind.QuarantineStart, ts, dest, trash⟩
  -- Case split on the snapshot result.
  refine ⟨?_, ?_⟩
  · -- r = .ok ↔ delete_licensed
    constructor
    · intro hok
      exact ⟨hmem, hok⟩
    · intro ⟨_, hsnap⟩
      exact hsnap
  · -- r = .err ↔ ¬ delete_licensed
    constructor
    · intro herr hlic
      have : SnapResult.err = SnapResult.ok := herr ▸ hlic.2
      exact SnapResult.noConfusion this
    · intro hnotlic
      -- snapshot result is either .ok or .err; if .ok, delete_licensed
      -- would hold (contradiction with hnotlic), so it must be .err.
      cases hcase :
          snapshot_recursive dest trash
            (a.commit ⟨AuditKind.QuarantineStart, ts, dest, trash⟩) with
      | ok =>
        exfalso
        exact hnotlic ⟨hmem, hcase⟩
      | err => rfl

/-! ### Rust-side trust contract (NOT axiomatised — see prose)

The Rust runtime's `unlink(dest)` syscall in
`crates/grex-core/src/tree/quarantine.rs::snapshot_then_rm` (forward
ref, Item 5b) fires iff the pure-model `delete_licensed` predicate holds
at the post-pipeline audit log. This binding is NOT axiomatised in Lean
because the theorem above already proves the model-side iff; the Rust
impl earns the safety guarantee by faithfully realising the pipeline:

```rust
fn snapshot_then_rm(dest, trash, ts, audit_log) -> Result<()> {
    let entry = AuditEntry { kind: QuarantineStart, ts, src: dest, trash };
    audit_log.append_and_fsync(entry)?;        // ← matches AuditLog.commit
    snapshot_recursive(dest, trash)?;          // ← matches snapshot_recursive
    // ↑ both ? early-return on failure. Reaching the next line means
    //   delete_licensed holds, so the unlink is sound:
    unlink_recursive(dest)?;
    audit_log.append_and_fsync(QuarantineComplete);
    Ok(())
}
```

The two `?` early-returns realize the "iff": if either step fails,
control flow does NOT reach `unlink_recursive`, matching the model's
`r = .err ↔ ¬ delete_licensed` branch.

**Re-review triggers** (any of these invalidates the safety guarantee):
- `snapshot_then_rm` adds an alternative `unlink` path bypassing either
  the audit append or the snapshot.
- audit log persistence model swapped (e.g. `O_DSYNC` write replaced by
  buffered I/O without explicit `fsync`).
- snapshot primitive swapped from `cap-std` recursive copy to a
  non-bounded primitive (capability-handle invariant lost).

Reviewers verify these against `inst/proof/impl-axiom-bridge.md`
(separate SSOT-repo commit per Rule 7).
-/

end Grex.Walker
