---
slug: feat-v-1-3-3-tasks
type: spec
status: active
last_updated: 2026-05-07
topic: ux-polish-bundle
depends_on: [feat-v-1-3-3, feat-v-1-3-3-design]
---

# feat-v1.3.3 — tasks

Phase-by-phase checklist. Each task ends with `→ verify: <one-line check>` per rule 4. Mark `[x]` when complete.

**Implementation ordering rule (per user, 2026-05-06):** Lean4 proofs (Phase 2) land BEFORE Rust impl (Phases 3–5). Locks the FA contract before executable code references it.

## § Phase 1 — Branch + OpenSpec triplet

- [x] Branch `feat-v1.3.3` cut from main @ 038d028 (post-feat-ssot-mount-rename merge)  → verify: `git branch --show-current` = `feat-v1.3.3`
- [x] `proposal.md` drafted  → verify: this file references it as `depends_on`
- [x] `design.md` drafted with B3/B5/B10 sections + Lean4 obligations + ordering rationale  → verify: `design.md` exists with all four sections
- [x] `tasks.md` drafted (this file)  → verify: file exists, frontmatter slug `feat-v-1-3-3-tasks`
- [ ] cavecrew-reviewer pass on triplet  → verify: 0 findings on triplet structure / frontmatter / scope coherence

## § Phase 2 — Lean4 proofs (BEFORE any Rust impl)

Lean4 file lives in `proof/` folder per OQ4. Proposed path `proof/Grex/RefFa.lean` (final path confirmed during this phase).

- [ ] Pick final Lean4 file path under `proof/` matching existing axiom-budget convention  → verify: path chosen; axiom budget baseline (9/4/0 from v1.3.2) recorded for delta tracking
- [ ] Encode `RefInput` type: `(B : Bool, C : Bool, U : Bool, branch? : Option String, commit? : Option String, url : String, parent_manifest : Manifest)`  → verify: type compiles; constructors round-trip
- [ ] Encode `RefOutput` type: `(refdir : String, action : Action)` where `Action` enum covers `Add | AddSibling | SilentReject | WarnReject | WarnAdd`  → verify: type compiles; all 5 constructors typecheck
- [ ] Encode `ref_fa : RefInput → RefOutput` per the 8-cell transition table in `design.md §B10 transition table`  → verify: all 8 cells have a matching arm; `lake build` clean for the function definition
- [ ] **Theorem 1 — `ref_fa_total`**: prove the FA is total over `RefInput`. `∀ input, ∃! out, ref_fa input = out`  → verify: theorem accepted, no `sorry`, no `axiom` introduced
- [ ] **Theorem 2 — `ref_folder_injective`**: prove distinct add-action inputs within same repo yield distinct refdirs. Statement per `design.md §B10 Lean4 obligations`  → verify: theorem accepted; exhaustive case split over `(B, C, U)` covers all 8 cells; collision-extend invariant referenced
- [ ] **Theorem 3 — `dup_safe`**: prove reject-action inputs leave `WorldState` unchanged. Statement per `design.md §B10 Lean4 obligations`  → verify: theorem accepted; covers `SilentReject` (cells 2, 4-same-branch) and `WarnReject` (cells 6-same-commit, 8-same-tuple)
- [ ] `lake build` clean over the whole proof tree  → verify: `lake build` exits 0; no new axioms beyond v1.3.2 baseline (or delta documented in `inst/cfg/proof/`)
- [ ] Axiom budget recheck: 9/4/0 (or documented delta)  → verify: `inst/cfg/proof/` baseline matches actual build output

## § Phase 3 — B3 impl (`--pack .` / `--workspace .` cwd shorthand)

- [ ] Locate `--pack` / `--workspace` flag parsing in `crates/grex/src/cli/`  → verify: argparse site identified
- [ ] Add `.` literal handling: resolve to `std::env::current_dir()`  → verify: code path treats `.` as cwd-alias, NOT walks parent dirs
- [ ] Reuse existing "not a pack root" error path; ensure error message prints resolved absolute cwd  → verify: manual test from non-pack cwd shows resolved absolute path in error
- [ ] Add unit test in `crates/grex/tests/` (or wherever CLI tests live): `--pack .` from pack cwd succeeds; `--pack .` from non-pack cwd errors with absolute path in message  → verify: `cargo test --workspace` exits 0 with new test green
- [ ] Repeat for `--workspace .` if `--workspace` is a separate flag with separate code path  → verify: same test pattern, both flags covered

## § Phase 4 — B5 impl (doctor `.gitignore`-aware drift, warn-only)

- [ ] Locate doctor check registry in `crates/grex-core/src/doctor/`  → verify: registration site identified
- [ ] Add new check: scan parent repo `.gitignore` rules vs pack content paths; classify each pack as tracked / not-tracked / over-ignored  → verify: classifier function exists with explicit enum return type
- [ ] Severity = warn (info); doctor exits 0 even when drift detected  → verify: unit test asserts exit code 0 in drift case
- [ ] Append summary block at end of `grex doctor` run when drift detected: list non-tracking packs, prompt user to add to `.gitignore` (or remove over-broad rule)  → verify: manual smoke from a parent repo with un-gitignored pack shows summary block
- [ ] Add unit test in `crates/grex-core/tests/`: drift-detection produces summary; non-drift run produces no summary; summary text matches Q3 prompt semantics  → verify: `cargo test --workspace` exits 0
- [ ] Re-run existing `doctor_advisory.rs` tests to confirm no regressions  → verify: existing doctor tests stay green

## § Phase 5 — B10 impl (`--ref` flag + folder FA + uniqueness)

- [ ] Locate `grex add` verb in `crates/grex/src/cli/verbs/add.rs` (or equivalent)  → verify: file exists, accepts `<url>` arg today
- [ ] Add `--ref <git-ref>` flag accepting the four syntactic forms (branch, commit, `branch@commit`, omitted)  → verify: argparse rejects malformed `--ref` (e.g. `@`, `main@`, `@a3f9c1d`); accepts each documented form
- [ ] Implement `parse_ref(&str) -> RefInput` populating `(B, C, branch?, commit?)` Booleans + Options per syntax  → verify: unit tests round-trip each of the four forms
- [ ] Port the Lean4 8-cell FA into Rust: `ref_fa(RefInput) -> RefOutput`. Implementation MUST match the Lean4 case-split exactly (one Rust arm per Lean cell)  → verify: golden table test in `crates/grex-core/tests/` enumerates all 8 cells, asserts `(refdir, action)` matches Lean4 spec
- [ ] Implement `<reponame>` derivation: URL last path segment, strip `.git` suffix  → verify: unit test covers `https://x/y.git` → `y`, `https://x/y` → `y`, `git@x:y.git` → `y`
- [ ] Implement `<branch>` slash-encoding: `feature/foo` → `feature_foo`  → verify: unit test
- [ ] Implement `<commit-short>` 7-char prefix + add-time uniqueness check: scan existing manifest entries for same `<reponame>`, extend prefix if any existing `<commit-short>` matches the new candidate  → verify: collision-extend unit test (synthesized SHAs sharing 7-char prefix); ensures both end up with unique extended prefixes
- [ ] Wire FA output to FS + manifest mutation per `design.md §B10 action semantics` table  → verify: integration test in `crates/grex-core/tests/` covers cells 1, 3, 5, 7 (Add); cell 4 sibling Add; cells 2, 4-same-branch (silent reject); cells 6-same, 8-same (warn + reject)
- [ ] Manifest writes resolved 40-char commit SHA into `ref:` field (never floating ref)  → verify: post-add manifest inspection shows full SHA, not branch name
- [ ] Local branch attachment for cells 5–8: create local branch `pin/<commit-short>` to avoid detached HEAD at FS level  → verify: post-add `git branch` inside the checkout shows `pin/<commit-short>`
- [ ] Add CLI integration test under `crates/grex/tests/` (or wherever real-smoke-style tests live)  → verify: `cargo test --workspace` exits 0 with B10 tests green

## § Phase 6 — Verification

- [ ] `cargo test --workspace` exits 0 (full suite, 1009+ tests)  → verify: stdout shows expected test count, no failures
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean  → verify: no warnings, no errors
- [ ] `lake build` over the proof tree clean  → verify: exit 0; axiom budget at 9/4/0 (or documented delta)
- [ ] Real-smoke harness (`v1.3.x` fixtures from PR #67) stays green  → verify: real-smoke job exits 0
- [ ] Manual smoke per `proposal.md §Acceptance` item 9 (three operator scenarios): B3 cwd, B5 drift summary, B10 `main@<sha>` add  → verify: all three behaviors observed end-to-end
- [ ] `python inst/scripts/validate.py` clean (post any SSOT mirror updates)  → verify: exit 0
- [ ] `CHANGELOG.md` updated with `[1.3.3]` entry listing B3 + B5 + B10  → verify: changelog grep for `1.3.3` returns the new entry
- [ ] Cargo.toml versions bumped for 4 published crates (grex-core, grex-mcp, grex-plugins-builtin, grex-cli) to `1.3.3`  → verify: `cargo metadata` shows 1.3.3 for each

## § Phase 7 — Commit + PR

- [ ] Commits ordered: Lean4 first (Phase 2), then B3 (Phase 3), then B5 (Phase 4), then B10 (Phase 5), then verify+changelog (Phase 6)  → verify: `git log feat-v1.3.3 ^main --oneline` shows ordering
- [ ] All commits without `Co-Authored-By` trailer (rule 13)  → verify: `git log feat-v1.3.3 ^main --format=%B | grep -i 'co-authored-by\|claude\|anthropic\|generated'` returns empty
- [ ] Push branch, open PR `feat-v1.3.3 → main`  → verify: PR created; PR body has no AI/assistant trailer
- [ ] PR body lists B3 + B5 + B10 user-visible changes + Lean4 theorem inventory  → verify: PR body grep for `ref_fa_total`, `ref_folder_injective`, `dup_safe` returns all three
- [ ] Reviewer pass: cavecrew-reviewer + code-reviewer  → verify: 0 critical findings; advisory findings logged
- [ ] CI green on the PR  → verify: `gh pr checks` shows all required green
- [ ] Squash-merge to main  → verify: merge commit appears on main; tag `v1.3.3` cut

## § Phase 8 — SSOT mirror (post-merge, rule 16)

- [ ] Mirror this triplet from grex `openspec/changes/feat-v1.3.3/` to SSOT `inst/grad/changes/feat-v1.3.3/{proposal,tasks,design}.md`  → verify: SSOT triplet exists with same content + frontmatter
- [ ] `inst/grad/progress.md`: append endpoint section recording v1.3.3 SHIPPED 2026-05-XX with commit SHA + tag  → verify: progress.md endpoint section present, references B3/B5/B10
- [ ] `inst/grad/milestone.md`: mark v1.3.3 row complete  → verify: milestone.md grep for `1.3.3` shows shipped status
- [ ] `inst/INDEX.yaml` regenerated  → verify: `python inst/scripts/build_index.py` exit 0
- [ ] SSOT pre-commit hook green  → verify: `python inst/scripts/validate.py` exit 0
- [ ] Commit + push SSOT changes (separate working tree per rule 7; no `Co-Authored-By` per rule 13)  → verify: grex-inst main advances

## § Phase 9 — Endpoint

- [ ] Append endpoint to `inst/grad/progress.md`  → verify: progress.md has new endpoint reflecting v1.3.3 SHIPPED on main
- [ ] Update top `## Where we are` block: v1.3.3 shipped, v1.3.4 cleanup bundle next (B1/B9/B15)  → verify: top block coherent
- [ ] Discussion doc `inst/buf/changes/feat-v1.3.3/discussion.md` archived to `inst/archive/buf-history/`  → verify: discussion no longer in `buf/`; archive entry present
- [ ] Session-complete check: rule 16 SSOT update bundle green  → verify: progress.md, milestone.md, INDEX.yaml all coherent
