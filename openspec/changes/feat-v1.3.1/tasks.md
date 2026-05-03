---
slug: feat-v1-3-1-tasks
type: spec
status: active
last_updated: 2026-05-02
topic: cli
depends_on: [feat-v1-3-1, feat-v1-3-1-design]
---

# v1.3.1 — tasks

Phase-by-phase checklist. Each task = one checkbox `- [ ]`. Mark `[x]` when complete.

## § Phase 0 — OpenSpec (this triplet)
- [ ] proposal.md drafted + frontmatter validates
- [ ] design.md drafted + frontmatter validates
- [ ] tasks.md drafted + frontmatter validates
- [ ] cavecrew-reviewer pass on triplet, no findings

## § Phase 1 — Lean (rule 8 gate)
- [ ] T1 `Grex.Walker.dry_run_no_side_effects` written in `proof/Grex/Walker.lean`
- [ ] T2 `Grex.Lockfile.lockfile_branch_mirrors_manifest_ref` written in `proof/Grex/Lockfile.lean`
- [ ] `lake build` green (exit 0, 0 sorry, 0 admit)
- [ ] Axiom audit: 7 theorems on `[propext]`/no-axioms; budget 9/4/0 unchanged
- [ ] Reviewer pass on Lean diff

## § Phase 2 — Rust impl (parallel workers, disjoint write-sets per rule 14)

Bundle W2+W4 into single worker (events.rs shared). 5 workers total.

### W1 — B2 cwd default pack root
- [ ] `crates/grex/src/cli/verbs/sync.rs`: cwd-default pack_root logic
- [ ] new test: `crates/grex/tests/cli_cwd_default.rs`
- [ ] real-smoke fixture `t_b02` flips green

### W2+W4 — B4 dry-run gate + B8 events schema v2
- [ ] `crates/grex-core/src/tree/walker.rs`: gate clone path + FS writes behind `if !ctx.dry_run`
- [ ] `crates/grex-core/src/audit/events.rs`: schema_version 2 hard-cut, `id` field, `ref` field, drop `pack`. Add `Event::DryRunWouldClone` variant
- [ ] new test: `crates/grex-core/tests/walker_dry_run.rs`
- [ ] new test: `crates/grex-core/tests/events_schema_v2.rs`
- [ ] real-smoke fixtures `t_b04`, `t_b08` flip green

### W3 — B7 tracing → stderr + op Display
- [ ] `crates/grex/src/cli/tracing_init.rs`: stderr writer
- [ ] `crates/grex-core/src/op.rs`: `impl Display for Op`
- [ ] extend `crates/grex/tests/tracing_to_stderr.rs`: stdout has 0 WARN lines; op name not Discriminant
- [ ] real-smoke fixture `t_b07` flips green

### W5 — B12 kill .gitignore mutation + doctor advisory
- [ ] `crates/grex-core/src/tree/gitignore.rs`: DELETE auto-write fn + callsites
- [ ] `crates/grex-core/src/doctor/findings.rs`: new finding `ParentGitTracksPackContent` (severity Info)
- [ ] new test: `crates/grex-core/tests/doctor_advisory.rs`
- [ ] new test: `crates/grex/tests/sync_no_gitignore_write.rs`
- [ ] real-smoke fixture `t_b12` flips green

### W6 — B14 lockfile branch carry
- [ ] `crates/grex-core/src/lockfile/writer.rs`: carry `manifest.ref` → `LockEntry.branch`
- [ ] new test: `crates/grex-core/tests/lockfile_branch_carry.rs`
- [ ] real-smoke fixture `t_b14` flips green

### Cross-cut
- [ ] All 4 crate Cargo.toml: version `1.3.0` → `1.3.1`
- [ ] `cargo update -w` regen Cargo.lock
- [ ] `crates/real-smoke/src/fixtures/`: 6 fixture expectations flipped FAIL → PASS
- [ ] `.omne/cfg/manifest.md`: schema_version 2 spec (SSOT, separate repo)
- [ ] `.omne/cfg/migration-v1.3.1.md`: B12 migration notice (SSOT, separate repo)

## § Phase 2R — Reviewer pass
- [ ] cavecrew-reviewer pass on full diff
- [ ] code-reviewer pass on full diff (security + API surface)
- [ ] Public API drift check: 0 removed/renamed exports
- [ ] Frozen contract check: 0 violations vs `freeze-v1.3.0.md`
- [ ] No `Co-Authored-By` in commits (rule 13)

## § Phase 3 — Validation gate
- [ ] `cargo fmt --check --all` exit 0
- [ ] `cargo build --workspace --all-targets` (debug) exit 0
- [ ] `cargo build --workspace --all-targets --release` exit 0
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` exit 0
- [ ] `cargo test --workspace --no-fail-fast` exit 0 (or carry-forward env-only failures documented)
- [ ] `cargo doc --workspace --no-deps` exit 0
- [ ] `lake build` (cwd=`proof/`) exit 0
- [ ] `python scripts/validate.py` exit 0
- [ ] `cargo xtask man-drift --check` exit 0 (regen if needed)
- [ ] `cargo xtask axiom-audit` exit 0 (7 theorems, [propext]/no-axioms)

## § Phase 4 — Real-smoke flip-green
- [ ] `cargo run -p real-smoke -- --suite all --json` outputs 8 pass / 7 fail
- [ ] CI workflow `real-smoke.yml` outputs 8 pass / 7 fail
- [ ] Local + CI outputs byte-match (excluding timestamps/paths)
- [ ] Pass set verified: `t_b01, t_b02, t_b04, t_b07, t_b08, t_b09, t_b12, t_b14`

## § Phase 5 — Ship
- [ ] Single squash commit on `feat-v1.3.1`, no `Co-Authored-By` (rule 13)
- [ ] Push branch, open PR `feat-v1.3.1 → main`
- [ ] CI required checks all green (8 builds × OS, cargo-deny, MCP conformance, man-drift, release-plan, typos)
- [ ] Lean4 proof gate green
- [ ] Squash-merge to main
- [ ] Tag `v1.3.1` annotated, push to origin
- [ ] Publish 4 crates topo order: `cargo publish -p grex-core` → `cargo publish -p grex-mcp` ‖ `cargo publish -p grex-plugins-builtin` → `cargo publish -p grex-cli`
- [ ] Verify all 4 live on crates.io at `1.3.1`

## § Phase 6 — Endpoint
- [ ] Append `## Endpoint (2026-05-02, main — v1.3.1 SHIPPED)` to `progress.md`
- [ ] Update top `## Where we are` block: bump to v1.3.1
- [ ] Update `.omne/cfg/dogfood-findings-v1.3.0.md`: mark B2/B4/B7/B8/B12/B14 as RESOLVED in v1.3.1 (SSOT, separate repo)
- [ ] Update `.omne/cfg/roadmap.md`: B11 → v1.3.2 (SSOT, separate repo)
