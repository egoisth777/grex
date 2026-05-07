---
slug: feat-v-1-3-3-design
type: spec
status: active
last_updated: 2026-05-07
topic: ux-polish-bundle
depends_on: [feat-v-1-3-3]
---

# feat-v1.3.3 — design

Three independent fixes shipped under one release window. Each gets its own design section. B10 is the heaviest (folder FA + Lean4 obligations); B3 and B5 are small surface deltas.

## § B3 — `--pack .` / `--workspace .` cwd shorthand

### Semantics (per Q2)

`.` resolves to **process cwd** (`std::env::current_dir()`). **No walk-up.** Per user (Q2): "grex must be called from a grex pack, why walking up?" — the cwd-only semantics matches the user's mental model that `grex` runs from a pack root.

### Argument parsing

In `crates/grex/src/cli/`, the `--pack <PATH>` and `--workspace <PATH>` flag handlers receive a literal `.` token. Detection point: after `clap` argument parse, before path canonicalization. Add a guard:

```
if path.as_os_str() == "." {
    let cwd = std::env::current_dir()?;
    // proceed with cwd as the resolved path
} else {
    // existing path handling unchanged
}
```

Resolved cwd path then flows into the existing pack-root validation. If validation fails (cwd is not a valid pack root), the existing error path triggers; error message prints the resolved absolute cwd so the user sees exactly what `.` resolved to.

### Why no walk-up

Walking up to find the nearest pack root is plausible UX (matches `git`'s behavior) but the user explicitly rejected it. Two reasons:

1. **Predictability.** Walk-up resolution depends on FS state above cwd, hidden from the user. cwd-only resolution is deterministic from the user's terminal context.
2. **Sub-pack workflow ambiguity.** If cwd is inside a sub-pack and walk-up finds a parent pack, the user's intent ("operate on this pack") is silently overridden by walk-up. cwd-only refuses ambiguity at parse time.

### Out of scope for B3

- `--pack ..` or other relative path forms — `.` is the only literal alias added. Other relative paths flow through existing path handling unchanged.
- Inheriting walk-up behavior from any other grex verb — if a different verb walks up today, B3 does not change that. B3 narrowly scopes to the literal `.` token in `--pack` / `--workspace` flag values.

## § B5 — `grex doctor` `.gitignore`-aware drift check

### Semantics (per Q3)

**Warn-only.** Doctor exits 0 even when drift detected. New summary block at end of doctor run prompts the user to add the pack to `.gitignore` (or remove an over-broad rule). Per user: "detect non tracking pack, create a summary at the end of the doctor to tell prompt user to add the pack."

### Drift detection

In `crates/grex-core/src/doctor/`, register a new check (alongside existing checks like `scan_undeclared.rs`). For each pack registered in the workspace:

1. Walk up from the pack root to find the nearest enclosing git repo (parent repo).
2. Read parent repo's `.gitignore` rules.
3. Classify the pack against parent rules:
   - **Tracked** — pack content is NOT matched by any ignore rule. Healthy.
   - **Not-tracked** — pack lives outside ignore coverage but project convention says it should be ignored (e.g. ephemeral checkouts). User likely forgot to add the pack to `.gitignore`.
   - **Over-ignored** — an over-broad ignore rule swallows pack content the user wants tracked. Less common; same drift class.

Drift = any pack classified as `Not-tracked` or `Over-ignored`.

### Summary block

End of doctor run, when drift detected, emit:

```
WARN: <N> pack(s) drift parent .gitignore tracking expectations:

  <pack-path>  — not-tracked (consider adding to .gitignore)
  <pack-path>  — over-ignored (consider narrowing rule '<rule>' in .gitignore)

Action: review .gitignore at <parent-repo-path> and add/remove rules accordingly.
```

Severity = warn (info). Doctor exits 0 in this case. Existing doctor checks (advisory + critical) unchanged.

### Why warn-only

Per Q3 user response: not all users gitignore packs (some intentionally track, some intentionally ignore). Failing doctor on drift would force a binary choice on users with legitimate non-default workflows. Warn-only surfaces the signal without blocking, summary prompt nudges toward common case.

### Test seam

Add unit test in `crates/grex-core/tests/` (e.g. `doctor_gitignore_drift.rs`):

- Drift case: parent repo with pack outside ignore coverage → summary block emitted, exit 0.
- No-drift case: parent repo correctly tracking pack → no summary, exit 0.
- Over-ignored case: ignore rule swallows pack → summary block lists the offending rule, exit 0.

## § B10 — `grex add --ref <git-ref>` flag + folder FA

This section reproduces the discussion-doc design block (`inst/buf/changes/feat-v1.3.3/discussion.md` §B10 Refined Design) verbatim with OQ1–OQ5 resolutions inlined.

### Folder layout (universal)

```
<parent-pack>/
  <reponame>/
    <refdir>/      <-- pack contents check out here
      ... files
```

`<reponame>` derived from URL last segment, stripped of `.git` suffix.

### Input axes (Boolean triple)

| Symbol | Meaning |
| ------ | ------- |
| **B**  | Branch component present in `--ref`? (1=yes, 0=no) |
| **C**  | Commit component present in `--ref`? (1=yes, 0=no) |
| **U**  | URL already tracked in parent manifest? (1=yes, 0=no) |

`--ref` syntax (per OQ1 resolution):

- `--ref main` → B=1, C=0
- `--ref a3f9c1d` (7-char SHA min, 40-char SHA accepted) → B=0, C=1
- `--ref main@a3f9c1d` (single flag, `@` delimiter) → B=1, C=1
- omitted → B=0, C=0

`@` chosen because it is illegal in branch names per git ref-format rules, so parsing is unambiguous.

### Transition table (8 cells)

| #   | B   | C   | U   | `<refdir>` resolution     | Action                                                                     | Notes                                                |
| --- | --- | --- | --- | ------------------------- | -------------------------------------------------------------------------- | ---------------------------------------------------- |
| 1   | 0   | 0   | 0   | `main`                    | **Add**                                                                    | default — `main` branch, latest commit pinned        |
| 2   | 0   | 0   | 1   | `main`                    | **silent reject**                                                          | dup of default checkout                              |
| 3   | 1   | 0   | 0   | `<branch>`                | **Add**                                                                    | latest commit on branch pinned                       |
| 4   | 1   | 0   | 1   | `<branch>`                | if same branch tracked → **silent reject**; else **Add** sibling           | branch differs → new folder under same `<reponame>/` |
| 5   | 0   | 1   | 0   | `main@<commit-short>`     | **Add**                                                                    | bare commit, no branch context (default-to-main)     |
| 6   | 0   | 1   | 1   | `main@<commit-short>`     | if same commit tracked → **warn + reject**; else **warn + Add**            |                                                      |
| 7   | 1   | 1   | 0   | `<branch>@<commit-short>` | **Add**                                                                    | branch + commit pin                                  |
| 8   | 1   | 1   | 1   | `<branch>@<commit-short>` | if (branch, commit) tuple matches → **warn + reject**; else **warn + Add** |                                                      |

Cells 5/6 use `main@<commit-short>` (per OQ2/OQ3 resolution — no `detached/` namespace; consistent default-to-main when B=0).

### Path-encoding rules

| Token | Format |
|---|---|
| `<reponame>` | URL last path segment, `.git` suffix stripped |
| `<branch>` | branch name with `/` → `_` (e.g. `feature/foo` → `feature_foo`) |
| `<commit-short>` | 7-char SHA prefix; **add-time uniqueness check; extend prefix on collision** (per OQ5) |
| Separator `@` | literal `@` (illegal in branch names per git ref-format rules) |

### Add-time uniqueness check (OQ5)

When computing `<commit-short>` for a new add:

1. Compute candidate = first 7 chars of resolved 40-char SHA.
2. Scan existing manifest entries under same `<reponame>/` for any existing `<commit-short>` whose prefix collides with candidate (or candidate's prefix collides with existing).
3. If collision: extend candidate prefix one char at a time until uniqueness (max 40 chars; collisions at 40 chars are full-SHA equality, which the FA classifies as cells 6/8 dedup branches, not collision).
4. Manifest persists the FULL 40-char SHA in `ref:` field. The folder name uses the (possibly extended) `<commit-short>`. Folder name and manifest SHA are linked but not identical.

### Universal invariants (no-dangling-HEAD discipline)

- **At add-time**, always resolve target to specific 40-char commit SHA. Manifest persists the SHA, never a floating ref.
- Branch checkouts (cells 1, 3, 4) **track** latest commit at add-time but freeze to that commit in manifest. Subsequent `grex sync` upgrades follow branch HEAD on next pull (semantics defined elsewhere — orthogonal to B10).
- Commit checkouts (cells 5, 6, 7, 8) freeze to exact SHA, never advance.
- HEAD never detached at FS level — local branch pointer always set even for cells 5–8 (created as detached but immediately attached to local branch named `pin/<commit-short>`).

### Action semantics

| Action | FS mutation | Manifest mutation | Exit code |
|---|---|---|---|
| Add | clone + checkout | append entry | 0 |
| Add sibling | clone into new `<refdir>` | append entry | 0 |
| silent reject | none | none | 0 (no-op idempotent) |
| warn + reject | none | none | 1 (with stderr warn) |
| warn + Add | clone + checkout | append entry | 0 (with stderr warn) |

### Lean4 proof obligations (ship-in-v1.3.3)

Per OQ4 resolution: Lean4 file in **`proof/`** folder (project repo grex), NOT `inst/cfg/proof/`. Final filename TBD during Phase 2 (proposed: `proof/Grex/RefFa.lean`).

Three theorems must land green in Lean axiom budget before v1.3.3 ships.

**Theorem 1: `ref_fa_total`**

For every input `(B, C, U, branch?, commit?, url, parent_manifest)` in the valid domain, the FA produces exactly one `(refdir, action)` pair. Total function, no undefined cells.

```lean
theorem ref_fa_total :
  ∀ (input : RefInput), ∃! (out : RefOutput), ref_fa input = out
```

**Theorem 2: `ref_folder_injective`**

Among inputs that resolve to action ∈ {Add, AddSibling, WarnAdd}, distinct inputs produce distinct `<refdir>` paths within the same `<reponame>/` parent. (Prevents silent overwrite. Encodes the OQ5 collision-extend invariant: post-extension, distinct commits yield distinct `<commit-short>`.)

```lean
theorem ref_folder_injective :
  ∀ (i₁ i₂ : RefInput),
    same_repo i₁ i₂ →
    is_add_action (ref_fa i₁).action →
    is_add_action (ref_fa i₂).action →
    i₁ ≠ i₂ →
    (ref_fa i₁).refdir ≠ (ref_fa i₂).refdir
```

**Theorem 3: `dup_safe`**

If action ∈ {SilentReject, WarnReject}, no FS mutation and no manifest mutation occurs.

```lean
theorem dup_safe :
  ∀ (input : RefInput) (state : WorldState),
    is_reject_action (ref_fa input).action →
    apply_action input state = state
```

### Why these three theorems

- **`ref_fa_total`** rules out the class of bugs where a `(B, C, U)` cell is forgotten in Rust impl — Lean exhaustivity check catches it before code lands.
- **`ref_folder_injective`** rules out silent-overwrite bugs (two distinct adds clobbering each other's checkout) — the most user-hostile failure mode in B10.
- **`dup_safe`** rules out the bug where re-adding an identical entry has side effects — locks idempotence at the spec level.

These three jointly cover the FA's safety surface. Liveness (the FA terminates, the FS clone actually happens) is delegated to existing crate-level tests; the FA itself is decidable + finite-state, so termination is structural.

## § Implementation ordering rationale (Lean4 first)

Per user (2026-05-06): "Lean4 proofs land BEFORE B3/B5/B10 code in implementation order."

Rationale:

1. **Contract lock.** The 8-cell table is the contract. Encoding it in Lean first forces the design to be precise (every cell named, every action enumerated, every invariant stated) before Rust impl makes any assumption ambient. If the Lean encoding reveals an underspecified cell, redesign happens cheaply at the spec level, not after Rust code is written.
2. **Test oracle.** Once Lean4 ports clean, the Rust impl has a concrete oracle: each Rust arm corresponds to one Lean cell. Golden-table tests in Rust derive directly from Lean cell enumeration. Test correctness is downstream of proof correctness.
3. **Axiom budget gate.** v1.3.2 baseline is 9/4/0. Lean4 first means axiom budget delta is measured before Rust impl hides the proof obligation under crate-level tests. If theorem 2 (`ref_folder_injective`) requires a new axiom, that axiom lands in the budget BEFORE B10 ships, not retroactively.
4. **Reviewer artifact.** Cavecrew-reviewer + code-reviewer can audit the Lean encoding independently of Rust impl. Catches FA-design defects in review without needing a working B10 implementation.

Commit ordering (Phase 7): Phase 2 (Lean) → Phase 3 (B3) → Phase 4 (B5) → Phase 5 (B10) → Phase 6 (verify+changelog). Reviewable as a stack.

B3 and B5 are independent of the Lean4 proofs (they touch CLI parsing and doctor checks, not the FA). They could land in any order relative to Lean4. The user-mandated ordering (Lean first) keeps the rule simple: all proofs before all impl.

## § Out of scope (design-level)

- **gix HTTPS feature** — deferred to v1.4.0 per Q5. Blocks 13 real-smoke fixtures; v1.3.3 keeps the existing real-smoke harness scope.
- **B1 / B9 / B15** — deferred to v1.3.4 cleanup bundle. Not addressed here.
- **`grex sync` ref-advance for branch checkouts** — orthogonal to B10 add-time pinning. Sync-time semantics defined elsewhere; v1.3.3 only changes add-time behavior.
- **`--branch` / `--commit` separate flags** (OQ1 alternative) — single `--ref` with `@` delimiter chosen.
- **`detached/` namespace folder prefix** (OQ2 / OQ3 alternative) — dissolved; cells 5/6 use `main@<commit-short>`.
- **12-char SHA default** (OQ5 alternative) — 7-char + collision-extend chosen instead.
- **Lean4 file under `inst/cfg/proof/`** (OQ4 alternative) — file lives in `proof/` at grex repo root per OQ4 resolution.
- **Public-API contract drift** — none. `--ref` flag is additive. `--pack .` shorthand is additive. `grex doctor` summary is additive (warn-only). 4 published crates retain v1.3.x compatibility.

## § Risks (design-level)

- **Lean4 encoding gap vs Rust impl.** If the Rust port of the FA drifts from the Lean encoding, golden-table tests catch most cases but not all (e.g. `<reponame>` derivation lives outside the FA proper). Mitigation: golden table covers all 8 cells with concrete `(reponame, branch, commit)` triples; `<reponame>` derivation has its own unit-test suite.
- **B5 false-positive churn.** Warn-only severity means false positives are cheap, but if every doctor run shows a noisy summary, users tune it out. Mitigation: scope drift detection narrowly (only flag clearly-not-tracked or clearly-over-ignored, not ambiguous middle ground); leave heuristic refinement to v1.3.4 if dogfood feedback signals noise.
- **B3 sub-pack confusion.** If user is in a sub-pack root and runs `grex sync --pack .`, cwd-only resolution operates on the sub-pack, NOT the parent. This is the user-intended behavior per Q2, but might surprise users from other tooling backgrounds. Mitigation: error message + summary block call out the resolved cwd explicitly.
- **B10 manifest schema interaction.** `--ref` writes `ref:` field. Existing manifests with hand-edited `ref:` are honored on re-add. If a user hand-edited `ref:` to a value the FA would not produce (e.g. tag name), re-add behavior is undefined. Mitigation: v1.3.3 scope only covers refs the FA produces; hand-edited exotic refs out of scope (deferred to a future feature).
