---
slug: feat-ssot-mount-rename
type: spec
status: draft
last_updated: 2026-05-07
topic: ssot-mount-rename
depends_on: [feat-ssot-reorg-velocity]
---

# feat-ssot-mount-rename — split SSOT mount from runtime worktree mount

## § Why

Today the grex repo mounts the SSOT (`grex-inst` working tree) at `.omne/`, AND the same `.omne/` tree houses runtime ephemera: `.omne/wt/<branch>` git worktrees the agent creates per rule 17. One directory, two semantics:

- **Knowledge space** (versioned, SSOT-tracked, mirror of `grex-inst` repo)
- **Runtime artifact space** (gitignored ephemera, agent-created worktrees)

Colocating them couples unrelated concerns. Future SSOT reorgs (cf. `feat-ssot-reorg-velocity` 2026-05-05 buckets) force-touch worktree-mount conventions; runtime-only refactors (e.g. moving worktree storage off-disk, sharding by parent feature) force-touch SSOT path references. The 2026-05-05 reorg already revealed the seam: rule 17 prescribes `.omne/wt/`, but `.omne/wt/` is logically not part of the 10 SSOT content buckets — it's a runtime mount that happens to share a prefix.

Splitting the mount makes the boundary explicit: knowledge at `inst/`, runtime at `.omne/`. Both gitignored at grex level (rule 17 + SSOT discipline rule 7 unchanged); only the path strings move.

## § Proposal

Rename the grex-side SSOT mount and re-purpose the freed name:

| Concern | Before (≤ 2026-05-06) | After (≥ 2026-05-07) |
|---|---|---|
| SSOT mount (knowledge) | `.omne/` | `inst/` |
| Runtime worktree mount | `.omne/wt/<branch>` | `.omne/wt/<branch>` (path unchanged; `.omne/` is now runtime-only) |
| `.gitignore` lines | `.omne` | `inst` + `.omne/` |
| Rule 17 prescribed path | `.omne/wt/<branch>` | `.omne/wt/<branch>` (text updated to clarify SSOT lives at `inst/`, decoupled) |

The rename is a path-string refactor + plain-directory move. No SSOT semantics shift — `grex-inst` repo's internal layout (10 buckets + 7 cfg/ sub-buckets, see `feat-ssot-reorg-velocity/design.md §Scope`) is UNCHANGED. Only the grex-side mount path changes.

### Touchpoints (counts pre-rewrite)

- 30 grex-repo external file refs to `.omne/` (grex-doc/, openspec/, CHANGELOG.md, `.github/workflows/ci.yml`, etc.; excludes generated artifacts like `searchindex.json`)
- 21 SSOT-internal refs (inside the mount itself — rules.md, design docs, history.md, etc.)
- 26 refs in `grex/CLAUDE.md` (this repo's bootloader)
- 1 ref in `grex-org/CLAUDE.md` (parent-org bootloader)
- ci.yml refs (path-based gates)
- `.omne/wt/` worktrees: NONE active; only `.gitkeep` placeholder exists

## § Out of scope (explicit)

- **`grex-inst` internal directory layout** — UNCHANGED. The 10 content buckets (`buf/`, `cfg/`, `grad/`, `rem/`, `schemas/`, `agents/`, `obs/`, `archive/`, `prompts/`, `skills/`) and `wt/` worktree-mount slot inside the SSOT repo all stay where they are. Only the grex-side mount path renames.
- **Lean4 proofs** — none. This is a structural/refactor change, not a verified-spec feature. No theorem deltas, no axiom-budget impact.
- **`.omne/wt/` worktrees migration** — none active. Only `.gitkeep` placeholder; recreated under new runtime `.omne/wt/.gitkeep` post-rename.
- **Historical reference rewriting** — entries in `inst/archive/` and `inst/grad/changes/feat-ssot-reorg-velocity/` that describe past `.omne/` state remain as-is. They describe historical state correctly.

## § SemVer verdict

**No bump.** Structural / dev-loop / tooling change. No grex CLI surface, no SSOT contract, no public-API delta. The 4 published crates (grex-core, grex-mcp, grex-plugins-builtin, grex-cli) are untouched. Rule 6 (SemVer = maintainer's call) trivially satisfied.

## § Acceptance criteria

1. `inst/` exists at grex repo root with full content of former `.omne/` (file-byte equivalent modulo `wt/` placeholder).
2. `.omne/wt/.gitkeep` exists; `.omne/` otherwise empty post-rename.
3. `.gitignore` ignores both `inst` and `.omne/` (single-line `.omne` replaced by two distinct lines).
4. Zero `.omne/<X>` refs remain in grex repo outside historical/archive contexts (audit grep clean against `grex-doc/`, `openspec/`, `CHANGELOG.md`, `.github/workflows/`, both `CLAUDE.md` files).
5. Zero `.omne/<X>` refs remain inside the mount itself, except in `inst/archive/` and `inst/grad/changes/feat-ssot-reorg-velocity/` (historical preservation).
6. `inst/schemas/conduct/rules.md` rule 17 reflects new layout: SSOT at `inst/`, worktree mount at `.omne/wt/<branch>`, decoupled.
7. `python .scripts/test.py` passes from parent meta-repo `E:\repos\utils\grex-org` (lint + integrity + regression all green).
8. SSOT INDEX.yaml regenerates clean (`python inst/scripts/build_index.py` exit 0; new path entries reflect `inst/` prefix where INDEX records mount-relative paths).
9. ci.yml path gates pass on a no-op CI run.

## § Risks

- **Stale worktrees on contributor machines.** Anyone with active local `.omne/wt/<branch>` directories that are git worktrees pointing at this repo will see them survive (worktrees gitignored, not in the rename path) but the SSOT they referenced moves. Mitigation: none active per inventory; document in design.md migration sequence + add a one-line operator hint in PR body.
- **External tooling indexers.** If any IDE / search tool indexes `.omne/` as a knowledge root, it will go quiet until reconfigured to `inst/`. Mitigation: PR body callout; `inst/` is the new canonical mount.
- **Rewrite ordering.** If physical rename happens before path-string rewrite, in-flight references break. Mitigation: rewrite refs FIRST (refs still valid against old `.omne/`), THEN physical rename (rewritten refs immediately re-point at new `inst/`). See design.md §Migration sequence.
- **Historical doc drift.** Archive / completed-feature design docs describe the OLD `.omne/` layout correctly; rewriting them would corrupt the historical record. Mitigation: explicit allowlist in tasks.md — preserve `inst/archive/**` and `inst/grad/changes/feat-ssot-reorg-velocity/**` as-is.

## § Decisions locked (2026-05-07)

1. **New SSOT mount name = `inst/`.** Mirrors the upstream repo name `grex-inst`; signals "instructions" / "instance of SSOT" without overloading `.omne/` semantics.
2. **Runtime mount keeps `.omne/` name.** Avoids a second rename in the same PR. `.omne/` semantics narrow from "SSOT + runtime" to "runtime-only".
3. **Both gitignored.** `inst/` is gitignored at grex level (it's a separate repo's working tree, parent meta-repo `REPOS.json` registers it). `.omne/` is gitignored at grex level (runtime ephemera). Two distinct `.gitignore` lines.
4. **Rule 17 stays at the same numeric slot.** Rewrite the text in place; do not renumber.
5. **No mirror to SSOT during draft.** Triplet lives only in grex `openspec/changes/feat-ssot-mount-rename/` until impl phase; mirroring to `inst/grad/changes/feat-ssot-mount-rename/` happens during Phase 2 per rule 17 conventions.
