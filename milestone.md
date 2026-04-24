# milestones — grex

Phased delivery plan from empty repo to v1.0.0 on crates.io. Each milestone has acceptance checks; downstream milestones assume upstream passed.

## M1 — Crate scaffold
Lay the foundation. No behavior yet, just structure that compiles.

- `cargo init --lib` root + `grex-cli` bin crate in workspace.
- `clap` derive CLI with all 12 verb stubs returning `unimplemented!()`.
- GitHub Actions CI matrix: Win/Linux/Mac × stable + beta toolchains.
- `cargo clippy -D warnings`, `cargo fmt --check`, `cargo deny check` wired.
- README skeleton, LICENSE placeholder, `rust-toolchain.toml` pinned.

**Acceptance**: `cargo build --release` succeeds on all 3 OS; `grex --help` lists all 12 verbs; CI green.
**Effort**: 1-2 days.
**Depends on**: nothing.

## M2 — Manifest + lockfile
Persistent state layer. Everything downstream reads/writes through it.

- `grex.jsonl` append-only event log (add/rm/update/sync events).
- `grex.lock.jsonl` resolved-state log (sha + branch + installed_at + actions_hash).
- Event fold → in-memory state struct.
- Atomic compaction: load → write `.tmp` → rename.
- `fd-lock` global workspace lock on manifest.
- `schema_version: "1"` field + migration stub.
- Crash-injection tests (kill mid-write, verify torn-line discard + recovery).

**Acceptance**: property tests (`proptest`) on event commutativity + idempotency; crash-recovery tests green; manifest round-trip preserves semantic state.
**Effort**: 3-5 days.
**Depends on**: M1.

## M3 — Pack model + sync engine
The first universal operation: `grex sync`. Everything else bolts onto this.

- `.grex/pack.yaml` parse (`serde_yaml`) with `schema_version` gate.
- Pack tree walk (child packs recurse via their own `.grex/pack.yaml`).
- Git backend behind `Fetcher` trait; choose `git2` vs `gix` here.
- `grex sync [--recursive]` clones missing, pulls existing, recurses.
- Cycle detection in pack graph (bail on cycle).
- URL → path resolution (respect `path:` override, else last path segment).

**Acceptance**: sync a 3-level nested pack tree end-to-end; cycle detection fires on self-referential pack; `--parallel N` honored. Note: `depends_on` satisfaction checking (external-prerequisite verification at plan phase) is **Stage B scope**, not Stage A. Stage A covers the `children` edge (ownership / walk) only.
**Effort**: 4-6 days.
**Depends on**: M2.

## M4 — Plugin system + lockfile idempotency  [✓ COMPLETE 2026-04-20]
M3 shipped the executor + 7 built-ins directly. M4 formalizes the plugin surface, wires lockfile idempotency, and replaces the two conservative-false predicate stubs.

**Status**: All 5 stages (A–E) shipped. Stages A–D landed on `main` via PR #20 (commit `2175a09`); Stage E landed on `feat/m4-e-plugin-inventory` (commits `aa6dc10` + `3867d80`). See `progress.md` for commit-level detail.

- `ActionPlugin` trait + `Registry` struct; 7 built-ins re-exported via `register_builtins(&mut Registry)`.
- Lockfile `actions_hash` compute + compare; matching hash emits `ExecResult::Skipped`. Executor dispatch switches from direct `Action` enum match to `Registry::get(name)` lookup in this stage (moved from Stage A — 2026-04-20 — to keep A surface-area small: threading `Registry` through `FsExecutor` / `PlanExecutor` cascades into >50 test-constructor changes, cleaner as its own unit).
- `reg_key` real Windows registry probe (`winreg`); `psversion` real PowerShell probe; both keep graceful degradation on non-Windows.
- CLI: `--ref <sha|branch|tag>` override, `--only <glob>` pack filter, lockfile auto-read at sync start + auto-write at sync end.
- Discovery: `register_builtins` is the canonical path in v1; optional `inventory::submit!` behind feature flag `plugin-inventory`.
- Stage order: A (trait + Registry + builtins behind trait; dispatch unchanged) → B (executor dispatch swap + actions_hash / Skipped) → C (reg_key / psversion probes) → D (CLI flags + lockfile r/w) → E (discovery hook).

**Acceptance**:
- ✓ `ActionPlugin` trait + `Registry` shipped with 7 built-ins re-exported through Registry and plugin-layer unit tests (no regression in M3 tests) (M4-A)
- ✓ executor dispatch routed through `Registry::get(name)` (M4-B)
- ✓ `grex sync` twice on unchanged pack → second run emits `ExecResult::Skipped` for every action (M4-B)
- ✓ `reg_key` + `psversion` return real probe results on Windows and `PredicateNotSupported` on non-Windows (M4-C)
- ✓ `grex sync --ref <sha>` overrides pack default ref (M4-C)
- ✓ `grex sync --only <glob>` filters to matching pack paths (M4-C)
- ✓ lockfile reads on startup and writes post-sync (shipped in M3, formalized M4-B)
- ✓ discovery: `register_builtins` canonical path + optional `inventory::submit!` behind `plugin-inventory` feature flag (M4-E)

External plugin loading (dylib/WASM) remains out of scope per v2 backlog.
**Effort**: 6-8 days.
**Depends on**: M3.

## M5 — 3 pack-types + gitignore auto
Pack-type plugin layer. Wires actions into lifecycle.

- `PackTypePlugin` trait with `install / update / teardown / sync` methods.
- Built-ins: `meta` (children-only), `declarative` (runs actions list), `scripted` (runs `.grex/hooks/{setup,sync,teardown}.{sh,ps1}`).
- Gitignore managed-block writer (markers: `# >>> grex managed >>>` / `# <<< grex managed <<<`).
- Teardown semantics: explicit `teardown:` block or reverse-order action rollback.

**Acceptance**: fixture pack of each type installs + teardowns cleanly; `.gitignore` managed block added on `add`, removed on `rm`, user edits outside block preserved.
**Effort**: 3-5 days.
**Depends on**: M4.

## M6 — Concurrency + Lean4 proof
Correctness-by-construction on the scheduler.

- Bounded `tokio::sync::Semaphore` gated by `--parallel N` (default = `num_cpus`).
- Per-pack `<path>/.grex-lock` file (fd-lock) prevents same-pack double-exec.
- Global manifest lock acquired before semaphore slot (ordering prevents deadlock).
- Lean4 project under `lean/`, theorem `Grex.Scheduler.no_double_lock` proving no two tasks hold lock on same resource path simultaneously.
- `lake build` in CI matrix.

**Acceptance**: stress test 100 parallel `grex sync` ops on overlapping trees; no data races; Lean4 `.olean` builds green.
**Effort**: 4-6 days (Lean4 theorem is the long pole).
**Depends on**: M5.

## M7 — MCP-native server (rmcp 1.5) + import + doctor  [✓ COMPLETE 2026-04-23]
Agent-native surface + legacy ingest + integrity.

**Status**: All sub-milestones shipped and squash-merged to `main`:
- M7-1 (MCP server) — PR #25 → `0b80a63`.
- M7-2 (test harness L2-L5) — PR #26 → `e98af8c`.
- M7-3 (mcp-ci-conformance) — PR #28 → `ce01eb5`.
- M7-4a (`grex import --from-repos-json`) — PR #31 → `aa8c7d1`.
- M7-4b (`grex doctor`) — PR #29 → `5ce880e`.
- M7-4c (MIT OR Apache-2.0 dual license) — PR #30 → `262770a`.

Follow-up issues tracked post-merge: #32, #33, #34, #35.


- `crates/grex-mcp` exposes a Path-B MCP server via the `rmcp 1.5.0` framework, NOT a hand-rolled `grex.<verb>` JSON-RPC surface.
- 11 tools registered via `#[tool]` macro on `impl GrexMcpServer`: `init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `import`, `run`, `exec`. Excludes `serve` (the transport itself) and `teardown` (plugin hook).
- Per-tool `read_only_hint` + `destructive_hint` agent-safety annotations. `exec` strips `--shell` (agent safety; CLI keeps `exec --shell` for interactive use).
- Cancellation via rmcp's built-in `local_ct_pool` (HashMap<RequestId, CancellationToken>) — NOT a custom DashMap layer. Per-request `CancellationToken` injected via `FromContextPart`; `notifications/cancelled` routes to it.
- Wired to stdio in `grex serve` (verb-scoped `--manifest`, `--workspace`, `--parallel`; default `RUST_LOG=grex=info,rmcp=warn`).
- `grex import --from-repos-json <path>` reads legacy flat `{url, path}[]` → emits `add` events.
- `grex doctor` — **SHIPPED (M7-4b, 2026-04-22)**: three default checks (manifest schema, gitignore sync, on-disk drift) + opt-in `config-lint` under `--lint-config` (`openspec/config.yaml` + `.omne/cfg/*.md`). `--fix` heals gitignore drift only (unit-tested to prove it never writes to the manifest on schema errors nor re-creates missing pack dirs). Exit codes 0/1/2 roll up via worst severity; property test asserts the invariant.
- License decision: `MIT OR Apache-2.0` dual — locked here. **[M7-4c: SHIPPED on `feat/m7-4c-license`]** workspace migrated to `MIT OR Apache-2.0` across all 4 crates via `license.workspace = true`; `LICENSE-MIT` + verbatim `LICENSE-APACHE` + dual-pointer `LICENSE` landed at repo root; README `## License` rewritten with standard Rust contribution paragraph; `crates/grex/tests/license_metadata.rs` asserts parity via `cargo metadata`.
- See `openspec/changes/feat-m7-1-mcp-server/spec.md` `## Known limitations` + `## rmcp 1.5.0 wiring notes` for known gaps and rmcp 1.5 surface quirks.
- **M7-3 (mcp-ci-conformance) — SHIPPED (PR #28 → `ce01eb5`)**: `mcp-conformance` CI job running `mcp-validator` (Janix-ai, tag `v0.3.1`, SHA `d766d3e…`) against `grex serve` at protocol `2025-06-18`. Self-contained, parallel to the `build` matrix, pinned via git+SHA (PyPI 0.3.1 is unpublished). See `docs/ci/mcp-conformance.md` for pin rationale + bypass procedure. Spec drift corrected: server command is POSITIONAL (not `--server-command`) per upstream `ref_gh_actions/stdio-validation.yml@v0.3.1`.
- **M7-4a (import) — SHIPPED (PR #31 → `aa8c7d1`)**: `grex import --from-repos-json` + `ImportPlan`/`ImportOpts`, dry-run, idempotent re-runs, JSON + human table output, 27 unit + 10 integration cases.
- **M7-4b (doctor) — SHIPPED (PR #29 → `5ce880e`)**: 3 default checks + `--lint-config` opt-in + `--fix` safety contract proven by tests; exit-code roll-up invariant covered by property test.
- **M7-4c (dual license) — SHIPPED (PR #30 → `262770a`)**: workspace now `MIT OR Apache-2.0` via `license.workspace = true`; LICENSE-MIT + verbatim LICENSE-APACHE + dual-pointer LICENSE at repo root.

**Acceptance**: MCP `initialize` handshake works; 11 tools discoverable via `tools/list`; `notifications/cancelled` aborts in-flight `sync` within budget; legacy REPOS.json import produces identical state to manual `grex add` sequence; doctor exits non-zero on known-broken fixtures.
**Effort**: 4-6 days.
**Depends on**: M6.

## M8 — Release v1.0.0
Ship it.

- `cargo-dist` cross-platform binaries (Win/Linux/Mac × x86_64 + arm64).
- Publish to crates.io under final name (`grex` or fallback).
- `docs.rs` builds + hosted docs site (mdBook from `.omne/cfg/`).
- Reference pack template repo published (`grex-pack-template`).
- Changelog + SemVer policy documented.
- `grex-inst` example pack listed as exemplar.

**Acceptance**: `cargo install grex-cli` works fresh on all 3 OS (installs binary `grex`); reference pack repo installs via `grex add`; docs site live.
**Effort**: 2-4 days.
**Depends on**: M7.

## v2 backlog

Deferred from v1 scope. Order indicates rough priority, not schedule.

- **External plugin loading** — dylib (`libloading`) or WASM (`wasmtime` / `extism`) runtime for third-party actions + pack-types.
- **Retro-futurist `ratatui` TUI dashboard** — live pack tree, action streams, lock inspection.
- **Additional pack-types** — `software-list`, `env-bundle`, `dotfiles` (via plugin, not compiled in).
- **Additional actions** — `pkg-install`, `url-download`, `archive-extract`, `file-append`, `patch`, `json-merge`, `template`, `path-add`, `shell-rc-inject` (via plugin).
- **Extra Lean4 proofs** — idempotency, commutativity of independent actions, crash-safety of manifest fold.
- **SQLite optional backend** — alternative to JSONL for very large workspaces.
- **Self-update** — `grex upgrade` via GitHub releases.
- **Pack registry (`grex.dev`)** — hosted index of discoverable packs.
- **Embedded scripting** — Lua / Rhai as middle ground between declarative YAML and shell escape.
