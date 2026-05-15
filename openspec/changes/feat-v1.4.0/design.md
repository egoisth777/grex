---
slug: feat-v-1-4-0-design
type: spec
status: active
last_updated: 2026-05-15
topic: stub-verb-completion-design
---

# feat-v1.4.0 — design

## Verb dispatch and exit-code contract

Existing `crates/grex/src/cli/verbs/mod.rs::emit_unimplemented_json` stays for **forward-compat with unknown verbs** but is no longer reachable from the six listed verbs. Each verb gains a real `run_impl` mirroring `sync.rs` / `teardown.rs`:

```rust
pub fn run(args, global, cancel) -> Result<()> {
    match run_impl(...) {
        Outcome::Ok        => Ok(()),
        Outcome::Validation=> std::process::exit(1),
        Outcome::Exec      => std::process::exit(2),
        Outcome::Tree      => std::process::exit(3),
        Outcome::Usage     => std::process::exit(2),
    }
}
```

`anyhow::Error` does not carry exit codes; the `std::process::exit` calls are the established pattern (see `sync.rs` line 88+).

## V1 — `init`

- **Args:** add optional positional `path: Option<PathBuf>` to `InitArgs` (was empty).
- **Behavior:** resolve `path` → cwd default → create dir if missing → write `<dir>/.grex/pack.yaml` with the minimal manifest.
- **Idempotency:** detect existing `<dir>/.grex/pack.yaml`; surface `already initialized` and exit 1.
- **JSON envelope:** `{"verb":"init","status":"ok","path":"<absolute>"}` or `{"verb":"init","error":{"kind":"already_initialized","path":...}}`.

Minimal manifest body (matches existing `crates/grex-core/src/pack/mod.rs` v1 schema):

```yaml
schema_version: "1"
pack_type: meta
actions: []
children: []
```

## V2 — `rm`

- **Args:** `path: String` (already present) + new `--force` boolean.
- **Behavior:**
  1. Resolve `<path>` to absolute pack root.
  2. Load pack manifest; if `pack_type: meta` and `children` non-empty and `--force` not set → exit 1 `"refusing to remove meta-pack with children"`.
  3. Run `sync::teardown` on the subtree.
  4. Delete the directory via existing `grex_core::fs::rmtree` helper (preserved across platforms).
- **Exit codes:** mirror sync (1 / 2 / 3).
- **JSON:** `{"verb":"rm","status":"ok","path":...}` or error envelope.

## V3 — `update`

- **Args:** `pack: Option<String>` (already present).
- **Behavior:** delegate to `verbs::sync::run` with a `SyncArgs` whose `pack_root = args.pack` (resolved via the same cwd helper) and all other fields default. **No new code path** — pure CLI alias.
- **Discoverability:** keep separate `Update` clap variant so `--help` lists it; the alias is implementation-only.

## V4 — `status`

- **Args:** add optional positional `pack_root: Option<PathBuf>` to `StatusArgs`.
- **Behavior:**
  1. Resolve `pack_root` via cwd helper.
  2. Build `SyncOptions::new().with_dry_run(true).with_validate(true)`.
  3. Call `sync::run`.
  4. Render per-pack state from the resulting `SyncReport`:
     - `clean` — no executed steps (`steps` empty for that pack).
     - `would-update N` — N steps would fire in wet-run.
     - `missing` — pack failed validation (planner halted).
  5. Never mutate state (already guaranteed by `dry_run=true`; B4 contract preserved).
- **Exit codes:** 0 clean, 0 with drift, 1 validation, 2 tree.
- **JSON:** `{"verb":"status","clean":bool,"packs":[{"path":...,"state":..., "drift_count": N}]}`.

## V5 — `run`

- **Args:** `action: String` (already present) + new optional `pack_root: Option<PathBuf>`.
- **Behavior:**
  1. Resolve pack root.
  2. Walk pack tree (`Walker::walk`).
  3. For each `PackNode`, filter `manifest.actions` to those whose `name == action` (string match).
  4. Build a synthetic `PlanExecutor` invocation with only the filtered steps.
  5. Execute via existing `execute::PlanExecutor::run`.
- **Filter scope:** v1.4.0 ships exact-string match. Glob/regex filters deferred.
- **No-match handling:** if zero packs declare the action, exit 0 with informational message (not an error — operator may have run against the wrong subtree).
- **JSON:** `{"verb":"run","action":...,"matched_packs":N,"steps":[...]}`.

## V6 — `exec`

- **Args:** `cmd: Vec<String>` (already present, `trailing_var_arg`) + new optional `--pack <path>`.
- **Behavior:**
  1. Resolve pack root (`--pack` overrides cwd default).
  2. Spawn `cmd[0]` with `args=&cmd[1..]` and `current_dir = pack_root`.
  3. Inherit stdio in human mode; capture in `--json` mode.
- **Shell semantics:** `cmd[0]` is the program name, not a shell string. Operators wanting shell expansion use `sh -c '...'` / `pwsh -Command ...`.
- **Exit code:** child exit code (clamped: `>=128` (signal-killed) maps to 125 to avoid clashing with sync band).
- **JSON:** `{"verb":"exec","cwd":...,"exit_code":N,"stdout":...,"stderr":...}`.

## V7 — `add` path-collision (B15)

- **Where:** `crates/grex/src/cli/verbs/add.rs` before the `grex_core::add::add_pack` call.
- **Check:** load parent manifest (already loaded for register), scan `children` for an entry whose normalized `path` equals the candidate path.
- **On collision:** emit warn-then-error (stderr `"grex add: path '<p>' already registered to <url>; not adding"`) and exit 1 without invoking `add_pack`.
- **Idempotency vs collision:** an exact URL+path match could be treated as a no-op success; v1.4.0 ships the conservative path (always exit 1 on collision) so operators can't unknowingly "re-add" a stale entry.

## Test strategy

One CLI integration test per verb under `crates/grex/tests/`:

| verb | new test file |
|------|---------------|
| init   | `init_cli.rs` |
| rm     | `rm_cli.rs` |
| update | `update_cli.rs` (delegates to sync — minimal smoke that exit code matches) |
| status | `status_cli.rs` |
| run    | `run_cli.rs` |
| exec   | `exec_cli.rs` |
| add B15 | extend `add_cli.rs` |

Tests follow the assert_cmd / tempfile pattern already in `crates/grex/tests/sync_e2e.rs`. Each covers: happy-path exit code, `--json` envelope shape, one error path (where applicable).

## Out-of-scope cross-checks

- No Lean4 obligations (rule 8 exemption — pure plumbing).
- No MCP surface change. The MCP tool dispatcher already maps `init`/`rm`/`update`/`status`/`run`/`exec` to the same handlers used by CLI; they pick up the new behavior automatically. Existing `crates/grex-mcp/tests/` smoke tests will catch surface drift.
- No manifest schema change.
