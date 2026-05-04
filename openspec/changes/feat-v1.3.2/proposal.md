---
slug: feat-v1-3-2
type: spec
status: active
last_updated: 2026-05-03
topic: v1-3-2-patch
depends_on: [feat-v1-3-1, dogfood-findings-v1-3-0, freeze-v1-3-0]
---

# v1.3.2 — contract-drift dogfood patch

## § Why

v1.3.0 dogfood (2026-05-02 against `E:\repos\cfg`) surfaced 15 defects (B1–B15) catalogued in `.omne/cfg/dogfood-findings-v1.3.0.md`. v1.3.1 closed the critical-bundle (B2/B4/B7/B8/B12/B14). v1.3.2 = the **contract-drift bundle** — three bugs where the v1.3.0 runtime ships behavior that disagrees with the locked SSOT contracts in `.omne/cfg/lockfile.md`, `.omne/cfg/manifest.md`, and `.omne/cfg/pack-spec.md`.

Bugs reproduced verbatim from `.omne/cfg/dogfood-findings-v1.3.0.md` (severity preserved):

| # | Severity | Verb / site | Expected | Actual | Fix |
|---|---|---|---|---|---|
| B6  | High         | `LockEntry.synthetic`            | Field retired per pack-spec.md §v1.2.0 (untracked = error)                                                            | `synthetic: true` emitted on 6/7 children post-sync                                                | Stop emitting `synthetic`; runtime to enforce "untracked = error" per spec |
| B11 | **Critical** | Lockfile location                | `.grex-lock`, `.grex.sync.lock`, `.grex-backend-*.lock` under `.grex/` (manifest.md / lockfile.md contract)            | Lockfiles land at workspace root                                                                    | Move lockfile writers under `.grex/` |
| B13 | High         | Nested `child.path`              | Slash-separated paths supported per pack-spec.md §v1.2.0                                                              | Runtime rejects: "path separators not allowed" — runtime BEHIND spec                                | Support slash-separated paths in walker / manifest loader |

## § Scope (3 bugs, drift bundle)

### B6 — retire `LockEntry.synthetic`

`pack-spec.md §v1.2.0` retired sync-time auto-synthesis: encountering `dest/.git` without `dest/.grex/pack.yaml` is an error (collected per Phase 1, reported at end-of-frame). The companion `LockEntry.synthetic` field was meant to be retired at the same time but the writer still emits `synthetic: true` on 6/7 children. Fix is purely subtractive: stop writing the field; if any reader still expects it, switch consumption to `serde::Default` and remove the read site too. SSOT contract (`.omne/cfg/lockfile.md` §`LockEntry` schema, `.omne/cfg/walker.md` §`LockEntry.synthetic` deprecation) is the canonical reference — runtime catches up.

### B11 — lockfile-location migration to `.grex/`

`.omne/cfg/manifest.md` and `.omne/cfg/lockfile.md` both pin the canonical paths under `<meta>/.grex/`. The v1.3.0 runtime still writes the three lock-related artifacts at workspace / pack root. v1.3.2 moves them all under `.grex/`:

| Artifact | Kind | v1.3.0 path (current) | v1.3.2 path (target) |
|---|---|---|---|
| Per-pack pack-lock | OS file mutex | `<pack_workdir>/.grex-lock` | `<pack_workdir>/.grex/.grex-lock` |
| Workspace sync sidecar | OS file mutex | `<workspace>/.grex.sync.lock` | `<workspace>/.grex/.grex.sync.lock` |
| Per-repo backend lock | OS file mutex | `<dest>.grex-backend.lock` (sibling file) | `<dest>/.grex/.grex-backend.lock` |

**Hard-cut readers — no fallback to old workspace-root path.** Matches the v1.3.1 B8 SCHEMA_VERSION hard-cut precedent (no field deployments to migrate; greenfield artifact). Operators with stale workspace-root lock files must either delete-and-resync or move-by-hand; migration recipe in design.md.

### B13 — slash-separated `child.path` in walker / manifest loader

`pack-spec.md §v1.2.0 — declarative nested paths (option c)` says `child.path:` MAY be a relative path with `/` separators (e.g. `tools/foo`, `courses/cpp/cpp-grammar`); each segment must individually satisfy the bare-name regex; absolute paths, leading `..`, escaping symlinks remain rejected. The v1.3.0 runtime rejects ANY `/` in `child.path` with `"path separators not allowed"`. Walker normalisation + manifest-loader gate must accept slash paths, while preserving the maintainer-locked invariant **"the walker never recurses into a folder lacking `.grex/`"** (= unmanaged subdir terminates descent — the safety boundary that prevents infinite descent).

## § Out-of-scope (explicitly deferred)

- B12 no-op shim deletion → v1.4.0 (per `freeze-v1.3.0.md` §"Carry-forward to v1.4.0").
- B1, B3, B5, B9, B10, B15 → v1.3.3 / v1.3.4 per `.omne/cfg/roadmap.md`.
- `--workspace` flag removal → v1.4.0 hard deadline.
- Plugin-API freeze → v1.4.0.
- Smoke-harness `gix` HTTPS feature → orthogonal infra item, separate v1.3.x lane.
- B14 cleanup (unused `lockfile/writer.rs::write_entry` parallel helper) → v1.4.0 (per `freeze-v1.3.0.md`).

## § SemVer verdict

**PATCH (1.3.1 → 1.3.2). Maintainer-locked.**

Per discipline rule 6 (SemVer = maintainer's call), the maintainer has locked this release as PATCH. Reviewer SemVer findings (api-contract, codex passes, etc.) are advisory and MUST surface as questions — not assertions — against the locked verdict. Justification:

- B6 is purely subtractive on a field that the v1.2.0 SSOT contract already retired; no operator-visible behavior changes other than disappearance of an obsolete field that readers have always defaulted.
- B11 is a hard-cut on path location with no field deployments to migrate (per `freeze-v1.3.0.md` §"In-bounds changes that look like freeze touches but are not" precedent — same justification as v1.3.1's B8 SCHEMA_VERSION hard-cut). The lockfile **schema** stays at `v1.2.0` (no schema_version bump). The 13 frozen contracts in `freeze-v1.3.0.md` remain intact (lockfile schema is FROZEN at the v1.2.0 shape; LOCATION is not in the freeze table).
- B13 is the runtime catching up to the v1.2.0 declared `pack-spec.md §v1.2.0` contract — no contract change, runtime alignment only.

Reviewer-flagged SemVer alternatives (e.g. MINOR for B11 path move) MUST surface as questions; the maintainer's PATCH verdict stands unless overridden.

## § Acceptance criteria (per-bug verification)

In-process tests are the regression lock for v1.3.2 — same pattern as v1.3.1 (real-smoke harness blocked on `gix` HTTPS feature, orthogonal infra item).

| Bug | Verification |
|---|---|
| B6  | New test asserts no `synthetic` key emitted in serialized `LockEntry` JSON for any sync output. Reader test asserts deserialization of v1.1.x lockfiles with `synthetic:true` continues to round-trip cleanly via `#[serde(default)]` (forward read, no break). Walker integration test confirms untracked-git encounter still errors per `pack-spec.md §v1.2.0` (re-asserts existing contract). |
| B11 | New test asserts lockfile / sync-lock / backend-lock all materialize under `.grex/` post-sync. Negative test asserts NO file appears at the old workspace-root or pack-root paths. Doctor surface test (if `grex doctor` reports lock paths) shows new path. |
| B13 | New test feeds a manifest with `path: tools/foo` and `path: courses/cpp/cpp-grammar` → walker resolves dest correctly, manifest loader accepts, lockfile keys by slash-path per `manifest.md §v1.2.0 keying`. Negative test asserts `..`, absolute paths, symlink-escapes still reject. Walker invariant test: a slash-path child whose intermediate folder lacks `.grex/` does NOT recurse beyond that folder (unmanaged-subdir invariant — safety boundary). |

## § Lean obligations summary (rule 8 gate)

| Bug | Lean status |
|---|---|
| B6  | Simple-exempt. Pure subtractive serde change on an already-retired field; no algorithm impact on any existing invariant. Justification per rule 8: bug fix confined to writer code, no new behavior, no contract change. |
| B11 | Simple-exempt. Path-only string change in writer — `<root>/.grex-lock` → `<root>/.grex/.grex-lock` etc. No algorithm impact on any locking invariant; the per-pack pack-lock invariant `I8` (concurrency.md), workspace-sync lock fail-fast, and backend-lock per-repo mutual exclusion all hold equally well at the new path. Justification per rule 8: bug fix confined to a single writer module, no invariant impact. |
| B13 | **NOT exempt — Lean theorem REQUIRED.** Walker invariant impact (slash paths participate in cycle-detection's `visited: Vec<String>` and in dest-path resolution); MUST add a theorem before any Rust change lands. Phase 2 deliverable. Phase 1 OpenSpec states the obligation only. |

CI axiom-stability gate extends from **7 theorems** (post-v1.3.1) to **8 theorems** (B13 theorem added). Axiom budget target: unchanged at 9 bridge / 4 types / 0 model.

Theorem name + signature proposed in `design.md §B13 Lean obligation` — the maintainer ratifies the final form before Lean Phase 2a.

## § Behavior contract impact

- **Adds:** none (no new public surface).
- **Removes:** `LockEntry.synthetic` field on serialized JSON output (deserialization tolerates legacy via `#[serde(default)]`); `child.path` slash-rejection error.
- **Changes:** lockfile / sync-lock / backend-lock locations (hard-cut, no fallback). Walker / manifest-loader accept slash paths.
- **Frozen contracts unchanged.** All 13 STABLE/FROZEN contracts in `freeze-v1.3.0.md` remain intact — lockfile schema (FROZEN) location is not in the freeze table; the schema shape itself is unchanged.

## § Open questions for maintainer

1. **B11 path naming.** Inside `.grex/`, retain bare names (`.grex-lock`, `.grex.sync.lock`, `.grex-backend.lock`) or drop the leading dot (since they no longer need to hide at workspace root)? Design.md proposes retaining bare names for diff minimality and hint-to-operator that they are internal lock files; awaiting maintainer confirmation.
2. **B13 theorem signature.** Phase-1 proposal lists three candidate names (see design.md §B13). Maintainer picks the final name + canonical statement before Phase 2a Lean dispatch.
3. **B11 `.grex-backend.lock` placement.** Current sibling-file design (`<dest>.grex-backend.lock` adjacent to `<dest>`) was chosen so the lock survives `<dest>` wipe. Moving inside `<dest>/.grex/` means the lock disappears on a destructive `rm -rf <dest>`. Acceptable per maintainer? Design.md proposes acceptance because v1.2.5 quarantine + v1.3.1 dry-run gates already make destructive wipes cooperative.

## § Risks

- **Real-smoke gix HTTPS gap unchanged.** v1.3.2 verification relies on in-process tests — same posture as v1.3.1. Once the smoke harness lands HTTPS support (separate v1.3.x infra lane), v1.3.2 fixtures gain end-to-end coverage; until then the in-process tests are the regression lock.
- **B11 stale-lockfile orphans.** Operators who upgrade from v1.3.0/v1.3.1 may have stale `<workspace>/.grex.sync.lock` / `<dest>.grex-backend.lock` / `<pack>/.grex-lock` files at old locations. Mitigation: migration recipe in design.md (one-line `rm` / `mv`); doctor advisory could surface them as `Info` finding (deferred to v1.3.3 if scope-tight).
- **B13 cycle-detection edge case.** With slash-path ids, two children at distinct paths sharing the same `name:` (e.g. `tools/foo` + `vendor/foo`) must not collide in the visited set. Path-keyed identity (per `manifest.md §v1.2.0 keying`) already disambiguates; B13 theorem must formalise this.

## § Ship plan

1. OpenSpec triplet (this) + maintainer sign-off on design.md.
2. Lean (1 theorem for B13 → `lake build` green BEFORE Rust).
3. Rust impl (3 parallel workers; B6 + B11 + B13 write-sets must be disjoint per rule 14).
4. Reviewer pass (cavecrew-reviewer + code-reviewer parallel).
5. Validation gate (fmt/build/clippy/test/doc/lake/validate.py/man-drift/axiom-audit at 8 theorems).
6. In-process test additions (regression lock).
7. Commit on `feat-v1.3.2` (no Co-Authored-By per rule 13), PR → main, squash-merge, tag `v1.3.2`.
8. Publish 4 crates topo: `grex-core → (grex-mcp ‖ grex-plugins-builtin) → grex-cli`.
9. SSOT update bundle per rule 16 (history.md, dogfood-findings-v1.3.0.md, roadmap.md, lockfile.md path note, pack-spec.md slash confirmation, manifest.md if `synthetic` referenced, freeze-v1.3.0.md `## § v1.3.2 follow-up` section, INDEX.yaml regen).
