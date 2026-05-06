---
slug: feat-v1-3-2-tasks
type: spec
status: active
last_updated: 2026-05-03
topic: v1-3-2-patch
depends_on: [feat-v1-3-2, feat-v1-3-2-design]
---

# v1.3.2 — tasks

Phase-by-phase checklist. Each task ends with `→ verify: <one-line check>` per rule 4 (goal-driven execution). Mark `[x]` when complete.

## § Phase 1 — OpenSpec (this triplet)

- [x] proposal.md drafted on branch `feat-v1.3.2`  → verify: `git log --oneline feat-v1.3.2 ^main` shows the openspec commit; frontmatter validates via `python .omne/scripts/validate.py`
- [x] design.md drafted with full algorithm + contract change spec  → verify: same validate.py exit 0; design covers B6 + B11 + B13 + Lean theorem candidates + file map + frozen-contract table
- [x] tasks.md drafted (this file)  → verify: same validate.py exit 0
- [x] cavecrew-reviewer pass on triplet  → verify: reviewer reports 0 findings on triplet structure / frontmatter / scope
- [x] **Maintainer sign-off on design.md** (Phase 2 gate)  → verify: maintainer explicitly ACKs the B11 path-naming + backend-lock placement + B13 theorem name decisions surfaced as open questions in proposal.md

## § Phase 2 — Implementation (gated on Phase 1 maintainer sign-off)

### § Phase 2a — Lean (rule 8 gate, MUST land BEFORE any Rust change)

- [x] B13 theorem written in `proof/Grex/Walker.lean` (name ratified by maintainer; default `slash_path_walker_terminates`)  → verify: theorem statement matches signature in design.md §B13 Lean obligation
- [x] `lake build` green (cwd = `proof/`)  → verify: exit 0, 0 sorry, 0 admit, 0 hypothesis-level holes in B13 theorem
- [x] Axiom audit: 8 theorems on `[propext]` only or no axioms; budget 9/4/0 unchanged  → verify: `cargo run -p xtask -- axiom-audit` exit 0; theorem count = 8
- [x] Reviewer pass on Lean diff  → verify: cavecrew-reviewer + code-reviewer both clean

### § Phase 2b — Rust impl (3 parallel workers, disjoint write-sets per rule 14)

Dispatch ALL THREE workers in parallel ONLY after Phase 2a Lean is green. Phase 2 walk first confirms write-sets are disjoint at file granularity; if any two share a file, bundle them.

#### W1 — B6 retire `LockEntry.synthetic` writer

- [x] Phase 2 grep `synthetic` across `crates/grex-core/` to confirm no live reader  → verify: grep results show only struct field + (possibly) backward-compat deserialize site, no live consumer branching on `synthetic == true`
- [x] `crates/grex-core/src/lockfile/entry.rs`: `#[serde(default, skip_serializing_if = "..")]` on `synthetic` field (or remove field entirely if maintainer ratifies + add v1.1.x compat shim)  → verify: cargo build green; serialized v1.2.0+ `LockEntry` JSON has NO `synthetic` key
- [x] `crates/grex-core/src/lockfile/writer.rs`: drop any explicit `synthetic: ..` assignment at construction sites  → verify: cargo grep `synthetic:` in writer modules returns 0 hits in writer code paths
- [x] (Reader-side) remove any code branch consuming `LockEntry.synthetic` if found  → verify: walker no longer references the field at runtime; doctor.rs no longer special-cases synthetic packs (per `walker.md §LockEntry.synthetic deprecation` such consumers should already be gone)
- [x] New test file: `crates/grex-core/tests/lockfile_no_synthetic.rs`  → verify: test asserts (a) sync output JSON has no `synthetic` key; (b) v1.1.x lockfile with `synthetic: true` deserializes cleanly via `#[serde(default)]`; cargo test exits 0

#### W2 — B11 lockfile-location migration (3 lock artifacts)

- [x] Confirm Phase 2 walk: identify exact module owning per-pack pack-lock path constant (best-effort `crates/grex-core/src/concurrency/packlock.rs` or `fs/lock.rs`)  → verify: grep finds the path-string definition for `.grex-lock`
- [x] Per-pack pack-lock: rewrite path `<pack_workdir>/.grex-lock` → `<pack_workdir>/.grex/.grex-lock`  → verify: lock file appears at `<pack_workdir>/.grex/.grex-lock` after first sync; old path NEVER created
- [x] Workspace sync sidecar: rewrite path `<workspace>/.grex.sync.lock` → `<workspace>/.grex/.grex.sync.lock` (touchpoint: `open_workspace_lock` already touched by v1.3.1 B4 dry-run gate)  → verify: lock file appears at new path post-sync; old path NEVER created
- [x] Per-repo backend lock: rewrite path `<dest>.grex-backend.lock` (sibling) → `<parent_meta>/.grex/locks/<child-path>.backend.lock` (parent-owned). `mkdir -p` intermediate dirs on first acquire (literal child path used; no slug encoding).  → verify: lock file appears at `<parent>/.grex/locks/<child-path>.backend.lock` on first clone; old sibling path NEVER created; no inside-dest path created either
- [x] Hard-cut readers: NO fallback open-path code in any of the 3 lock writers  → verify: code review of all 3 lock-open sites confirms only the new path is attempted; no `if old_exists { ... }` branches
- [x] Update any rustdoc / inline comment referencing old paths  → verify: grep over `crates/grex-core/` and `crates/grex/` for `\.grex-lock|\.grex.sync.lock|\.grex-backend\.lock` returns only path-string references at the writer site (and updated test fixtures)
- [x] If `grex doctor` / `grex doctor --json` surfaces lock-paths in any finding, update path strings  → verify: doctor unit tests assert new path string in finding rendering
- [x] New test file: `crates/grex-core/tests/lockfile_under_grex.rs`  → verify: test asserts (a) per-pack lock at `<pack_workdir>/.grex/.grex-lock`, (b) workspace lock at `<workspace>/.grex/.grex.sync.lock`, (c) per-repo backend lock at `<parent_meta>/.grex/locks/<child-path>.backend.lock`. Negative assertion no file at old sibling/workspace-root/pack-root/inside-dest paths. cargo test exits 0

#### W3 — B13 walker / manifest loader slash-path support

- [x] `crates/grex-core/src/manifest/loader.rs` (or equivalent): replace `/`-rejection with per-segment validation (split → bare-name regex per segment → reject `..`, absolute, escape)  → verify: loader accepts `path: tools/foo`; loader still rejects `path: ../escape`, `path: /abs`, `path: tools/..`
- [x] `crates/grex-core/src/tree/walker.rs`: Phase 1 dest resolution joins parent-relative slash path; canonicalised dest must remain inside `current_meta`  → verify: walker resolves `tools/foo` correctly; walker rejects symlink-escape via existing `dest_has_git_repo` symlink-metadata probe
- [x] Walker invariant preserved: `walker never recurses into a folder lacking .grex/` (maintainer-locked 2026-05-03)  → verify: integration test confirms slash-path child whose intermediate folder lacks `.grex/` does NOT trigger recursion past that folder
- [x] `crates/grex-core/src/tree/graph_build.rs` (or `PackNode` construction): propagate slash-path through to lockfile keying per `manifest.md §v1.2.0 keying`  → verify: lockfile entry for `tools/foo` keys as `"tools/foo"` (forward-slash POSIX, normalized at write-time)
- [x] Cycle-detection `visited: Vec<String>` accepts slash-path ids without collision  → verify: B13 Lean theorem compiled in Phase 2a covers this; integration test with two children sharing `name:` at distinct paths confirms no false-cycle
- [x] New test file: `crates/grex-core/tests/slash_paths.rs`  → verify: positive cases (`tools/foo`, `courses/cpp/cpp-grammar`, mixed bare + slash siblings); negative cases (`..`, absolute, symlink-escape, intermediate-no-`.grex/` terminates); cargo test exits 0

### § Phase 2c — Cross-cuts (after W1/W2/W3 land)

- [x] All 4 crate Cargo.toml: version `1.3.1` → `1.3.2`  → verify: grep `version = "1.3.1"` over `crates/*/Cargo.toml` returns 0 hits
- [x] `cargo update -w` regen `Cargo.lock`  → verify: Cargo.lock diff shows only the 4 grex crates bumped 1.3.1 → 1.3.2
- [x] Update any rustdoc on lockfile writer / per-pack-lock module mentioning old path  → verify: cargo doc --no-deps green; broken-intra-doc-link checks pass

### § Phase 2R — Reviewer pass

- [x] cavecrew-reviewer pass on full diff  → verify: 0 findings or all findings addressed
- [x] code-reviewer pass on full diff (security + API surface)  → verify: 0 critical findings; advisory findings logged
- [x] Public API drift check: 0 removed/renamed exports  → verify: rustdoc symbol diff vs v1.3.1 shows no removals on `grex-core::lib.rs` / `grex-cli::lib.rs` public surface (`LockEntry.synthetic` is internal struct field, not pub re-export — confirm)
- [x] Frozen contract check: 0 violations vs `freeze-v1.3.0.md` (cross-check the table in design.md)  → verify: reviewer explicitly cross-checks each of the 13 frozen rows
- [x] No `Co-Authored-By` in commits, PR body, or commit body trailers (rule 13)  → verify: `git log feat-v1.3.2 ^main --format=%B | grep -i 'co-authored-by\|claude\|anthropic'` returns empty
- [x] Worker desync check: W1/W2/W3 write-sets disjoint at file granularity  → verify: reviewer cross-walks the diff; no file appears in 2 workers' touched-set
- [x] Hard-cut B11 audit: NO fallback open-path code  → verify: reviewer audits the 3 lock-open sites and confirms only new path is attempted

## § Phase 3 — Validation gate

- [x] `cargo fmt --check --all` exit 0  → verify: stdout empty
- [x] `cargo build --workspace --all-targets` (debug) exit 0  → verify: no compile errors
- [x] `cargo build --workspace --all-targets --release` exit 0  → verify: no compile errors
- [x] `cargo clippy --workspace --all-targets -- -D warnings` exit 0  → verify: no new lints vs v1.3.1 baseline
- [x] `cargo test --workspace --no-fail-fast` exit 0 (or carry-forward env-only failures documented)  → verify: only `dispatch_parallel` + `pack_type_dispatch` Windows UAC os err 740 carry-forward fails permitted (per v1.3.1 precedent); all 3 new test files pass
- [x] `cargo doc --workspace --no-deps` exit 0  → verify: no broken-intra-doc-link errors
- [x] `lake build` (cwd = `proof/`) exit 0  → verify: 0 sorry, 0 admit, B13 theorem compiles
- [x] `python .omne/scripts/validate.py` exit 0  → verify: all SSOT files clean; openspec triplet frontmatter validates
- [x] `cargo run -p xtask -- gen-man` drift-free  → verify: man-drift --check exit 0; if path strings surface in CLI help, regenerate
- [x] `cargo run -p xtask -- axiom-audit` exit 0 (8 theorems, `[propext]` only or no axioms)  → verify: theorem count = 8 (= 7 from v1.3.1 + 1 from v1.3.2); no new bridge axioms

## § Phase 4 — Ship

- [x] In-process test additions verified (3 new test files: `lockfile_no_synthetic.rs`, `lockfile_under_grex.rs`, `slash_paths.rs`)  → verify: cargo test --tests --workspace shows all 3 in the run list
- [x] Real-smoke baseline confirmed UNCHANGED at 2/13 (gix HTTPS gap orthogonal)  → verify: real-smoke output matches v1.3.1 endpoint baseline byte-for-byte modulo timestamps
- [x] Single squash commit on `feat-v1.3.2`, no `Co-Authored-By` (rule 13)  → verify: git log shows one squash commit; commit body grep for `co-authored-by|claude|anthropic` returns empty
- [x] Push branch, open PR `feat-v1.3.2 → main`  → verify: PR created via `gh pr create`; PR body has no AI/assistant trailer
- [x] CI required checks all green (8 builds × OS, cargo-deny, MCP conformance, man-drift, release-plan, typos)  → verify: `gh pr checks` shows all required green; non-required (code-metrics, real-smoke) advisory only
- [x] Lean4 proof gate green in CI  → verify: lake build job in CI exits 0
- [x] Squash-merge to main  → verify: merge commit appears on main; branch protection allows
- [x] Tag `v1.3.2` annotated, push to origin  → verify: `git tag -l v1.3.2` shows annotated tag; `git push --tags` propagates
- [x] Publish 4 crates in topo order: `cargo publish -p grex-core` → `cargo publish -p grex-mcp ‖ -p grex-plugins-builtin` → `cargo publish -p grex-cli`  → verify: all 4 crates appear at `1.3.2` on crates.io within 5 minutes of publish
- [x] Verify all 4 live on crates.io at `1.3.2`  → verify: `curl -s https://crates.io/api/v1/crates/grex-core | jq '.crate.max_version'` returns `"1.3.2"` (and same for 3 others)

## § Phase 5 — SSOT update bundle (rule 16, MANDATORY before session-complete)

All Phase 5 tasks land in the SSOT repo (`grex-inst`, mounted at `.omne/`). Per rule 7, these never appear in grex's git history. Per rule 16, they MUST land in the SAME session as the ship.

- [x] `cd .omne/` → all subsequent edits in the SSOT working tree  → verify: `pwd` ends in `.omne` and `git remote -v` shows `grex-inst`
- [x] `.omne/history.md`: append per-release entry for v1.3.2 (one line minimum + body)  → verify: history.md has new entry with commit SHA + tag + crates URLs
- [x] `.omne/var/dogfood-findings-v1.3.0.md`: mark B6 / B11 / B13 as RESOLVED in v1.3.2 with commit refs  → verify: bug rows show "RESOLVED v1.3.2 (commit `<sha>`)" + brief fix description
- [x] `.omne/roadmap.md`: shift active backlog (v1.3.3 = B3/B5/B10; v1.3.4 = B1/B9/B15; v1.4.0 unchanged)  → verify: roadmap reflects v1.3.2 as SHIPPED, v1.3.3 as next
- [x] `.omne/lockfile.md`: confirm canonical paths under `.grex/` for the 3 lock artifacts (path move now in runtime, doc was already correct)  → verify: lockfile.md §"File location" + §"Three lock artifacts" reads coherent with new runtime
- [x] `.omne/pack-spec.md`: §v1.2.0 slash-path support — add note "RUNTIME ALIGNED v1.3.2" if helpful  → verify: pack-spec.md no longer reads as forward-looking on slash paths
- [x] `.omne/manifest.md`: if `LockEntry.synthetic` referenced anywhere, mark the field as RETIRED v1.3.2 (writer no longer emits)  → verify: manifest.md `synthetic` references coherent with new runtime
- [x] `.omne/var/freeze-v1.3.0.md`: add `## § v1.3.2 follow-up (2026-05-03)` section mirroring the v1.3.1 follow-up; confirm 13 frozen contracts intact  → verify: freeze-v1.3.0.md has new section asserting intact-status with per-row check
- [x] `.omne/migration-v1.3.2.md`: NEW file with operator migration recipe for B11 stale lock orphans (per discipline rule 16, only required if release introduces operator-visible behavior change — B11 qualifies)  → verify: file exists with one-liner `mv` / `rm` recipe; G1 routing table updated
- [x] `.omne/schemas/rules.md`: G1 routing table — add `cfg/migration-v1.3.2.md` row (hand-edited)  → verify: routing table grep shows new row
- [x] `.omne/INDEX.yaml` regenerated via `python .omne/scripts/build_index.py` (per rule 12)  → verify: build_index.py exit 0; INDEX.yaml shows new migration-v1.3.2 entry + bumped feat-v1-3-2 triplet
- [x] `.omne/schemas/rules.md`: bump `last_updated` if any rule text or G1 routing-table row changed (per rule 16)  → verify: rules.md frontmatter `last_updated: 2026-05-03`
- [x] SSOT pre-commit hook green (`python scripts/validate.py` exit 0 in `.omne/`)  → verify: SSOT validate.py clean across all `.md` frontmatter
- [x] Commit + push SSOT changes (separate working tree per rule 7; no `Co-Authored-By` per rule 13)  → verify: grex-inst main advances; PR if branch protection requires

## § Phase 6 — Endpoint

- [x] Append `## Endpoint (2026-05-03, main — v1.3.2 SHIPPED)` to grex `progress.md`  → verify: progress.md has new endpoint section mirroring the v1.3.1 endpoint structure (state, what shipped, Lean obligations, validation gate, real-smoke status, crates URLs, v1.3.x backlog state, decisions locked)
- [x] Update top `## Where we are` block in `progress.md`: bump to v1.3.2  → verify: top block reflects v1.3.2 as SHIPPED on main, v1.3.3 next
- [x] Commit progress.md update on main (or via PR if branch protection)  → verify: main advances with the endpoint commit
- [x] Session-complete check: rule 16 SSOT update bundle MUST be green BEFORE marking complete  → verify: both grex `progress.md` and `.omne/history.md` reflect v1.3.2 SHIPPED in coherent state
