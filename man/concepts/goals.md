# goals

Philosophy, competitive positioning, and scope for `grex` v1.

## Philosophy (7 principles)

1. **Git repo is a universal container** for machine-configurable state. Configs, tools, env declarations, symlink trees, install manifests all ride on git: free versioning, distribution, diffing, authorship.

2. **Pack = git repo + `.grex/` directory.** The `.grex/` dir is the **contract** grex understands. Pack content outside `.grex/` is **opaque** to grex. A pack is just a git repo that opts into the protocol.

3. **Every pack is a meta-pack.** Uniform model. Packs can nest child packs. Leaf packs just have zero children. No special-casing in code.

4. **Repo sync is a universal op**, orthogonal to pack-type. Every pack gets `grex sync` (git fetch/pull + recurse into children) for free. Install / update / teardown are per-pack-type.

5. **Extensibility is vital.** grex cannot precompile every install or config logic. The **action vocabulary** and **pack-types** are plugin interfaces. v1 ships a small built-in set compiled in; v2 opens external plugin loading.

6. **Future-proof core, pragmatic content.** Stable schemas + trait APIs at v1. Action vocabulary stays small (YAGNI) but grows via plugin contributions over time.

7. **Agent-native.** Embedded MCP stdio JSON-RPC server exposing all CLI verbs 1:1. Not a subprocess wrapper — handlers call the same library entrypoints the CLI dispatcher calls.

Cross-cutting: **blazingly fast** via Rust + tokio. All built-in actions are native Rust (no shell fork). A shell escape hatch exists (`exec` action, `scripted` pack-type) but is the last resort, not the default.

## Competitive positioning

| Axis | `codyaverett/metarepo` | `grex` |
|---|---|---|
| Domain | git repos only | any resource via pack protocol |
| Concurrency | sequential | tokio parallel, bounded semaphore |
| State | intent-only | intent + lockfile (separate files) |
| Atomic writes | no | yes (temp + rename always) |
| MCP | subprocess wrapper | embedded in-process server |
| Lean4 proof | no | 1 scheduler invariant v1 |
| Nesting | via sub-meta | uniform (every pack = meta) |
| Extension | code changes only | trait-based plugin registry |
| Cross-plat | yes | yes + explicit Win/Linux/Mac CI matrix |

## v1 shippable scope

### Core (always compiled)
- Manifest (JSONL, intent events)
- Lockfile (JSONL, resolved SHA + state, separate file)
- Scheduler (tokio + bounded semaphore + per-pack `.grex-lock` + `fd-lock` global)
- Sync engine (git clone/pull, recurse into children)
- Gitignore automation (managed block markers)
- MCP server (stdio JSON-RPC 2.0, methods = CLI verbs 1:1)
- Pack discovery (`.grex/pack.yaml` parse)
- Action executor + in-process action plugin registry
- Pack-type executor + in-process pack-type plugin registry
- Atomic file writes (temp + rename always)
- Lean4 invariant proof (no double-lock on same resource path)

### Built-in pack-types (3)
- `meta` — nests children, no own actions.
- `declarative` — runs Tier 1 actions from `pack.yaml`.
- `scripted` — escape hatch; runs `.grex/hooks/{setup,sync,teardown}.{sh,ps1}`.

### Built-in actions (7 Tier 1, grounded in real E:\repos scripts)
1. `symlink` — create/update symlink w/ backup, idempotent, cross-platform.
2. `env` — set env var (user / machine / session scope).
3. `mkdir` — idempotent dir creation (parents).
4. `rmdir` — remove dir, optional backup.
5. `require` — prereq / idempotency gate (path-exists, cmd-available, reg-key, os, psversion, symlink-ok).
6. `when` — platform / conditional gate wrapping nested actions.
7. `exec` — shell escape (array-form cmd, no shell-parse by default).

### CLI verbs (12, frozen contract)
`init add rm ls status sync update doctor serve import run exec`

### Stable public APIs (breaking changes forbidden post-v1 without major bump)
1. `.grex/pack.yaml` schema (with `schema_version: "1"`).
2. `grex.jsonl` manifest schema.
3. `grex.lock.jsonl` lockfile schema.
4. `ActionPlugin` Rust trait.
5. `PackTypePlugin` Rust trait.
6. `Fetcher` Rust trait.
7. CLI verb surface.
8. MCP method surface (= CLI verbs 1:1).

## v2 backlog (NOT v1)

- External plugin loading (dylib via `libloading` or WASM via `wasmtime` / `extism`).
- Retro-futurist `ratatui` TUI dashboard.
- Additional pack-types (`software-list`, `env-bundle`, `dotfiles`) via plugin.
- Additional actions (`pkg-install`, `url-download`, `archive-extract`, `file-append`, `patch`, `json-merge`, `template`, `path-add`, `shell-rc-inject`) via plugin.
- Extra Lean4 proofs (idempotency, commutativity, crash-safety of manifest fold).
- SQLite optional backend for very large workspaces.
- Self-update (`grex upgrade`).
- Pack registry (`grex.dev`).
- Embedded scripting (Lua / Rhai) — middle ground between declarative YAML and shell escape.

## Non-goals (permanent)

- Monorepo conversion.
- Git submodule full replacement.
- Cross-VCS support (hg, svn, fossil, perforce).
- Language-specific build orchestration.
- Generic CI runner.

## Grounded reality — action-vocab rationale

Scanned real-world `E:\repos` scripts: 3 PowerShell scripts, 945 LOC total. Pattern frequencies:

| Pattern | Count | v1 Action |
|---|---|---|
| `symlink-create` | 8 | `symlink` |
| `idempotency-check` | 9 | `require` |
| `env-set` | 7 | `env` |
| `exec-cmd` (chain scripts) | 5 | `exec` |
| `dir-create` | 2 | `mkdir` |
| `platform-gate` | 2 | `when` |
| `dir-remove` (backup pattern) | 1 | `rmdir` |
| package installs | 0 | deferred v2 plugin |
| JSON merges | 0 | deferred v2 plugin |
| archive extracts | 0 | deferred v2 plugin |

The 7-primitive Tier 1 vocab is **grounded**, not speculated. Everything else is deferred to v2 plugin contributions.
