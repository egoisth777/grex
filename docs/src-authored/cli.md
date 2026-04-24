# cli — v1 frozen verb contract

12 verbs. Freeze is **additive-only** post-v1: adding verbs, flags, or JSON-output fields is allowed; removing or renaming is a v2 change.

## Universal flags

| Flag | Effect |
|---|---|
| `--json` | emit machine-readable JSON to stdout, suppress ANSI |
| `--plain` | ANSI off, no Unicode, CI/agent-friendly |
| `--dry-run` | compute plan, print it, do NOT mutate disk or manifest |
| `--parallel N` | bound scheduler semaphore to N permits (default: `num_cpus`) |
| `--filter <expr>` | restrict verb to matching packs (name glob, type, depth) |
| `--manifest <path>` | override default `./grex.jsonl` |
| `-v`, `-vv`, `-vvv` | tracing verbosity |

Output mode precedence: `--json` > `--plain` > TTY auto-detect pretty default.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | success |
| 1 | generic error |
| 2 | CLI usage error |
| 3 | manifest integrity failure |
| 4 | pack op failed (fetch/install/sync/teardown) |
| 5 | lock contention / concurrency |
| 6 | MCP protocol error |
| 7 | doctor found drift |
| 8 | plugin / unknown action or pack-type |

## The 12 verbs

### `grex init`

Initialize a grex workspace (creates `grex.jsonl`, configures hooks, writes `.gitignore` managed-block markers if missing).

- **Args**: none.
- **Flags**: `--hooks-path <dir>` (default `.grex/hooks`), `--no-clone` (skip fetch of pre-existing entries).
- **Example**: `grex init --parallel 4`
- **JSON**: `{"workspace":"<cwd>","created":["grex.jsonl","grex.lock.jsonl"],"hooks":"<path>","cloned":[]}`

### `grex add <url> [path]`

Register a pack, clone it, run its install.

- **Args**: `<url>` required; `[path]` optional bare-name, inferred from URL basename.
- **Flags**: `--type <meta|declarative|scripted>` (auto-detected from pack.yaml), `--ref <branch|tag|sha>`, `--no-install` (clone only).
- **Exit**: 2 if path not bare; 4 if fetch or install fails.
- **Example**: `grex add git@github.com:user/warp-cfg.git warp-cfg`
- **JSON**: `{"id":"warp-cfg","type":"declarative","path":"warp-cfg","sha":"abc123","installed":true}`

### `grex rm <path>`

Run teardown, remove pack dir, tombstone in manifest, update `.gitignore`.

- **Args**: `<path>` required.
- **Flags**: `--keep-files` (tombstone only), `--skip-teardown` (do not run teardown actions/hooks).
- **JSON**: `{"id":"...","removed":true,"files_deleted":true,"teardown":"ok"}`

### `grex ls`

List registered packs (post-fold).

- **Flags**: `--type <...>`, `--long` (include SHA + install time + actions_hash), `--tree` (nested view).
- **JSON**: `[{"id":"...","type":"...","path":"...","ref":"...","sha":"...","installed_at":"..."}]`

### `grex status`

Drift report: manifest vs lockfile vs on-disk.

- **Flags**: `--stale-after <duration>`, `--fail-on-drift`.
- **JSON**: `[{"id":"...","on_disk":true,"sha_match":true,"actions_hash_match":true,"drift":"clean|dirty|missing|untracked|stale"}]`
- **Exit**: 7 if any drift with `--fail-on-drift`.

### `grex sync [--recursive]`

Git fetch/pull every pack; recurse into children. Install actions are **not** re-run here (see `update`).

- **Flags**: `--recursive` (default true), `--only <id>`, `--fail-fast`.
- **M4-D flags** (additive, freeze-preserving):
  - `--ref <REF>` — override every pack's declared `ref` for this sync invocation (branch, tag, or commit SHA). Applied by the walker at each child clone / checkout; the root pack itself is not re-checked-out (operator manages root via `grex add` / manual git). Empty and whitespace-only values rejected at parse time.
  - `--only <GLOB>` — restrict sync to packs whose **workspace-relative pack path**, normalized to forward-slash form (`/`), matches the glob. Cross-platform consistent: `a`, `b/c`, `vendor/*` evaluate identically on Windows and POSIX. The root pack (whose path lies outside the workspace) falls back to its absolute forward-slash path. Bare pack names do not match unless the name coincides with the workspace-relative path. Repeat the flag to OR-combine multiple patterns. Non-matching packs are skipped entirely (no action execution); their prior lockfile entry is carried forward so a subsequent unfiltered sync still short-circuits on hash. Invalid globs exit 2 (CLI usage error). Caveat — `--only` does NOT expand to include a pack's `depends_on` / child dependencies; operator must include them explicitly if dependency-filtered runs are required. Empty and whitespace-only values rejected at parse time.
  - `--force` — re-execute every pack even when its `actions_hash` is unchanged from the prior lockfile. Bypasses the M4-B skip-on-hash short-circuit. Caveat — non-idempotent actions (`exec` without guard, `mkdir` with `mode` drift, etc.) may produce duplicate / compounding side effects when `--force` replays after a mid-run halt; operator responsibility to ensure action idempotency before using `--force` on a partially-applied workspace.
- **JSON**: `[{"id":"...","result":"ok|err","sha_before":"...","sha_after":"...","message":""}]`
- **Exit**: 4 on op failure without `--keep-going`.

### `grex update [pack]`

Sync + re-run install actions for packs whose lockfile SHA or `actions_hash` changed.

- **Args**: `[pack]` optional; defaults to all.
- **Flags**: `--force` (re-run install regardless of lock), `--only <id>`.
- **JSON**: `[{"id":"...","synced":true,"reinstalled":true,"reason":"sha-changed|hash-changed|forced|none"}]`

### `grex doctor`

Integrity + drift + lint.

- Checks: manifest schema, gitignore managed-block in sync, on-disk pack drift, `.grex/pack.yaml` schema validity, stale `.grex-lock` files, orphan entries.
- **Flags**: `--compact` (run manifest compaction), `--fix` (auto-fix fixable issues).
- **Exit**: 7 on drift, 3 on manifest integrity failure, 0 clean.

### `grex serve --mcp`

Launch embedded MCP stdio JSON-RPC 2.0 server.

- **Flags**: `--mcp` (required; reserved for `--http` in v2).
- **Exit**: 6 on protocol error.
- Details: [mcp.md](./mcp.md).

### `grex import`

Bring external state into the manifest.

- **Flags**:
  - `--from-repos-json <path>` — ingest legacy flat `REPOS.json` array.
  - `--scan` — walk workspace one level deep, register untracked `.git` dirs.
  - `--default-type <...>` — pack-type assumed for entries without pack.yaml (default: `meta`).
- **JSON**: `{"imported":[...],"skipped":[...],"errors":[...]}`

### `grex run <action> [--filter <expr>]`

Invoke a registered action by name across matched packs. Primarily for testing/diagnostic use; production installs go through pack-type lifecycle.

- **Args**: `<action>` required; matches registered plugin name.
- **Flags**: `--filter <expr>`, `--parallel N`.
- **JSON**: `[{"pack":"...","action":"...","changed":true,"message":""}]`

### `grex exec <cmd> [-- args...] [--filter <expr>]`

Run an arbitrary command inside each matched pack's workdir.

- **Args**: `<cmd>` required.
- **Flags**: `--filter`, `--parallel N`, `--shell` (opt-in shell parsing; off by default).
- **Example**: `grex exec git status`
- **JSON**: `[{"pack":"...","stdout":"","stderr":"","exit":0}]`

## Verb interactions

- `sync` only fetches; `update` = `sync` + install re-run on lockfile delta.
- `run` operates on actions directly, bypassing pack-type lifecycle; useful for debugging a single action.
- `exec` is never filtered through the action plugin registry; it runs arbitrary commands.
- `serve --mcp` does not block other verbs; it exposes them over JSON-RPC.

## Freeze semantics

A v1.x release may:

- Add a new verb.
- Add a new flag to an existing verb.
- Add a new field to any `--json` output.
- Add a new action name, pack-type name, or MCP method (all additive).

A v1.x release may NOT:

- Remove or rename a verb.
- Change the meaning of an existing flag.
- Change the type of an existing JSON output field.
- Remove an action name or pack-type name.

`grex doctor` validates `pack.yaml` against the frozen schema version.
