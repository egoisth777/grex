---
slug: feat-ssot-mount-rename-tasks
type: spec
status: draft
last_updated: 2026-05-07
topic: ssot-mount-rename
depends_on: [feat-ssot-mount-rename, feat-ssot-mount-rename-design]
---

# feat-ssot-mount-rename — tasks

Phase-by-phase checklist. Each task ends with `→ verify: <one-line check>` per rule 4. Mark `[x]` when complete.

## § Phase 1 — Branch + OpenSpec triplet

- [x] Branch `feat-ssot-mount-rename` cut from main  → verify: `git branch --show-current` = `feat-ssot-mount-rename`
- [ ] `proposal.md` drafted  → verify: `python .omne/scripts/validate.py` (pre-rename) exit 0; re-validate with `python inst/scripts/validate.py` after Phase 3
- [ ] `design.md` drafted with migration sequence + boundary contract + rule 17 text proposal  → verify: `python .omne/scripts/validate.py` (pre-rename) exit 0; reviewer cross-checks rule 17 text reads coherent
- [ ] `tasks.md` drafted (this file)  → verify: `python .omne/scripts/validate.py` (pre-rename) exit 0
- [ ] cavecrew-reviewer pass on triplet  → verify: 0 findings on triplet structure / frontmatter / scope

## § Phase 2 — Path-string rewrites (3 parallel workers, disjoint write-sets per rule 14)

Dispatch all three workers in parallel. Phase 2 walk first confirms write-sets are disjoint at file granularity; if any two share a file, bundle them. **All rewrites happen BEFORE physical rename** so refs stay valid through Phase 2 (`.omne/` still exists on disk during this phase).

### W1 — SSOT-internal refs (21 files, lives inside the mount)

- [ ] Grep `.omne/` across the mount (current `.omne/` working tree) excluding `archive/` and `grad/changes/feat-ssot-reorg-velocity/`  → verify: grep returns the 21 expected files; allowlist directories untouched
- [ ] Rewrite `.omne/<X>` → `inst/<X>` in each match  → verify: grep over rewrite scope returns 0 hits
- [ ] **Rule 17 update** in `inst/schemas/conduct/rules.md` (currently `.omne/schemas/conduct/rules.md` lines 282-286): adopt the text from `design.md §Rule 17 update` verbatim  → verify: rule 17 explicitly states "SSOT lives at `inst/` (separate mount). Runtime `.omne/` and SSOT `inst/` are decoupled — modifying one never touches the other."
- [ ] Bump `last_updated` on any frontmatter where rule text or path strings changed (per rule 16)  → verify: each touched file's frontmatter shows `last_updated: 2026-05-07`
- [ ] **Allowlist preservation check**: `inst/archive/**` and `inst/grad/changes/feat-ssot-reorg-velocity/**` UNCHANGED  → verify: `git diff --stat` over those subtrees shows 0 lines changed

### W2 — grex-repo external refs (30 files)

- [ ] Grep `.omne/` across grex repo excluding the mount itself + generated artifacts  → verify: grep returns the 30 expected files (grex-doc/, openspec/, CHANGELOG.md, `.github/workflows/ci.yml`, scripts in `crates/*/build.rs` if any reference SSOT)
- [ ] **Exclude generated artifacts**: `grex-doc/build/`, `grex-doc/searchindex.json`, any `target/` artefact, any `Cargo.lock`-adjacent generated file  → verify: rewrite scope grep shows none of those paths in the diff
- [ ] Rewrite `.omne/<X>` → `inst/<X>` in each match  → verify: grep over rewrite scope returns 0 hits
- [ ] `.github/workflows/ci.yml` path gates re-point at `inst/`  → verify: ci.yml grep shows `inst/` path strings; workflow YAML still parses (`yamllint` or equivalent)
- [ ] `openspec/changes/feat-ssot-mount-rename/{proposal,tasks,design}.md` already use `inst/` in forward-looking text — sanity check  → verify: this triplet refers to `inst/` as the NEW mount and `.omne/` only when describing the OLD state or runtime worktree path

### W3 — CLAUDE.md bootloaders (grex + grex-org)

- [ ] `<repo-root>/CLAUDE.md` (26 refs): rewrite all `.omne/<X>` → `inst/<X>` EXCEPT references to runtime worktree path `.omne/wt/<branch>`  → verify: grep `\.omne/wt/` returns the runtime-mount references only; grep `\.omne/[^w]` returns 0 hits
- [ ] `<grex-org>/CLAUDE.md` (1 ref): rewrite `.omne/dist/AGENTS.md` → `inst/dist/AGENTS.md` (or whatever the canonical bootloader path becomes)  → verify: grep `\.omne/` over grex-org/CLAUDE.md returns 0 hits
- [ ] Update memory-discipline section in `grex/CLAUDE.md` (currently mentions `.omne/`) to reflect SSOT-only writes go to `inst/`  → verify: section reads coherent — "All memorable project knowledge lives EXCLUSIVELY in the SSOT at `inst/` (mounted from `grex-inst` repo)"
- [ ] Update "0-state hop-in" auto-load list to point at `inst/grad/progress.md`, `inst/grad/milestone.md`, etc.  → verify: all 7 auto-load entries reference `inst/`
- [ ] Update "SSOT layout" block to reflect the new mount name (10 buckets now under `inst/`, runtime worktrees under `.omne/wt/`)  → verify: layout block reads coherent

## § Phase 3 — Physical rename + scaffold

Gated on Phase 2 complete (all path-string rewrites landed; refs now point at `inst/` but `inst/` does not yet exist on disk — this is the brief invalid window the migration sequence accepts).

- [ ] Plain-directory rename: `.omne/` → `inst/`  → verify: `inst/` exists; `.omne/` does not exist on disk
- [ ] Recreate runtime mount: `mkdir .omne/`  → verify: `.omne/` exists, empty
- [ ] Create `.omne/wt/.gitkeep` to preserve worktree-mount slot  → verify: `.omne/wt/.gitkeep` exists
- [ ] `.gitignore`: replace single `.omne` line with two entries — `inst` and `.omne/` — on separate lines  → verify: `.gitignore` contains both `inst` and `.omne/` (or `.omne` — match existing convention) on separate lines
- [ ] Verify both mounts gitignored: `git status --ignored` shows both `inst/` and `.omne/`  → verify: both directories appear under "Ignored files"
- [ ] Refs now valid: spot-check 3 random rewritten refs resolve correctly via filesystem (e.g. `inst/grad/progress.md`, `inst/schemas/conduct/rules.md`, `inst/INDEX.yaml`)  → verify: each path resolves to a file
- [ ] Regenerate SSOT INDEX: `python inst/scripts/build_index.py` (assuming script path also rewrote)  → verify: `inst/INDEX.yaml` regenerates with new mount-relative paths; build_index.py exit 0

## § Phase 4 — Verification

- [ ] Run `python .scripts/test.py` from parent meta-repo `<grex-org>`  → verify: lint + integrity + regression phases all exit 0
- [ ] Audit grep: `grep -rn "\.omne/" .` excluding `inst/archive/`, `inst/grad/changes/feat-ssot-reorg-velocity/`, `target/`, `grex-doc/build/`, `searchindex.json`, `.git/`, `.omne/wt/`  → verify: 0 hits OR all hits are runtime-worktree references (`\.omne/wt/`) or rule-17 text describing the runtime mount
- [ ] Audit grep: `grep -rn "\.omne/" inst/` excluding `inst/archive/`, `inst/grad/changes/feat-ssot-reorg-velocity/`  → verify: 0 hits (the SSOT no longer references its own old mount path internally, except in historical scope)
- [ ] SSOT validate: `python inst/scripts/validate.py` exit 0  → verify: all SSOT frontmatter clean
- [ ] CI dry-run: trigger `.github/workflows/ci.yml` on the branch  → verify: path-based gates resolve against new `inst/` paths; CI green
- [ ] Rule 17 spot-read by reviewer  → verify: text matches `design.md §Rule 17 update` verbatim

## § Phase 5 — Commit + PR

- [ ] Stage all rewrites + rename + `.gitignore` change  → verify: `git status` shows expected file set; no stray `.omne/` paths
- [ ] Single squash commit on `feat-ssot-mount-rename`, no `Co-Authored-By` (rule 13)  → verify: `git log feat-ssot-mount-rename ^main --format=%B | grep -i 'co-authored-by\|claude\|anthropic'` returns empty
- [ ] Push branch, open PR `feat-ssot-mount-rename → main`  → verify: PR created; PR body has no AI/assistant trailer
- [ ] PR body includes contributor migration hint: "Local `.omne/wt/<branch>` worktrees survive; SSOT mount moved to `inst/`. Re-clone the SSOT submodule or `git mv` your local working tree."  → verify: PR body grep for "inst/" returns the migration hint
- [ ] Reviewer pass: cavecrew-reviewer + code-reviewer  → verify: 0 critical findings; advisory findings logged
- [ ] CI green on the PR  → verify: `gh pr checks` shows all required green
- [ ] Squash-merge to main  → verify: merge commit appears on main

## § Phase 6 — SSOT mirror (post-merge, rule 16)

- [ ] Mirror this triplet from grex `openspec/changes/feat-ssot-mount-rename/` to SSOT `inst/grad/changes/feat-ssot-mount-rename/{proposal,tasks,design}.md`  → verify: SSOT triplet exists with same content + frontmatter
- [ ] `inst/grad/progress.md`: append endpoint section recording v1.3.x dev-loop refactor SHIPPED 2026-05-07 with commit SHA  → verify: progress.md endpoint section present
- [ ] `inst/INDEX.yaml` regenerated  → verify: `python inst/scripts/build_index.py` exit 0
- [ ] SSOT pre-commit hook green  → verify: `python inst/scripts/validate.py` exit 0
- [ ] Commit + push SSOT changes (separate working tree per rule 7; no `Co-Authored-By` per rule 13)  → verify: grex-inst main advances

## § Phase 7 — Endpoint

- [ ] Append endpoint to grex `progress.md` (or `inst/grad/progress.md` post-rename)  → verify: progress.md has new endpoint reflecting SSOT-mount-rename SHIPPED on main
- [ ] Update top `## Where we are` block: SSOT now at `inst/`, runtime at `.omne/wt/`  → verify: top block coherent
- [ ] Session-complete check: rule 16 SSOT update bundle green  → verify: both grex `progress.md` and `inst/grad/progress.md` reflect SHIPPED in coherent state
