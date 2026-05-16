---
slug: feat-v-1-4-1-design
type: spec
status: backfilled
last_updated: 2026-05-16
topic: smoke-test-bug-bundle-design
depends_on: [feat-v1.4.1]
---

# feat-v1.4.1 — design

## § Bridge architecture: events.jsonl ↔ pack.yaml

v1.4.0 had two parallel state stores that were only loosely related:

- **`.grex/events.jsonl`** — append-only audit log; written by `add` / `import` / `sync`; read by
  `doctor`'s drift check and the lockfile-vs-disk reconciler.
- **`.grex/pack.yaml`** — declarative manifest; hand-authored or `grex init`-seeded; walked by
  `sync` / `ls` / `status` / `build_graph`.

The v1.4.0 implementation wrote to events.jsonl ONLY on `add` / `import`. Result: sync's planner had no
idea the new pack was registered. Doctor saw the events but treated the missing on-disk dir as drift.

### Why not "fold events.jsonl into the graph"?

Considered: have `build_graph` start from events.jsonl rather than pack.yaml.children. Rejected for two
reasons:

1. **Spec stability.** The v1 frozen contract (`inst/grad/roadmap.md` § Frozen public APIs) names
   `.grex/pack.yaml` schema as item #1. Authors hand-edit it. Folding events.jsonl as the canonical
   source would either (a) demote pack.yaml to a cache, requiring a migration story, or (b) force
   `build_graph` to reconcile two stores at every walk, doubling read cost.
2. **Ergonomic ground truth.** The pack-spec is intentionally declarative — users read pack.yaml to
   understand what their workspace SHOULD be. Folding events would mean "your workspace is what your
   audit log replays to", which is a fundamentally different mental model from the one the v1 docs
   establish.

So: pack.yaml stays canonical. events.jsonl stays audit-only. **Bridge: `add` / `import` write to BOTH.**

### YAML round-trip approach

The new `crates/grex-core/src/pack/yaml_writer.rs` loads `pack.yaml` as `serde_yaml::Value::Mapping`,
mutates the `children:` key only, and serializes back. Three benefits:

1. **No round-trip through the typed `PackManifest`.** The typed parse is lossy — actions are
   key-dispatched (not `#[serde(untagged)]`), so the parse/serialize surface diverges across action
   variants. Round-tripping at the `Value` layer preserves every field grex does not yet understand,
   including future `x-*` extensions.
2. **Comments are lost.** Documented in the module header. The pack-spec already promises that
   `children:` is grex-owned, so authors who annotate that section with comments have opted into
   round-trip loss.
3. **`serde_yaml::Mapping` preserves insertion order.** The internal storage is a `Vec<(Value, Value)>`,
   so existing key ordering survives.

### Skeleton synthesis when pack.yaml is absent

The previous behavior was: `grex add` outside a pre-initialized workspace silently writes to events.jsonl
with no pack.yaml ever materializing. v1.4.1 synthesizes the minimal v1 manifest skeleton on demand:

```yaml
schema_version: "1"
name: <derived-from-workspace-dir>
type: meta
actions: []
children:
  - url: ...
    path: ...
```

`derive_pack_name` cleans the workspace dir name to satisfy `^[a-z][a-z0-9-]*$` (lowercase, hyphen-join,
strip leading non-letters). On unrecoverable input (`...` / empty after cleaning) it falls back to
`workspace`.

## § BuildOptions surface

`build_graph` is `pub` and called by `sync`, `teardown`, and the property-test in
`crates/grex-core/tests/property.rs`. We preserve the v1.4.0 entry point as a thin forwarder so external
callers (xtask, future downstream consumers) keep working:

```rust
pub fn build_graph(workspace: &Path, backend: &dyn GitBackend, loader: &dyn PackLoader,
                   ref_override: Option<&str>) -> Result<PackGraph, TreeError> {
    build_graph_with(workspace, backend, loader, ref_override, BuildOptions::default())
}

#[non_exhaustive]
#[derive(Debug, Clone, Copy, Default)]
pub struct BuildOptions {
    pub tolerate_unsynced_children: bool,
}

pub fn build_graph_with(workspace: &Path, backend: &dyn GitBackend, loader: &dyn PackLoader,
                        ref_override: Option<&str>, opts: BuildOptions) -> Result<PackGraph, TreeError> {
    // ... existing body, plus opts threaded through to handle_child ...
}
```

Sync wires `BuildOptions { tolerate_unsynced_children: dry_run, ..Default::default() }` so:

- **Wet run.** `tolerate_unsynced_children = false`. `build_graph` enforces the strict pre-v1.4.1
  contract — every declared child must have a manifest on disk (or carry `.git/` for plain-git
  synthesis). Phase 1 has already cloned by the time `build_graph` runs, so the contract holds.
- **Dry run.** `tolerate_unsynced_children = true`. Phase 1 records would-clone but does not materialize.
  `build_graph` synthesizes a placeholder via the existing `synthesize_plain_git_manifest` helper
  (re-used for the v1.1.1 plain-git path), so the planner can produce a complete graph the executor
  no-ops over.
- **Teardown.** Keeps `tolerate_unsynced_children = false`. Teardown operates against materialized
  packs; a missing dest really IS an error there.

The synthesized placeholder carries `synthetic = true` so `grex ls` renders it as `(declared, unsynced)`
(matches the pre-v1.4.1 ls semantic).

## § Phase 1 collision warning — why `eprintln!` not `tracing`

Initial implementation used `tracing::warn!(target: "grex::sync", ...)`. CI consistently saw the warning
emit AFTER the test's panic (race against subscriber init on the rayon worker pool). The captured
`CliResult.stderr` was empty even though the warn fired inside the subprocess.

Switched to direct `eprintln!`. Rationale:

1. **Synchronous write.** Bypasses any subscriber buffering / EnvFilter ordering / thread-local
   subscriber registration. The line lands on subprocess stderr before any other code observes it.
2. **No log-level config dependency.** Operators running `grex sync --dry-run` see the warning
   regardless of `RUST_LOG`.
3. **No `Co-Authored-By:` analog.** The warning is part of the CLI's user-facing diagnostic surface;
   like collision detection on `add`, it deserves a guaranteed delivery channel.

Trade-off: structured-log consumers can't filter on `target=grex::sync` to suppress it. Acceptable for a
hard-stop diagnostic that operators should NEVER want to silence in production sync flows.

## § Online-tier real-smoke unblock

Three independent root causes, all addressed in this release:

### Cause 1 — gix HTTPS

`gix` workspace pin gains `blocking-http-transport-reqwest-rust-tls`. Brings in `reqwest` + `rustls` +
`webpki-roots` as transient deps. The `CDLA-Permissive-2.0` license (Mozilla CA bundle) is added to the
`deny.toml` allowlist. SSH transport stays handled by the existing `blocking-network-client` feature.

### Cause 2 — Walker panic on never-synced dry-run meta-packs

Discussed under § BuildOptions surface.

### Cause 3 — Out-of-tree fixtures

The fixture repos predate the v1.2.0 strict-name-gate (`graph_build::verify_child_name`) and the v1
schema freeze (`PackType` enum closed to `meta` / `declarative` / `scripted`). Three fixture commits
pushed alongside this release:

- `grex-test-leaf @ d1203cf` — `type: pack` → `type: scripted`.
- `grex-test-meta-flat @ deb7e89` — points at three new `grex-test-leaf-{1,2,3}` repos so each child's
  name matches its path basename.
- `grex-test-meta-nested @ a05114c` — child paths use the referenced fixtures' canonical names
  (`nested/grex-test-meta-flat`, `nested/grex-test-leaf`).

Plus three NEW repos under `egoisth777/`: `grex-test-leaf-1`, `grex-test-leaf-2`, `grex-test-leaf-3`.

## § Test-side regression tracking

The dogfood B1–B15 regression suite has been red on `main`'s nightly schedule since 2026-05-07. The
deltas behind that long red streak are:

- 5 failures from Cause 1 (gix HTTPS).
- 3 failures from Cause 2 (walker panic).
- 4 failures from Cause 3 (fixture content).
- 2 failures from genuine v1.4.0+ behavior changes the tests didn't track:
  - `t_b09` expected `status` and `update` to be stubs (v1.4.0 wired them — invert assertion).
  - `t_b10` invoked `grex add --url <URL>` (incorrect; the CLI takes URL as the first positional).
- 1 failure from lockfile schema migration the test didn't track:
  - `read_lockfile` was looking at `.grex.sync.lock` (the workspace concurrency lock, an empty sidecar)
    instead of `grex.lock.jsonl` (the resolution lockfile).
- 1 failure from `LockEntry` schema migration:
  - `t_b14` discriminated child entries on the legacy `url` field. The current `LockEntry` tuple is
    `{id, path, sha, branch, installed_at, actions_hash, schema_version}`. Discriminator updated to
    `{id, path, sha}`.

Tests updated; assertions tightened where the underlying contract changed.

## § Offline real-smoke framework

The user explicitly asked for an offline tier so the bug class that surfaced today is regression-tested
on every CI run (not gated on the `real-smoke` PR label). Design:

```text
crates/real-smoke/
  src/
    seed.rs       NEW. Local bare-repo seeder backed by `git init --bare`.
                  No network. Companion render_repos_json + file_url helpers.
    journey.rs    NEW. Step DSL + Journey runner. Each Step runs a grex
                  invocation via the existing black-box subprocess driver,
                  asserts on exit code + stdout/stderr + on-disk state.
    grex_cli.rs   Existing. Patched to probe target/llvm-cov-target/
                  alongside the conventional target/{debug,release}/.
  tests/
    cfg_shape.rs  NEW. First end-to-end journey. Walks init → import → ls
                  → sync → doctor with one assertion per v1.4.0 smoke-test
                  bug. Each bug now has a passing regression gate.
```

Inheritance chain:

- The existing online-tier `regression.rs` keeps `#[ignore]`'d behind the
  `requires network + SSH key + provisioned GH fixtures` marker. The `real-smoke` workflow continues to
  run them on PR label + nightly cron.
- The new offline-tier `cfg_shape.rs` tests are NOT `#[ignore]`'d. They run on every `cargo test
  --workspace` invocation — including standard PR builds and coverage.

This is the architecture the user described in the v1.4.0 endpoint's smoke-test report:

> **Three layers** — unit < e2e-offline < smoke-online. `e2e-offline` is the missing tier. Hermetic.
> Fast. Runs in CI without `GITHUB_TOKEN`.

## § Rollback / safety

PATCH release. No schema migrations. No file format changes. The bridge writes to a fresh `pack.yaml`
when one isn't present, but never modifies fields outside `children:` on an existing file. Idempotent on
duplicate child path.

If the bridge causes pain (unlikely — every contract change is additive), users can:

1. Hand-edit `.grex/pack.yaml` to remove unwanted children.
2. Pin to v1.4.0 via `cargo install grex-cli@1.4.0`.

The `BuildOptions` knob is opt-in; downstream callers of `build_graph` (the v1.4.0 entry point) see no
behavior change.
