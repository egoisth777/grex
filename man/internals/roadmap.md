# roadmap

Content scope by release. Timeline is ordering + dependencies, not dates.

## v1 — Pack-based orchestrator, stable core

Ships all 7 philosophy principles (see [goals.md](../concepts/goals.md)).

### Core always compiled
- Manifest (JSONL intent log).
- Lockfile (JSONL resolved state).
- Scheduler (tokio + bounded semaphore + per-pack `.grex-lock` + `fd-lock`).
- Sync engine (git clone/pull, recursion).
- Gitignore automation (managed-block markers).
- MCP stdio JSON-RPC server.
- Pack discovery (`.grex/pack.yaml`).
- Action plugin registry + 7 built-in actions.
- Pack-type plugin registry + 3 built-in pack-types.
- Atomic file writes (temp + rename).
- Lean4 proof `Grex.Scheduler.no_double_lock`.

### Frozen public APIs
1. `.grex/pack.yaml` schema (v1).
2. `grex.jsonl` event schema.
3. `grex.lock.jsonl` schema.
4. `ActionPlugin` trait.
5. `PackTypePlugin` trait.
6. `Fetcher` trait.
7. CLI verb surface (12 verbs).
8. MCP method surface (1:1 with CLI).

### Explicitly NOT in v1
- External plugin loading.
- TUI.
- Non-git fetchers.
- Additional pack-types / actions beyond the built-ins.
- Pack registry.
- Self-update.

Exit criteria: all success criteria in the feature spec PASS in CI matrix; crates.io publish successful; reference pack repo installs cleanly.

## v2 — Extensibility & aesthetics

Opens third-party extension; adds TUI + non-git fetchers.

### External plugin loading
Two candidate routes evaluated in v2 alpha:

- **Dylib** (`libloading` + `abi_stable`): native speed, strict ABI versioning.
- **WASM** (`wasmtime` / `extism`): sandboxed, forward-compatible, syscall bridging required.

Decision in v2 alpha; both may ship (host selects by file extension).

### Retro-futurist TUI
`ratatui`-based dashboard, feature-flagged `--features tui`. Live pack tree, per-pack sync stream, lock inspector, CRT glyph aesthetic. Falls back to plain ANSI when `--plain` or non-TTY.

### Additional pack-types (via plugin)
- `software-list` — iterates package installs (winget/brew/apt).
- `env-bundle` — manages a coherent group of env vars + PATH entries.
- `dotfiles` — dotfile-manager style: iterate + symlink.

### Additional actions (via plugin)
`pkg-install`, `url-download`, `archive-extract`, `file-append`, `patch`, `json-merge`, `template`, `path-add`, `shell-rc-inject`.

### Additional Lean4 proofs
- I2: manifest append serialization under fd-lock.
- I3: `.gitignore` managed-block idempotence.
- I4: compaction fold-equivalence.
- Commutativity of disjoint-path events.

### SQLite optional backend
Feature flag `sqlite`. Same `Manifest` API. For users with >100k events.

### Self-update
`grex upgrade` pulls latest release from GitHub.

### Embedded scripting
Lua or Rhai in-process scripting — middle ground between declarative YAML and full shell escape. Candidate for a pack-type plugin in v2.

### Non-git fetchers
`rclone`, `s3`, `oci`, `http` — all implement the `Fetcher` trait. `grex add` accepts `--scheme <rclone|s3|...>` or auto-detects from URL.

## v3+ — Scale & federation

Exploratory. No commitments yet.

- **Pack registry** (`grex.dev`) — hosted index of discoverable packs.
- **Rules engine** — `.rules.yaml` per pack, enforced on add/sync (modeled after metarepo's rules plugin).
- **Org-level federation** — multiple top-level workspaces referencing each other.
- **Interactive HTTP dashboard** — `grex serve --http` with web UI.
- **Distributed locking** — optional consul/etcd for multi-host deployments.
- **p2p fetchers** — IPFS, BitTorrent.
- **Supply-chain signing** — pack signatures; registry-enforced integrity.

## Non-roadmap (never)

- Cross-VCS support (hg, svn, fossil, perforce).
- Monorepo conversion tooling.
- Git replacement.
- Generic CI runner.
- Full `.gitmodules` semantic replacement.

## Dependency ordering (cross-release)

```
v1 (frozen APIs)
  └─► v2 external plugin loading
        └─► v2 additional pack-types + actions (as plugins)
        └─► v2 non-git fetchers (via Fetcher trait impls)
  └─► v2 TUI (independent)
  └─► v2 SQLite backend (independent)
  └─► v3 pack registry (needs plugin signing story)
```
