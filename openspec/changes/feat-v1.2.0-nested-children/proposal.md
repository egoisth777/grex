# feat-v1.2.0-nested-children — parent-relative dest + distributed lockfile + nested children at any depth

**Status**: draft
**Milestone**: v1.2.0
**Depends on**: v1.1.1 (SHIPPED 2026-04-28 — synthetic scripted-no-hooks packs for plain-git children, all 4 crates live on crates.io)

## Why

1. **The locked positioning ("nested meta-repo manager") promises *nested* trees, not flat siblings.** v1.1.0 unlocked flat-sibling layout; v1.1.1 unlocked plain-git children. Both still resolve every `children[].path` against a *single workspace anchor* — every pack is materially a sibling under the root. Real meta-pack hierarchies need parent-relative resolution: a meta declares a child at `apps/web/`, the dest lands at `<current-meta>/apps/web/`, and that child can itself be a meta with its own `children:` resolved against *its* directory. The flat layout is the v1.1.x ceiling.
2. **User-stated motivation: "packs can live wherever in parent."** Real workspace evidence: `E:\repos` has 4 levels of nesting today (`utils/grex-org/grex/`), each level is its own logical meta with its own children. v1.1.x cannot model this without flattening into a synthetic top-level workspace, which loses the per-meta authority every intermediate node should have over its own subtree.
3. **The single workspace lockfile is the wrong unit.** With every meta resolving children against itself, the natural lockfile boundary becomes per-meta: each `<meta>/.grex/grex.lock.jsonl` tracks its *direct children only*. A sub-meta's lockfile is its own ground truth — a parent meta does not own grandchildren state. This mirrors how real meta-repos delegate (`.scripts/sync.py` recurses into child `.scripts/sync.py`).
4. **Synthesis is a v1.1.1 stop-gap that contradicts the new model.** v1.1.1's synthetic-pack fallback writes nothing on disk — the user has no record of what grex declared. v1.2.0 makes the manifest the source of truth: untracked `.git/` directories are an *error* surfaced at sync time, with the offending paths listed and a one-line fix (`grex add <path>`). This is the same contract that `cargo` enforces for unregistered workspace members.

## What changes

A single coherent migration from v1.1.x's *workspace-anchored, single-lockfile, synthetic-tolerant* model to a *parent-relative, distributed-lockfile, manifest-strict* model. The five locked decisions:

1. **Parent-relative resolution.** `dest = current_meta.join(child.path)`. No global workspace anchor. The recursion entry is the cwd of the CLI invocation. `child.path` may carry forward slashes (`apps/web`), but `..`, absolute paths, symlinks crossing the parent boundary, Windows junctions, NTFS reparse points, gitfile `.git` files, Unicode-NFC duplicates, and Windows-special segments (`:`, `$`, `~digit`) are rejected.
2. **Distributed lockfile (β).** Each meta has its own `<meta>/.grex/grex.lock.jsonl` tracking direct children only. Sub-meta lockfiles are autonomous — the parent does not read or write them. `LockEntry` gains an optional `path: Option<String>` field (relative-to-meta) with read-time fallback for v1.1.x entries that lack it.
3. **Cargo-style parallel.** Siblings within one meta walk in parallel; sub-meta recursion ALSO runs in parallel with sibling work. Ordering is dependency-driven, not depth-driven.
4. **Synthesis retired at sync time.** Untracked `.git/` directories under a declared meta are aggregated into a single `TreeError::UntrackedChildren { paths: Vec<PathBuf> }` raised at the end of the walk. Each entry includes a one-line fix: `grex add <path>`. `LockEntry.synthetic: bool` from v1.1.1 stays in the schema for backward-compat reads but is dead under v1.2.0 (always `false` for newly written entries).
5. **6 fundamental verbs, recursive by default.** `grex add`, `grex sync`, `grex remove`, `grex doctor` (recursive default + `--shallow` opt-out), `grex ls`, `grex init`. `doctor` walking is the same walker as `sync`; `ls` renders the parent-relative tree using each meta's own lockfile.

## Acceptance criteria

Each criterion is a test-driven goal: when the user runs the command, the system produces the stated outcome, verified by the named test.

1. **Parent-relative resolution.** A 3-level fixture (`root/` → `root/apps/web/` → `root/apps/web/services/api/`) where each meta declares the next level as a child via `apps/web` / `services/api` walks end-to-end. Dest at every level resolves against its own meta, not the root. *Verified by* `crates/grex/tests/nested_children_walk.rs`.
2. **Distributed lockfile isolation.** After a sync of the 3-level fixture, `root/.grex/grex.lock.jsonl` lists only `apps/web` (one entry); `root/apps/web/.grex/grex.lock.jsonl` lists only `services/api` (one entry); `root/apps/web/services/api/.grex/grex.lock.jsonl` is either absent or empty (leaf). No lockfile contains entries for grandchildren. *Verified by* `crates/grex-core/tests/distributed_lockfile.rs`.
3. **Sub-meta autonomy.** Running `grex sync root/apps/web/` (cwd at the sub-meta, NOT at the root) walks `apps/web`'s subtree only, writes `apps/web/.grex/grex.lock.jsonl`, and does not touch `root/.grex/grex.lock.jsonl`. *Verified by* `crates/grex/tests/sub_meta_autonomy.rs`.
4. **Untracked-children error aggregation.** A meta with 3 declared children + 2 undeclared `.git/` directories produces `TreeError::UntrackedChildren` listing both undeclared paths, with each line carrying the suggested `grex add <path>` fix. Exit code is non-zero. *Verified by* `crates/grex-core/tests/untracked_children_error.rs`.
5. **Validator rejects.** Each of `..`, absolute paths, symlinks crossing the parent boundary, Windows junctions, NTFS reparse points, gitfile `.git` files, Unicode-NFC duplicate names, and segments containing `:` / `$` / `~<digit>` produces a typed `TreeError` variant at validate-time, before any walk happens. *Verified by* `crates/grex-core/tests/validator.rs` parameterised cases.
6. **Cargo-style parallel.** A meta with 4 sibling sub-metas, each declaring 4 leaf children, completes in roughly the time of the slowest sibling chain (not the sum), and the per-meta fd-locks prevent two walkers from writing the same lockfile concurrently. *Verified by* `crates/grex-core/tests/parallel_scheduler.rs` (timing-tolerant; uses `Barrier`).
7. **Cleanup.** A child removed from a parent's manifest causes the next CLI command (sync, doctor, or remove) to `rm -rf` the dest *and* prune the corresponding lockentry, with a recursive consent walk if the dest is itself a sub-meta (cannot prune a sub-meta with dirty children — surfaces a fresh error variant). *Verified by* `crates/grex/tests/cleanup_consent.rs`.
8. **Idempotency.** A second sync immediately after the first exits 0 with byte-identical lockfiles at every level. *Verified by* `crates/grex/tests/nested_children_walk.rs::idempotent_resync`.
9. **Termination.** Cycles (a meta declaring itself as a transitive child) and unreachable-but-cyclic subgraphs are detected pre-walk; the walker is total. *Verified by* the existing cycle-detection tests + new nested-cycle case in `crates/grex-core/tests/cycle_detection.rs`.
10. **Migration: v1.1.x lockfile readable.** A workspace produced by v1.1.1 (single `grex.lock.jsonl` at root, no `path` field on entries) reads cleanly under v1.2.0: missing `path` falls back to `entry.id` as the relative path; `synthetic: true` entries are accepted but trigger an upgrade-advisory log line. *Verified by* `crates/grex-core/tests/lockfile_v1_1_compat.rs`.
11. **Doctor recursive default.** `grex doctor` from a meta walks the full subtree by default; `grex doctor --shallow` walks one level only. Both modes share the same validator and surface the same error variants as `sync`. *Verified by* `crates/grex/tests/doctor_recursion.rs`.
12. **8 invariants Lean-proven.** `lean/Grex/Walker.lean` builds clean under `lake build` with the 8 invariants (boundary preservation, distributed isolation, termination, idempotency, sub-meta autonomy, no-untracked, cleanup safety, concurrency safety) carried as theorems. 4 bridge axioms link the Lean abstract walker to the Rust implementation; bridge axioms documented in `lean/Grex/Bridge.md`. *Verified by* the lake-build CI gate.

## SemVer rationale

**MINOR** (1.1.1 → 1.2.0). Justification:

- **Maintainer override of reviewer's MAJOR call.** Two R2 reviewers flagged the change as MAJOR on the basis that the workspace anchor semantics change. Maintainer ruling: every observable behaviour delta is *additive* under SemVer — existing pack.yaml stays valid (no field removed, no field semantics changed), existing lockfiles read clean (with the documented `path` fallback), `LockEntry` gains an optional field (additive), the `workspace` API field stays in place with a `cwd_meta` alias added (deprecation, not removal). Under strict SemVer, additive-with-deprecation is MINOR.
- **`pack.yaml` schema unchanged.** Same fields, same validation regex, same children-list semantics. The `path:` regex relaxation (allow `/`) is purely additive — every v1.1.x path is still valid.
- **Lockfile schema additive.** `LockEntry.path: Option<String>` with `#[serde(default)]`. v1.1.x lockfiles deserialize cleanly; v1.2.0 lockfiles deserialize into a v1.1.x reader as long as the v1.1.x reader does not set `deny_unknown_fields` (it does not).
- **`SyncOptions::workspace` and `with_workspace()` retained.** Adds `cwd_meta` as a synonym; `workspace` deprecated with `#[deprecated(since = "1.2.0", note = "use cwd_meta")]`. No removal until 2.0.
- **MCP `SyncParams.workspace` and `LsResponse.workspace` keep field names.** Value semantics shift from "workspace anchor" to "cwd meta entry point" — documented in the MCP changelog. Tool-schema diff is zero; no breaking client changes.

A future MAJOR (2.0) is the right place to *remove* `workspace`, drop v1.1.x lockfile compat, retire the `synthetic` field, and require `cwd_meta` everywhere. v1.2.0 stays additive.

## Cross-references

- **Canonical algorithm**: `.omne/cfg/walker.md` — the parent-relative walker pseudocode (single source of truth; this proposal lifts the algorithm verbatim into [`design.md`](./design.md)).
- **Lean proof**: `lean/Grex/Walker.lean` (368 lines; `lake build` clean; 8 theorems + 4 bridge axioms).
- **Rust mechanism decisions**: `openspec/changes/feat-v1.2.0-nested-children/rust-design-decisions.md` — sibling SSOT being authored in parallel by the rust-expert subagent. Contains TOCTOU mitigation crate selection, fd-lock layering, error-variant surface, and concurrency primitive choices. Referenced from [`design.md`](./design.md) "Open questions" section.
- **History context**: `.omne/cfg/history.md` — M1 through v1.1.1 evolution; v1.2.0 closes the "nested meta-repo manager" promise that has been latent since M1.
- **R2 review aggregation**: 9-reviewer round produced 12 deduped BLOCKERs and ~25 non-blocking CONCERNs. BLOCKERs are tracked in [`design.md`](./design.md) "Open questions"; CONCERNs are routed to in-flight fix agents.
- **Pack-template**: external `pack-template` repo (seeded at v1.0.0) gets a v1.2.0 update demonstrating nested-children layout. PR planned post-merge.
