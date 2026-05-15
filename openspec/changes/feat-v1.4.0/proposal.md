---
slug: feat-v-1-4-0
type: spec
status: active
last_updated: 2026-05-15
topic: stub-verb-completion
depends_on: [feat-v1.3.3]
---

# feat-v1.4.0 — basic-actions completion (B9 + 6 stub verbs + B15)

## § Why

Six v1 CLI verbs — `init`, `rm`, `update`, `status`, `run`, `exec` — still ship as M1 scaffolds. They print `"grex <verb>: unimplemented (M1 scaffold)"` and exit `0`, which is the v1.3.x dogfood backlog item **B9** ("stub verbs exit non-zero"). Operators hit the stubs in CI matrices and see green where a hard failure was intended.

The `feat-grex` charter (success criterion #1) enumerates `init / add / rm / sync / update / doctor / import` as table stakes for v1, plus `ls / status / run / exec / serve` from criterion #2. `add / sync / doctor / import / ls / serve / teardown / migrate-lockfile` are fully wired. The six stubs are the remaining gap.

A secondary v1.3.x backlog item **B15** lives on `add`: registering a pack whose path collides with an already-tracked pack silently overwrites the prior entry. Both items are additive UX fixes — no breaking surface, no proof-of-concept retirement. Together they justify a MINOR bump (maintainer call per `inst/schemas/conduct/rules.md` rule 6).

## § Proposal

Ship seven behavior changes inside one release. SemVer = MINOR (no contract change to wired verbs; new behavior for previously-stub verbs counts as additive).

### V1 — `grex init [<path>]` — workspace bootstrap

Initialize a grex workspace at `<path>` (default cwd). Creates `<path>/.grex/pack.yaml` with the minimal meta-pack manifest (`schema_version: "1"`, `pack_type: meta`, empty `actions` + `children`). Idempotent: if `pack.yaml` already exists, exit `1` with `"grex init: <path> already initialized"` and do not overwrite.

### V2 — `grex rm <path>` — pack removal

Tear down the pack at `<path>` (existing `sync::teardown` lifecycle) then delete the directory. Refuses to remove a meta-pack that still has children unless `--force`. Exit codes mirror sync (1 validation, 2 exec, 3 tree).

### V3 — `grex update [<pack>]` — refresh + re-install

Behaviorally equivalent to `sync` for v1.4.0 (the existing `sync` pipeline already re-runs install actions on lockfile delta — see `grex_core::sync::run`). `update` becomes a documented alias that forwards to the sync handler with `force: false`. Future v2 may diverge if "pull-then-install" needs distinct lifecycle hooks.

### V4 — `grex status [<pack_root>]` — drift vs lockfile

Walks the pack tree, runs the sync pipeline with `dry_run = true`, prints per-pack drift (`clean` / `would-update <action_count>` / `missing`). Never mutates state. `--json` emits `{"verb":"status","packs":[{"path":..., "state":...}]}`. Exit codes: 0 clean, 0 with drift (drift is informational not error), 2 tree/walk error.

### V5 — `grex run <action> [<pack_root>]` — named action across packs

Walks the pack tree, filters each pack's `actions:` by `name == <action>` (string match), executes matched actions via the standard `PlanExecutor`. Exit codes mirror sync (1 validation, 2 exec, 3 tree). Empty filter (no pack has a matching action) exits `0` with `"grex run: no packs declare action <name>"` and a non-zero count under `--json`.

### V6 — `grex exec [--] <cmd>...` — shell in pack context

Resolves pack root via the same cwd-default helper used by `sync` / `teardown`. Spawns `<cmd>` with cwd = pack root, inheriting parent stdio (no JSON envelope by default — `--json` captures stdout/stderr and emits a single doc on exit). Exit code = child exit code, capped at 125 to keep the 1/2/3 sync band reserved for grex-side failure.

### V7 — `grex add` path-collision warn (B15)

Before invoking `add_pack`, scan the parent manifest for an existing entry whose `path` field collides with the requested target. When a collision is detected, emit a single `warning:` line to stderr (`grex add: path '<p>' already registered to <url>`) and exit `1` without mutating state. `--json` callers receive `{"verb":"add","error":{"kind":"path_collision","existing_url":...,"path":...}}`.

## § Non-goals

- No new pack-types or actions.
- No MCP-surface additions (the six newly-wired verbs are already exposed as MCP tools via `serve`'s shared dispatcher; no change to the tool list).
- No Lean4 theorem additions (rule 8 simple exemption — pure CLI plumbing over existing verified primitives).
- No `--workspace` removal. That stays a v2.0.0 item per the existing freeze.
- No plugin-API freeze. That stays v1.5.0+ scope.

## § Success criteria

1. All 6 previously-stub verbs return a real exit code under both `--json` and human modes — never `Ok(())` with `"unimplemented"`.
2. `cargo test --workspace` passes (no regressions in the 463 existing core tests + ~140 CLI/integration tests).
3. Each new verb has at least one CLI integration test under `crates/grex/tests/` using `assert_cmd`.
4. B15 collision warn covered by a new test under `crates/grex/tests/add_cli.rs` (or sibling).
5. CHANGELOG.md gains a `## [1.4.0] — 2026-05-15` block with the seven items.
6. SSOT `inst/grad/progress.md` gains a `v1.4.0 SHIPPED` endpoint after merge (separate SSOT commit per rule 7).
