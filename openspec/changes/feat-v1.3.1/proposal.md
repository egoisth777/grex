---
slug: feat-v1-3-1
type: spec
status: active
last_updated: 2026-05-02
topic: cli
depends_on: [dogfood-findings-v1-3-0, freeze-v1-3-0]
---

# v1.3.1 — critical-bundle dogfood patch

## § Why

v1.3.0 dogfood (2026-05-02 against `E:\repos\cfg`) surfaced 15 defects (B1–B15) catalogued in `.omne/cfg/dogfood-findings-v1.3.0.md`. v1.3.x patch series fixes them in tiered batches. v1.3.1 = critical bundle.

## § SemVer

PATCH. Strictly additive + bug-only. Exit codes, JSON envelopes, lockfile schema, MCP method shapes, manifest event log v1 → v2 hard-cut (no field deployments per maintainer 2026-05-02; no back-compat shim needed).

## § Scope (6 bugs)

| Bug | Severity | Site | Fix summary |
|---|---|---|---|
| B2  | Medium   | `grex sync` cwd default | When `.grex/pack.yaml` present in cwd, default `pack_root = cwd` instead of `<pack_root> required` error. |
| B4  | Critical | `grex sync --dry-run`   | Gate clone path AND filesystem write behind `if !dry_run`. v1.3.0 performs real network clones in dry-run mode (spec violation). |
| B7  | High     | `tracing::warn!`        | Route warnings to stderr (currently leak to stdout, breaks JSON consumers). Format op name via `Display`, not `Debug` discriminant. |
| B8  | High     | events.jsonl schema     | Hard-cut to schema_version 2: emit `id` (= folder name = repo name, applies meta-pack and single pack uniformly), `ref` (manifest ref), `schema_version: 2`. Drop legacy `pack` field. No back-compat (no deployed v1.2.x logs in field). |
| B12 | Critical | `grex sync` .gitignore  | REMOVE auto-mutation entirely. `grex sync` does not write to `.gitignore`. `grex doctor` adds advisory finding when sub-pack content visible to parent git index. |
| B14 | Critical | Lockfile `branch`       | `LockEntry.branch` carries `manifest.ref:` value. v1.3.0 emits `branch: ""` for every entry despite manifest specifying `ref:`. |

## § Deferred (NOT in this release)

- B11 (lockfile path move `.grex-lock` → `.grex/grex.lock`) → v1.3.2 with B6, B13 (contract drift bundle). Reason: maintainer chose to defer.
- B1, B3, B5, B6, B9, B10, B13, B15 → v1.3.2/3/4 per `.omne/cfg/roadmap.md`.

## § Lean obligations (rule 8 gate)

2 new theorems must compile (lake build green, 0 sorry, 0 admit, axiom budget 9/4/0 unchanged):

- `Grex.Walker.dry_run_no_side_effects` — formalizes B4 invariant: `dry_run = true ⇒ no network call, no FS write, audit emits dry-run event only`.
- `Grex.Lockfile.lockfile_branch_mirrors_manifest_ref` — formalizes B14 invariant: `forall child, LockEntry.branch(child) = manifest.ref(child)`.

Other 4 bugs (B2, B7, B8, B12) = simple exempt (CLI surface, log routing, schema rename, removal of side effect).

CI axiom-stability gate extends to **7 theorems** (5 from v1.2.6 + 2 from v1.3.1).

## § Real-smoke regression-lock contract

Real-smoke harness `crates/real-smoke/` (v1.3.0 baseline 2 pass / 13 fail). v1.3.1 must flip exactly these 6 fixtures green: `t_b02, t_b04, t_b07, t_b08, t_b12, t_b14`. Local + CI workflow `real-smoke.yml` outputs MUST match.

Post-v1.3.1 baseline: 8 pass / 7 fail.
Pass set: t_b01, t_b02, t_b04, t_b07, t_b08, t_b09, t_b12, t_b14.
Fail set: t_b03, t_b05, t_b06, t_b10, t_b11, t_b13, t_b15.

## § Behavior contract impact

- Adds: `events.jsonl schema_version=2` field (hard-cut). doctor advisory finding for parent-gitignore visibility.
- Removes: `grex sync` writes to `.gitignore` (was undocumented behavior, treated as bug; not part of frozen 13 contracts).
- Changes: `grex sync` cwd default behavior. Lockfile `branch` field now non-empty.
- Frozen contracts unchanged (per `.omne/cfg/freeze-v1.3.0.md`).

## § Open questions resolved (this release)

- Q1 lockfile path: deferred to v1.3.2 (PATCH, no shim needed since deferred).
- Q2 events rename: hard-cut, no field deployments.
- Q3 .gitignore: kill auto-mutation entirely (operator owns the file). doctor reports advisory.

## § Risks

- B12 removal may surprise dogfood operators who relied on auto-add. Mitigation: `grex doctor` advisory finding + migration note in `.omne/cfg/migration-v1.3.1.md` (added in this release).
- B8 schema_version bump may surprise readers. Mitigation: writers pin schema_version=2; readers v1.3.1+ accept v2 only. v1.2.x readers will not consume v1.3.1 logs (acceptable per maintainer).

## § Ship plan

1. OpenSpec triplet (this).
2. Lean (2 theorems → lake build green).
3. Rust impl (5 parallel workers (W2+W4 bundled on events.rs), disjoint files).
4. Reviewer pass (cavecrew-reviewer + code-reviewer).
5. Validation gate (fmt/build/clippy/test/doc/lake/validate.py/man-drift/axiom-audit).
6. Real-smoke flip-green (local + CI).
7. Commit on `feat-v1.3.1` (no Co-Authored-By per rule 13), PR → main, squash-merge, tag `v1.3.1`.
8. Publish 4 crates topo: `grex-core → (grex-mcp ‖ grex-plugins-builtin) → grex-cli`.
