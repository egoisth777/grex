# SemVer policy for grex

grex follows [Semantic Versioning 2.0.0](https://semver.org/spec/v2.0.0.html).
This document pins down what "breaking", "additive", and "fix" mean concretely
for grex, because the public surface spans four distinct contracts that users
and agents depend on:

1. **Manifest schema** — `grex.jsonl` + `grex.lock.jsonl` row shapes, keyed on
   the per-row `schema_version` field.
2. **CLI surface** — verb names, flag names, exit codes, and the `--json` /
   `--plain` stdout formats.
3. **MCP tool surface** — JSON-RPC tool names, input/output JSON schemas, and
   tool annotations exposed by `grex serve --mcp`.
4. **`pack.yaml` schema** — pack-type plugin names, action names, and action
   field shapes consumed by the pack parser.

A release is MAJOR, MINOR, or PATCH based on the **worst** change across all
four surfaces. A MAJOR change on any one surface forces a MAJOR release, even
if the other three are additive-only.

## The short version

| Bump  | What it means                                                                                |
|-------|----------------------------------------------------------------------------------------------|
| MAJOR | Existing workspaces / agents / packs may stop working after upgrade; migration may be needed. |
| MINOR | Everything that worked before still works; new capabilities are available to opt into.       |
| PATCH | Behaviour identical from the user's perspective; bugs fixed, perf improved, docs clarified.  |

## The manifest-wire invariant

The single load-bearing invariant across all four surfaces is the JSONL wire
format of `grex.jsonl` and `grex.lock.jsonl`:

- Every row carries a `schema_version` integer field (since M2 /
  [PR #2](https://github.com/egoisth777/grex/pull/2)).
- **Writers** never emit rows at a schema version older than the one their
  binary understands. A newer `grex` writes newer rows; an older `grex` never
  downgrades.
- **Readers** treat unknown future fields on a known-version row as
  **skip-don't-error** — extra keys are ignored, not rejected.
- Bumping `schema_version` past the max a reader supports is a **MAJOR** event
  for that row kind; readers older than the new major will refuse the row with
  a structured error and instruct the user to upgrade.

This is the one rule that survives any SemVer ambiguity below: if you cannot
round-trip a manifest through an older compatible `grex` without silent data
loss, the change is MAJOR.

## Per-surface rules

### 1. Manifest schema (`grex.jsonl` / `grex.lock.jsonl`)

| Change                                                                        | Bump  |
|-------------------------------------------------------------------------------|-------|
| Remove a row kind (e.g. drop `RegisterPack` rows)                             | MAJOR |
| Rename a required field on an existing row (e.g. `url` → `repo_url`)          | MAJOR |
| Change the type of an existing field (e.g. `parallel: int` → `parallel: str`) | MAJOR |
| Tighten a constraint (e.g. a previously free-form string becomes enum-only)   | MAJOR |
| Bump `schema_version` past what older readers support                         | MAJOR |
| Add a new row kind that older readers skip cleanly (unknown `kind` = skip)    | MINOR |
| Add a new optional field to an existing row (readers ignore unknown fields)   | MINOR |
| Widen a constraint (e.g. enum gains a new variant — older readers skip row)   | MINOR |
| Fix a writer bug that emitted malformed rows (readers already tolerant)       | PATCH |
| Improve compaction perf; rewrite internals without format change              | PATCH |

### 2. CLI surface (verbs, flags, exit codes, stdout format)

| Change                                                                        | Bump  |
|-------------------------------------------------------------------------------|-------|
| Rename or remove a verb (`grex add` → `grex register`)                        | MAJOR |
| Change a verb's positional-argument shape (`<url> [path]` → `<path> <url>`)   | MAJOR |
| Remap an existing exit code's meaning (e.g. `2` previously = parse error, now `2` = lock contention) | MAJOR |
| Change the shape of `--json` stdout for an existing verb                      | MAJOR |
| Remove a flag (even a short alias) that was stable in the previous MINOR      | MAJOR |
| Add a new verb                                                                | MINOR |
| Add a new flag with a safe default that preserves prior behaviour             | MINOR |
| Add a new field to an existing `--json` payload (consumers ignore unknowns)   | MINOR |
| Improve an error message; reword `--help` text; fix tab alignment             | PATCH |
| Fix a buggy exit code that never returned its documented value                | PATCH |

Caveat on exit-code fixes: a PATCH-class exit-code correction is still visible
to scripts that pinned against the buggy value. The CHANGELOG entry **must**
call it out under `Fixed` in bold so operators notice before upgrading.

### 3. MCP tool surface (`grex serve --mcp`)

| Change                                                                        | Bump  |
|-------------------------------------------------------------------------------|-------|
| Rename or remove a tool (`pack_add` → `register_pack`)                        | MAJOR |
| Remove a required field from a tool's input schema                            | MAJOR |
| Add a new required field to a tool's input schema                             | MAJOR |
| Change a tool's output schema field type                                      | MAJOR |
| Change or remove a tool annotation an existing client depends on              | MAJOR |
| Add a new tool                                                                | MINOR |
| Add an optional input field with a safe default                               | MINOR |
| Add a new output field (clients ignore unknowns per MCP spec)                 | MINOR |
| Add a new annotation                                                          | MINOR |
| Fix a handler bug where the tool returned success on partial failure          | PATCH |
| Improve tool description strings; tighten input-validation error messages     | PATCH |

The MCP conformance suite
([PR #28](https://github.com/egoisth777/grex/pull/28)) pins the 2025-06-18 MCP
spec revision. Bumping to a later MCP spec revision is itself MAJOR if the
newer spec has breaking changes the grex surface propagates; otherwise MINOR.

### 4. `pack.yaml` schema (pack-type + action plugins)

| Change                                                                        | Bump  |
|-------------------------------------------------------------------------------|-------|
| Rename a built-in pack-type (`declarative` → `static`)                        | MAJOR |
| Rename a built-in action (`file-write` → `write-file`)                        | MAJOR |
| Remove a field from an existing action's input shape                          | MAJOR |
| Change an action's default behaviour for an existing field                    | MAJOR |
| Remove a built-in pack-type or action                                         | MAJOR |
| Add a new built-in pack-type                                                  | MINOR |
| Add a new built-in action                                                     | MINOR |
| Add a new optional field to an existing action                                | MINOR |
| Loosen a validation rule (previously rejected input now accepted with warning)| MINOR |
| Fix a parser bug; improve error locations; clarify validation messages        | PATCH |
| Improve action-execution perf; refactor executor internals                    | PATCH |

External plugin ABI stability is deferred to the v2 plugin spec; v1.0.0 has
no external plugin surface.

## Deprecation policy

- When grex needs to remove a verb, flag, tool, annotation, pack-type, action,
  or manifest field, it first **deprecates** the surface in a MINOR release.
- A deprecated surface continues to work for at least one full MINOR cycle
  before removal in the next MAJOR.
- `grex doctor` surfaces deprecation warnings in its output when a workspace's
  manifest or pack tree uses a deprecated surface. `doctor`-clean before a
  MAJOR upgrade means no deprecated usage left.
- MCP clients receive deprecation notices via the tool-annotation mechanism; a
  deprecated tool's annotation gets a `deprecated: true` marker and a
  human-readable message pointing at its replacement.
- Deprecation entries go under `### Deprecated` in `CHANGELOG.md` on the MINOR
  release that introduces them and under `### Removed` in the MAJOR release
  that retires them, with a back-reference to the deprecation entry.

## What is **not** covered by SemVer

grex's SemVer contract covers the four public surfaces above. The following
are explicitly out-of-scope:

- **Internal module layout** (`grex-core` internals, private items). Reshuffled
  without bumping — consumers should not depend on private crate APIs.
- **Log / trace / stderr formatting** (not `--json` and not `--plain`). Free to
  evolve at any point.
- **Build artefact names and installer script URLs** — these follow the
  cargo-dist release pipeline's conventions, not grex SemVer.
- **`docs/` content, design notes, and `milestone.md`**. Documentation is
  maintained for correctness but not versioned.
- **CI matrix composition** (adding or dropping platforms from the build
  matrix). Platform-support drops will be called out in the CHANGELOG but
  follow their own platform-support policy.
- **Minimum supported Rust version (MSRV)** — MSRV bumps are MINOR and are
  called out in the CHANGELOG.

## See also

- [`CHANGELOG.md`](../CHANGELOG.md) — per-release entries with categorised
  changes.
- [`.omne/cfg/manifest.md`](../.omne/cfg/manifest.md) — normative manifest /
  lockfile schema.
- [`.omne/cfg/cli.md`](../.omne/cfg/cli.md) — v1 frozen verb contract.
- [`.omne/cfg/mcp.md`](../.omne/cfg/mcp.md) — MCP server surface.
- [`.omne/cfg/pack-spec.md`](../.omne/cfg/pack-spec.md) — `pack.yaml` schema
  and built-in pack-types.
