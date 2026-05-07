---
slug: feat-ssot-mount-rename-design
type: spec
status: draft
last_updated: 2026-05-07
topic: ssot-mount-rename
depends_on: [feat-ssot-mount-rename]
---

# feat-ssot-mount-rename — design

## § Why two mounts (knowledge vs runtime separation)

The grex repo has, until 2026-05-06, conflated two distinct concerns under a single mount path `.omne/`:

1. **Knowledge space.** The SSOT — design docs, conduct rules, openspec triplets, agent registry, history, INDEX. This is the working tree of a separate repo (`grex-inst`), checked out into grex via parent-meta-repo `REPOS.json` registration. Versioned, reviewable, single-source-of-truth.

2. **Runtime artifact space.** Agent-created git worktrees (`.omne/wt/<branch>`) per rule 17. Ephemeral, gitignored, no SSOT semantics, never reviewed, may be wiped at will.

Sharing one mount path created an implicit coupling: any refactor of SSOT layout (e.g. the 2026-05-05 `feat-ssot-reorg-velocity` 10-bucket reorg) had to be careful not to disturb worktree-mount conventions, and any rule-17 worktree-handling change had to thread through SSOT path discipline. The seam already showed in the 2026-05-05 reorg: rule 17 prescribes `.omne/wt/`, but `.omne/wt/` is not one of the 10 SSOT content buckets — it's a runtime concept that happens to live under the same prefix.

Splitting the mounts makes the boundary explicit and stable:

| Mount | Semantics | Versioning | Gitignored at grex | Lifecycle |
|---|---|---|---|---|
| `inst/` | SSOT (knowledge) | Tracked in `grex-inst` repo | Yes (separate repo's working tree) | Long-lived, reviewable |
| `.omne/` | Runtime (worktrees) | Untracked anywhere | Yes | Ephemeral, agent-managed |

## § Boundary contract

**`inst/` (SSOT mount):**

- Mounted from `grex-inst` repo via parent-meta `REPOS.json` registration (no submodule, no symlink — plain directory checkout).
- All "memorable" project knowledge lives here per `CLAUDE.md` Memory-discipline section (rule overrides global auto-memory).
- Internal layout governed by `feat-ssot-reorg-velocity` (10 content buckets + 7 cfg/ sub-buckets). Internal layout UNCHANGED by this feature; only the grex-side mount path renames.
- Never modified by runtime tooling or agent worktree operations.

**`.omne/` (runtime mount):**

- Plain gitignored directory at grex repo root. Not a checkout of any external repo.
- Holds `.omne/wt/<branch>` worktrees the agent creates per rule 17.
- May contain other ephemera in future (caches, transient state) — semantics narrowed from "SSOT + runtime" to "runtime-only", which OPENS this slot for future runtime needs.
- Never reviewed, never SSOT-tracked, may be deleted and recreated at will.

**Decoupling guarantee:** modifying `inst/` (e.g. SSOT reorg, doc edits, INDEX regen) NEVER touches `.omne/`. Modifying `.omne/` (e.g. creating a worktree, wiping stale worktrees) NEVER touches `inst/`. The two mounts share no path overlap and no semantic dependency.

## § Rule 17 update (proposed text)

Current rule 17 text (`inst/schemas/conduct/rules.md` lines 282-286, post-rename — currently at `.omne/schemas/conduct/rules.md`):

> 17. Worktree storage location. `.omne/wt/` is the canonical location for all git worktrees the agent creates for this project. Use `git worktree add .omne/wt/<branch-name>`.

Proposed replacement text (verbatim — adopt in W1 task):

> 17. Worktree storage location. `.omne/wt/` is the canonical location for all git worktrees the agent creates for this project. Use `git worktree add .omne/wt/<branch-name>`. SSOT lives at `inst/` (separate mount). Runtime `.omne/` and SSOT `inst/` are decoupled — modifying one never touches the other.

Rationale:

- Path string `.omne/wt/<branch-name>` is UNCHANGED. No worktrees on contributor machines need migration.
- Numeric slot (rule 17) UNCHANGED. No renumbering side-effects on rule cross-references elsewhere in the codebase.
- Added clarification (sentences 2-3) makes the boundary contract explicit at the rule site, so future readers grasp the split without needing to consult this design doc.

## § Migration sequence (rewrite refs first, THEN physical rename)

Order matters. Two valid sequences exist; we adopt sequence A:

**Sequence A (adopted):** rewrite path strings → physical rename. Brief invalid window: between Phase 2 (rewrites complete, refs point at `inst/`) and Phase 3 (physical rename creates `inst/`), the rewritten refs are stale because `inst/` does not yet exist. Mitigation: Phase 2 + Phase 3 land in a SINGLE commit (no intermediate push); the invalid window only exists in working-tree state, never in pushed history.

**Sequence B (rejected):** physical rename → rewrite refs. Brief invalid window between physical rename (refs still point at `.omne/` but `.omne/` is gone) and rewrite (refs updated to `inst/`). Same mitigation possible (single commit), but during the rewrite phase the agent itself relies on SSOT files (rules.md, design docs) being readable at the path the agent expects — sequence B breaks the agent's own workflow mid-task.

Sequence A keeps the SSOT readable at `.omne/` throughout the rewrite, then completes the rename atomically in the same commit.

### Phase walk

1. **Phase 2 — rewrites.** Three parallel workers (W1 SSOT-internal, W2 grex-repo external, W3 CLAUDE.md). Disjoint write-sets at file granularity per rule 14. Refs now textually point at `inst/`; `inst/` does not yet exist on disk; SSOT remains physically at `.omne/`.

2. **Phase 3 — physical rename.** Plain-directory move `.omne/` → `inst/`. Recreate empty `.omne/` with `.omne/wt/.gitkeep`. Update `.gitignore` to ignore both. Refs now resolve correctly.

3. **Single commit.** Phase 2 + Phase 3 land as one squash commit. Pushed history never contains the invalid intermediate state.

## § Backout plan

Single PR, single squash commit. Backout = `git revert <merge-commit>` on main:

- Path-string rewrites reverse atomically (the diff is symmetric: `.omne/` ↔ `inst/`).
- Physical rename reverses atomically (the diff records the directory move).
- `.gitignore` reverses atomically (one line removed, one added).
- Rule 17 text reverses atomically (text replacement).

No partial-state recovery needed. No stateful migration to unwind. Contributors with stale local clones simply pull the revert.

## § Historical refs preservation (allowlist)

The following paths describe past SSOT layout correctly and MUST NOT be rewritten:

- `inst/archive/**` — durable records of completed features, retros, deprecated specs, buf history. Entries describe `.omne/` as it existed at the time of the entry; rewriting would corrupt the historical record.
- `inst/grad/changes/feat-ssot-reorg-velocity/**` — the 2026-05-05 SSOT reorg's own design doc references the pre-reorg `.omne/` layout. Rewriting would make the design doc internally inconsistent (it'd describe rewriting `.omne/` to `inst/` while ALSO describing the pre-reorg `.omne/` layout — same prefix, different scope).

Phase 2 W1 task explicitly excludes these subtrees from grep + rewrite. Phase 4 audit grep accepts hits in these subtrees as expected.

## § File map (rough touchpoint inventory)

Pre-rewrite counts (from feature scope):

| Scope | File count | Refs |
|---|---:|---:|
| W1 SSOT-internal | ~21 | ~21 |
| W2 grex-repo external | ~30 | ~30 |
| W3 CLAUDE.md (grex) | 1 | 26 |
| W3 CLAUDE.md (grex-org) | 1 | 1 |
| `.gitignore` | 1 | 1 line edited (replaced with 2) |
| Rule 17 text | 1 | 1 paragraph edited |
| Physical rename | 1 directory | n/a |

Total file edits: ~53 files. Total ref rewrites: ~78. Single directory rename. Single `.gitkeep` recreation.

## § Risks (design-level)

- **CI path-gate brittleness.** `.github/workflows/ci.yml` may reference `.omne/` paths in skip-checks or path-based triggers. W2 must include ci.yml in scope; Phase 4 verifies CI green before merge.
- **`grex-doc/searchindex.json` and other generated artifacts.** These are emitted by Sphinx / mdBook / similar; rewriting them by hand is meaningless because they regenerate from source. Phase 2 W2 explicitly EXCLUDES generated paths from rewrite scope; the next doc rebuild emits clean artifacts pointing at `inst/`.
- **Index regen drift.** SSOT `INDEX.yaml` is generated from path scan. Post-rename, regen MUST run before commit, or INDEX records stale `.omne/` paths internally. Phase 3 final task runs `python inst/scripts/build_index.py`.
- **Cross-repo references.** If any external repo (e.g. another sub-repo under `grex-org/`) hardcodes `.omne/` paths into grex, those hardcodes break post-merge. Inventory believes none exist, but Phase 4 should grep across the parent meta-repo to confirm.

## § Out of scope (design-level reaffirmation)

- `grex-inst` internal layout: UNCHANGED.
- Lean4 proofs: NONE (structural refactor; no algorithm).
- `.omne/wt/` worktree migration: NONE active (only `.gitkeep`).
- Worktree-mount path string: UNCHANGED at `.omne/wt/<branch>`.
- Crate versions, public APIs, CLI surfaces: UNTOUCHED.
- SSOT discipline rules other than rule 17: UNTOUCHED.
