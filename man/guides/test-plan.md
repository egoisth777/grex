# test-plan

Pyramid from unit through CI cross-platform + Lean4 proof compilation + pack-protocol contract tests.

## Pyramid

```
        ┌──────────────────────────────┐
        │  Cross-plat CI matrix        │  few, slow
        ├──────────────────────────────┤
        │  Pack-protocol contract      │  fixture packs end-to-end
        ├──────────────────────────────┤
        │  MCP roundtrip               │  JSON-RPC scripted
        ├──────────────────────────────┤
        │  Crash injection             │  SIGKILL / TerminateProcess
        ├──────────────────────────────┤
        │  Integration                 │  real git, temp dirs
        ├──────────────────────────────┤
        │  Property (proptest)         │  manifest CRUD algebra
        ├──────────────────────────────┤
        │  Unit                        │  fast, exhaustive, in-process
        └──────────────────────────────┘
```

## Unit tests

In-module `#[cfg(test)]`. Fast, no IO except via `tempfile`.

Coverage targets:

- `manifest::event` — every event variant, schema bump rejection, malformed line behavior.
- `manifest::fold` — ordering, tombstone precedence, update idempotence.
- `manifest::lock` — last-write-wins per id.
- `pack::schema` — full `pack.yaml` schema validation, rejects + accepts.
- `gitignore` — managed-block insert, update, preserve-user-lines, idempotent-sync.
- `cli::output` — JSON / plain / pretty modes against golden strings.
- `concurrency::scheduler` — semaphore acquisition order with mocked `PackLock`.
- `actions::*` — each of 7 primitives has targeted unit tests (args parsing, dry-run, idempotency check).
- `packtypes::*` — each lifecycle method dispatches correctly.
- `fetchers::git` — URL parsing, ref-spec resolution.

## Integration tests (`tests/`)

Each spins a temp dir via `tempfile::TempDir`, invokes compiled binary via `assert_cmd` or library entrypoints directly.

| File | Scenario |
|---|---|
| `integration_add.rs` | `grex add` against local bare-repo fixture → event appended, dir cloned, `.gitignore` updated, pack.yaml auto-detected |
| `integration_rm.rs` | add → rm → manifest tombstoned, dir gone, teardown ran |
| `sync_recursive.rs` | meta-pack with nested children syncs 3 levels deep |
| `sync_parallel.rs` | 8 local fixture packs, `grex sync --parallel 4`, all succeed, wall time sub-linear |
| `gitignore_preserves_user_lines.rs` | pre-populated `.gitignore` with user content outside managed block → round-trip preserves byte-for-byte |
| `crash_recovery.rs` | spawn child, SIGKILL (Win: TerminateProcess) mid-append, `grex ls` recovers via torn-line detection |
| `mcp_stdio.rs` | spawn `grex serve --mcp`, scripted JSON-RPC session, assert responses |
| `import_legacy.rs` | seed `REPOS.json` + `.gitignore`, run `grex import --from-repos-json`, verify manifest + gitignore coexistence |
| `doctor_drift.rs` | corrupt manifest / delete workdir, `grex doctor --fix` restores invariants |
| `pack_types_end_to_end.rs` | one fixture of each of 3 pack-types: install + sync + teardown full round-trip on all OSes |
| `bench_manifest.rs` | 10k events fold < 1s, 100k events < 10s (criterion; non-blocking) |

Git fixtures: bare `.git` local repos under `tests/fixtures/`, served via `file://` URLs. No network in CI tests.

## Property tests (`proptest`)

`tests/property_manifest.rs`:

- Generate arbitrary sequences of `add` / `rm` / `update` / `sync` events.
- Invariants under fold:
  - Tombstoned id never in state map.
  - Compaction idempotent: `compact(compact(m)) == compact(m)`.
  - Fold-equivalence: `fold(m) == fold(compact(m))`.
  - Update last-writer-wins per id.

`tests/property_gitignore.rs`:

- Random pre-existing `.gitignore` + random sequences of add/rm.
- Invariants:
  - User lines outside managed block unchanged byte-for-byte.
  - Two consecutive syncs produce identical output.

`tests/property_actions.rs`:

- Each action primitive: running twice in sequence is equivalent to running once (idempotency).
- `rollback(execute(x))` == starting state (for actions that support rollback).

## Crash injection

`tests/crash_recovery.rs`:

- Spawn helper binary (`crash-helper`, built alongside the test) that appends to `grex.jsonl` then panics mid-write (partial bytes, no newline, exits).
- Parent opens the manifest, runs fold, expects success + one truncated-tail warning in tracing output.

Windows variant uses `TerminateProcess` via raw handle (`#[cfg(windows)]`).

## MCP roundtrip

`tests/mcp_stdio.rs`:

1. `assert_cmd` spawns `grex serve --mcp --manifest <tempdir>/grex.jsonl`.
2. Pipe JSON-RPC frames to stdin, read stdout.
3. Sequence: `initialize` → `grex.add` → `grex.ls` → `grex.sync` → `grex.status` → `grex.rm` → `grex.ls`.
4. Assert each response matches expected JSON shape (via `serde_json::Value` equality + predicates).
5. Assert clean shutdown on stdin close.

## Cross-plat CI matrix

All integration + property + crash + MCP tests run on:

- `ubuntu-latest`
- `macos-latest`
- `windows-latest`

Fixtures avoid platform-specific paths — all tests use `tempfile` + `PathBuf`.

## Lean4 proof verification

`.github/workflows/lean.yml`:

```yaml
- uses: leanprover/lean-action@v1
- run: cd lean && lake build
```

Job succeeds only if `lean/Grex/Scheduler.lean` compiles to `.olean` with zero `sorry`. Any unresolved `axiom` outside the single `pack_lock_exclusive` model-bridge axiom (resolved to theorem by M5-exit) fails CI.

Lean type-checking is the guarantee; CI does not attempt to verify proof content beyond compilation.

## Pack-protocol contract tests

`tests/pack_types_end_to_end.rs` + fixture pack repos under `tests/fixtures/packs/`:

- `meta-basic/` — meta pack with 2 nested declarative children.
- `declarative-basic/` — declarative pack exercising all 7 action types.
- `scripted-basic/` — scripted pack with setup.sh + setup.ps1 + teardown.{sh,ps1}.

Contract assertions:

- Install + sync + teardown round-trip leaves the workspace in the pre-install state.
- Install followed by install (no changes) = idempotent.
- Teardown followed by teardown = idempotent (second is no-op).
- Lockfile entry matches expected `sha` + `actions_hash` after install.

Fixtures double as living documentation — they're the canonical "what does a v1 pack look like" examples.

## Coverage

`cargo-llvm-cov` weekly on `main`. Target: 80% line coverage on `manifest`, `pack`, `plugin`, `actions`, `packtypes`, `gitignore`, `concurrency`. CLI + MCP exercised via integration tests, not measured for line coverage.

## Smoke test (pre-release, manual)

Before tagging a release:

1. `cargo install --path grex`
2. `cd <tempdir> && grex init && grex add git@github.com:grex-org/grex-inst.git && grex ls --long && grex sync && grex doctor`.
3. `grex serve --mcp` → send `initialize` manually, verify response.
4. `grex doctor` → exit 0.
5. Repeat on macOS and Windows.
