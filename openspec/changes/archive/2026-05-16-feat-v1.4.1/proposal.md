---
slug: feat-v-1-4-1
type: spec
status: backfilled
last_updated: 2026-05-16
topic: smoke-test-bug-bundle
depends_on: [feat-v1.4.0]
---

# feat-v1.4.1 — cfg-metarepo smoke-test bug bundle + offline real-smoke harness

> **Backfilled 2026-05-16, post-implementation.** This proposal was authored AFTER the implementation landed
> on the `v1.4.1` branch (PR #80, all CI green). It exists to satisfy SSOT rule 6 and document the decisions
> taken; it is NOT a pre-implementation alignment artifact. The maintainer authorized the backfill on
> 2026-05-16 alongside the four other rule remediations (see `inst/grad/progress.md` § Cold-boot
> remediation pickup).

## § Why

A user-driven smoke test on 2026-05-16 attempted to convert the script-managed `cfg/` metarepo (six config
sub-repos bucketed by platform `cmn / win / lnx / mac`) into a grex workspace via the v1.4.0 CLI surface
documented in the README. **Seven distinct bugs landed in one pass**, and the convertible state of `cfg/`
turned out to be entirely silent — `grex add` and `grex import` both reported success while the resulting
workspace was inoperable.

| # | Severity | Symptom |
|---|----------|---------|
| 1 | SEV-1 | `add` / `import` write `Event::Add` to `.grex/events.jsonl` but leave `.grex/pack.yaml` untouched. `sync` / `ls` / `status` walk `pack.yaml.children` only — every newly-registered pack is invisible to them. |
| 2 | SEV-1 | `add` / `import` print `"added <path>"` even though the operation has no observable effect on the workspace beyond the audit log. Success message is misleading. |
| 3 | SEV-2 | `grex import` silently drops the `platform:` field in `REPOS.json`. `cmn` / `win` / `lnx` / `mac` layout collapses to flat root paths. No warning. |
| 4 | SEV-2 | After import, `grex doctor` reports every imported pack as `registered pack dir missing` because composed paths don't exist on disk. |
| 5 | SEV-2 | `doctor` folds `events.jsonl` while `ls` / `sync` / `status` walk `pack.yaml.children` — two views of "what's registered" in one binary. |
| 6 | SEV-3 | `grex status` undercounts post-import (root-only line). |
| 7 | SEV-3 | `import --help` claims "Import legacy REPOS.json" implying complete migration; in practice only audit events land. |

Independently, the GitHub-fixture-driven `real-smoke` workflow has been red on every nightly run since
**2026-05-07** — three distinct root causes block 12 of the 15 dogfood regression tests
(`t_b01`–`t_b15`):

- **gix lacked an HTTPS transport.** Workspace pin enables `blocking-network-client` but not
  `blocking-http-transport-reqwest-rust-tls`. Every fixture clone over `https://github.com/...` explodes
  with `'https' is not compiled in. Compile with the 'http-client-curl' or 'http-client-reqwest' cargo
  feature`. Blocks b06/b07/b08/b11/b12/b13/b14/b15.
- **`grex sync --dry-run` panicked on never-synced meta-packs.** The sync orchestrator runs `sync_meta`
  (Phase 1 records would-clone for missing dests on dry-run, no FS mutation) and THEN `build_graph` to
  produce a planner-ready tree. `build_graph` insisted on loading every declared child's manifest, so the
  very first un-cloned child surfaced `ManifestNotFound`. Blocks b02/b03/b04.
- **Fixture content out of date.** The fixture repos (`grex-test-leaf`, `grex-test-meta-flat`,
  `grex-test-meta-nested`) carry pack.yaml content that pre-dates the v1.2.0 strict-name-gate and v1
  schema freeze (`type: pack`, child paths whose basename doesn't match the referenced child's `name:`).
  Blocks b06/b08/b09/b10/b11/b12/b13/b14/b15.

The user's authorization for this release was an autonomous `/goal Yes, no v1.5.x to land fixes, I want
you to resolve all up-to-date tech-debt and bugs and the real-smoke test issues in v1.4.1, no human in
the loop, no permission asked; All PR should pass and all real-smoke test should also pass.`

## § Proposal

Ship a single PATCH release closing the seven smoke-test bugs, unblocking the online real-smoke regression
suite end-to-end, and adding the **offline `real-smoke` framework** the user requested so the bug class
that surfaced today is regression-tested on every CI run going forward (not gated on the manual
`real-smoke` PR label).

SemVer = **PATCH** (per rule 6). All changes are additive: new `pack.yaml` writer module, optional
`BuildOptions` knob on the existing public `build_graph` entry point (forwarder preserves the v1.4.0
signature), new `AddError::PackYaml` / `ImportError::PackYaml` variants on enums already marked
`#[non_exhaustive]`, additive `pack_yaml_updated: bool` field on the existing `#[non_exhaustive]`
`AddReport`. CLI surface unchanged; verb help text expanded but every existing arg parses
byte-identically.

### B1 — `add`/`import` bridge to `pack.yaml.children`

New module `crates/grex-core/src/pack/yaml_writer.rs` performs a raw `serde_yaml::Value` round-trip so
`pack.yaml` is preserved verbatim except for the appended `children:` entry. Comments are lost (serde_yaml
0.9 limitation, documented in the module header). Auto-creates `pack.yaml` from the minimal v1 skeleton
when absent. Idempotent on duplicate path. Hooked into `add_pack` (`crates/grex-core/src/add.rs`) and
`import_from_repos_json` (`crates/grex-core/src/import.rs`).

### B2 — `pack_yaml_updated` boolean

Additive bool on `AddReport`; surfaced in `grex add --json` envelope so machine consumers can distinguish
"appended" from "already present" without re-reading `pack.yaml`.

### B3 — REPOS.json `platform:` honored

`RawEntry` deserializes `platform: Option<String>`. `compose_path(entry)` returns `<platform>/<path>` when
platform is set and non-blank, otherwise the bare path. The composed path runs through the existing
`pack::validate::child_path::reject_reason` gate, so each segment still satisfies the `^[a-z][a-z0-9-]*$`
regex.

### B4 — `doctor` consults `.gitignore`

`crates/grex-core/src/doctor/mod.rs::read_gitignore_top_level` reads the workspace `.gitignore` and skips
any top-level directory whose name matches a literal (non-wildcard, non-nested) ignore line. Closes the
v1.3.0 dogfood B5 ("doctor consults `.gitignore`").

### B5 — `doctor` stdout/stderr split by severity

Doctor's `print_table` routes `OK` rows + the header to stdout, `WARN` / `ERROR` rows to stderr. Closes
v1.3.0 dogfood B7 ("warn lands on stderr"). `--json` mode unchanged.

### B6 — gix workspace pin gains HTTPS transport

`gix = { ..., features = [..., "blocking-http-transport-reqwest-rust-tls"] }` so the workspace can clone
from `https://` URLs. The transient `webpki-roots` license `CDLA-Permissive-2.0` added to `deny.toml`
allowlist.

### B7 — `build_graph` synthesizes placeholders for un-cloned children under dry-run

New `BuildOptions { tolerate_unsynced_children: bool }` struct (marked `#[non_exhaustive]`); sync opts in
under `--dry-run`. When set, `handle_child` synthesizes a placeholder via the existing
`synthesize_plain_git_manifest` helper instead of returning `ManifestNotFound`. The v1.4.0 `build_graph`
public entry point is preserved as a thin forwarder; new entry point is `build_graph_with(.., opts)`.

### B8 — Phase 1 collision warning on dry-run

`crates/grex-core/src/tree/walker.rs::dest_has_nongit_content` probes whether a declared dest exists with
non-empty non-git content; when true the Phase 1 `Missing` arm emits a direct `eprintln!("warning:
declared child slot ... collides with pre-existing non-git content")` to subprocess stderr. Bypasses
`tracing` to avoid race conditions on the rayon worker pool's subscriber state.

### B9 — Offline `real-smoke` harness

`crates/real-smoke/src/seed.rs` + `journey.rs` + `crates/real-smoke/tests/cfg_shape.rs` build a hermetic
offline tier:

- `seed_bare_repo` initialises `git init --bare` repos with arbitrary `pack.yaml` content; `file_url`
  converts the bare-repo path to a `file://` URL that gix accepts as a clone source.
- `Journey` / `Step` DSL composes scripted user-flow tests (`init` → `import` → `ls` → `sync` →
  `doctor`); each step asserts on exit code + stdout/stderr + on-disk state via small reusable
  closures (`file_exists`, `path_absent`, `stdout_contains_all`, `events_jsonl_add_count`,
  `pack_yaml_has_children`).
- `tests/cfg_shape.rs` reconstructs the user's cfg-shape workflow against six seeded leaf bares spread
  across `cmn / win / lnx / mac` buckets and asserts the round-trip across every one of the seven bug
  surfaces above.

### B10 — Fixture corrections (out-of-tree, alongside this release)

- `egoisth777/grex-test-leaf` — `type: pack` → `type: scripted`.
- `egoisth777/grex-test-leaf-{1,2,3}` — NEW. Three distinct leaf packs with `name:` matching their path
  basename, so `verify_child_name` accepts them as flat-sibling children of `meta-flat`.
- `egoisth777/grex-test-meta-flat` — child URLs flipped from three clones of `leaf` to the three new
  `leaf-{1,2,3}` repos.
- `egoisth777/grex-test-meta-nested` — child paths updated to `nested/grex-test-meta-flat` and
  `nested/grex-test-leaf` to satisfy `verify_child_name`.

## § Out of scope

- MCP-side wiring of remaining stubs (`rm`, `status`, `update`, `run`, `exec`) — rolls forward to v1.5.0
  per the v1.4.0 endpoint's pickup list.
- Recursive `grex run` walking child packs — v1.5.0.
- `--workspace` flag removal (currently emits deprecation warn) — v2.0.0.
- Plugin-API freeze — v1.5.0.
- The lockfile schema's `LockEntry` is unchanged; a future release may surface a `url` field for
  ergonomic third-party tooling, but the v1.3.1 B14 contract (LockEntry tuple is `{id, path, sha, branch,
  ...}`) holds.

## § Acceptance

Pass conditions for PR #80 squash-merge:

1. `cargo test --workspace` green on Windows, macOS, Linux × stable.
2. `cargo clippy --workspace --all-targets -- -D warnings` clean.
3. `cargo fmt --all -- --check` clean.
4. `cargo doc --workspace --no-deps --all-features` clean under `RUSTDOCFLAGS="-D warnings"`.
5. `cargo-deny` / `cargo-audit` / `cargo-machete` / `typos` all green.
6. `code-metrics` (cbo ≤ 10, cyclomatic ≤ 15) green.
7. `man-drift` green (regenerate `man/` via `cargo run -p xtask -- gen-man`).
8. `Lean4 proof gate` green (no axiom changes).
9. `MCP protocol conformance` green.
10. `release-plan (cargo-dist)` green.
11. **Online-tier `real-smoke` workflow green** — gated on the `real-smoke` PR label per the workflow's
    `if:` clause. All 15 `t_b*` regression tests pass.
12. CHANGELOG entry under `## [1.4.1]` covers every bullet above per Keep-a-Changelog 1.1.0.
13. `xtask/tests/version_test.rs::EXPECTED_WORKSPACE_VERSION` bumped to `1.4.1`.

## § SemVer rationale

PATCH (per rule 6). The v1.4.0 endpoint deferred three HARD items into v1.4.x: **`--workspace` removal,
plugin-API freeze, smoke-harness gix HTTPS**. Only the last lands in v1.4.1; the other two roll forward
to v1.5.0 unchanged. The deferred items are themselves additive-or-warned-removal, not breaking, so the
PATCH classification holds across the v1.4.x range.

## § Lean4

NONE. Bug-fix bundle on top of already-verified primitives. The new `tolerate_unsynced_children`
synthesis path mirrors the existing plain-git synthesis already covered by walker theorems
(`Grex.Walker.dry_run_no_side_effects`, `Grex.Walker.synthesize_plain_git_manifest_*`); no new algorithmic
behavior. Axiom budget unchanged at 9 bridge / 4 types / 0 model.
