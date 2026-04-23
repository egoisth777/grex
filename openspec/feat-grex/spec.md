# feat-grex — Pack-based cross-platform dev-env orchestrator

## Problem

Dev-environment setup across machines is fragmented: a handful of config repos, per-tool install scripts, hand-written PowerShell/bash for symlinks + env vars, no uniform sync story, no lockfile, no agent-native control surface. Git submodules are brittle. Package managers install tools but don't orchestrate config placement, declarative env state, or multi-repo sync. No existing OSS tool combines a **pack abstraction** (git repo + contract dir) with declarative actions, a lockfile, and embedded MCP for agents.

## Goal

Ship a Rust CLI `grex` that orchestrates **packs** — git repos bearing a `.grex/` contract directory — across Windows, Linux, and macOS. Deliver unified repo sync, three built-in pack-types, seven built-in cross-platform actions, a Lean4-verified scheduler invariant, and an embedded MCP JSON-RPC server. Keep the core extensible: new action types and pack-types plug in via Rust traits with zero grex-core recompile for authors writing their own plugin modules, and (v2) zero recompile for third-party dylib/WASM plugins.

## Non-goals

- Monorepo conversion tooling.
- Full git submodule replacement (only covers "sub-repo fetch/sync"; does not reproduce `.gitmodules` semantics end-to-end).
- Non-git fetchers (rclone, S3, OCI) in v1 — deferred to v2.
- Retro-futurist TUI dashboard in v1 — deferred to v2.
- Language-specific build orchestration.
- Generic CI runner.

## Success criteria

1. `grex init`, `add`, `rm`, `sync`, `update`, `doctor`, `import` produce expected manifest + lockfile + on-disk state across Windows, Linux, and macOS (CI matrix integration tests).
2. `grex serve` speaks MCP protocol version `2025-06-18` natively (stdio, newline-delimited JSON). Every CLI verb except `serve` itself is exposed as an MCP tool invoked via `tools/call` — 11 tools total: `init`, `add`, `rm`, `ls`, `status`, `sync`, `update`, `doctor`, `import`, `run`, `exec`. Tracing routes to stderr only; stdout carries only the JSON-RPC wire. The M6 5-tier lock ordering (workspace-sync → scheduler → pack-lock → backend → manifest) is preserved by every tool handler.
3. A `declarative` pack exercising all 7 action types installs correctly on each of the three OSes.
4. A `scripted` pack runs `.grex/hooks/setup.{sh,ps1}` on the matching OS and no-ops on the others.
5. A `meta` pack with nested children syncs the tree recursively in parallel under the `--parallel N` bound.
6. Manifest + lockfile round-trip survives crash injection (SIGKILL / TerminateProcess mid-write) and torn lines are discarded on next load.
7. Lean4 proof `Grex.Scheduler.no_double_lock` compiles cleanly under `lake build` with zero `sorry` / zero unresolved `axiom` in deliverable scope.
8. `pack.yaml` has `schema_version: "1"`; v1 packs remain readable by future v2 without breaking.
9. `ActionPlugin`, `PackTypePlugin`, `Fetcher` traits are documented and used internally by every built-in plugin (proof of extensibility by construction).

## Architecture summary

Single crate `grex` (bin + lib). CLI verbs dispatch into the library. Data flow: CLI parse → manifest load (fold JSONL events) → pack tree walk (parse `.grex/pack.yaml` under each registered path, recurse through children) → pack-type plugin dispatch (`install`/`update`/`teardown`/`sync`) → action plugin registry execution → lockfile write → gitignore managed-block sync. Concurrency: tokio multi-thread runtime with a bounded semaphore, per-pack `.grex-lock` file via `fd-lock`, global manifest lock. Extensibility: in-process trait-object registries for `ActionPlugin` + `PackTypePlugin` + `Fetcher`; v2 adds external loading. Full module layout in [../../.omne/cfg/architecture.md](../../.omne/cfg/architecture.md).

## Out of scope v1

- External plugin loading (dylib / WASM).
- Retro-futurist TUI.
- Non-git fetchers (rclone, S3, OCI, HTTP).
- Pack-types beyond the 3 built-ins.
- Actions beyond the 7 Tier 1 primitives.
- Hosted pack registry (`grex.dev`).
- Self-update command.

## Dependencies

**Rust crates:**

| Crate | Purpose |
|---|---|
| `tokio` | async runtime |
| `clap` | CLI parsing |
| `serde`, `serde_yaml`, `serde_json` | schema I/O |
| `simd-json` (optional feature) | fast manifest fold |
| `gix` or `git2` | git operations (choice at M3) |
| `fd-lock` | cross-platform file locking |
| `anyhow` | binary error propagation |
| `thiserror` | typed library errors |
| `tracing`, `tracing-subscriber` | structured logs |
| `comfy-table` | `ls`/`status` tables |
| `owo-colors` | ANSI color w/ TTY detect |
| `async-trait` | async trait objects |
| `inventory` | plugin registration |

**Dev dependencies:**

| Crate | Purpose |
|---|---|
| `proptest` | property tests |
| `assert_cmd` | CLI integration tests |
| `tempfile` | test fixtures |

**External binaries:**

- `git` CLI (fallback for operations not covered by chosen Rust backend).
- OS symlink APIs (via `std::os::{unix,windows}::fs`).
- `lake` + `lean` (CI-only, for proof job).

## Acceptance

All success-criteria items PASS in the GitHub Actions matrix (Windows + Ubuntu + macOS × stable + beta toolchains). Lean4 `.olean` builds clean. `cargo install grex-cli` works from crates.io on all three OSes (the published crate is `grex-cli`; the installed binary is `grex`). At least one reference pack repo (`grex-inst` or successor) is published as an installable example.

## M3 Stage B — Variable expansion (slice 1)

Action-argument strings in `pack.yaml` carry variable placeholders as literals (parser stays pure; see Stage A). Expansion is a **pure transformation** applied at action-execute time against an environment map.

### Requirements

1. **Supported forms** (equivalent, author-choice):
   - `$NAME` — POSIX bare form. NAME boundary is the first byte not matching `[A-Za-z0-9_]` or end-of-input.
   - `${NAME}` — POSIX braced form. Must close with `}`.
   - `%NAME%` — Windows form. Must close with a second `%`.
2. **Escapes** (the only escapes recognised):
   - `$$` → literal `$`.
   - `%%` → literal `%`.
   - Backslash escapes (`\$`, `\%`) are NOT recognised; backslash passes through literally.
3. **Platform scoping**: POSIX forms (`$NAME`, `${NAME}`) are accepted on every platform. `%NAME%` is also accepted on every platform at parse/expand time — `pack.yaml` is cross-platform-authored. Whether the variable resolves is determined purely by the env map passed at expand time.
4. **Variable NAME regex**: `^[A-Za-z_][A-Za-z0-9_]*$`. Names violating this regex produce `InvalidVariableName` at expand time. Parser does NOT validate names — pack-parse stays a pure structural transform.
5. **Missing variable policy**: a well-formed placeholder whose NAME is not present in the env map produces `MissingVariable { name, offset }`. Error is actionable (includes the name and byte offset into the input string).
6. **Malformed placeholders**: `${FOO` (unclosed brace), `%FOO` (unclosed percent), `${}` (empty brace), `trailing$` (bare `$` with no following name char), `50% off` (isolated `%` with no second `%`) all produce typed errors at the offset of the opening token. Single `%` in the middle of a string is treated as an unclosed percent expansion — literal `%` requires `%%`.
7. **No recursive expansion**: the expanded value is not re-scanned. If `$A` expands to `$B`, the final string contains the literal four bytes `$B`.
8. **API shape** (`crates/grex-core/src/vars/`):
   - `pub fn expand(input: &str, env: &VarEnv) -> Result<String, VarExpandError>`
   - `VarEnv::new() / from_os() / insert / get`
   - `VarExpandError` is `thiserror`-derived; `Display` messages include offset and, where applicable, variable name.
9. **Platform-specific casing**: Stage B slice 1 stores env keys case-sensitively on every platform. Windows case-insensitive lookup is deferred to a later slice when wiring into the exec context; documented on `VarEnv::from_os`.

### Out of scope for slice 1

- Wiring expansion into action execute path (slice 5).
- Windows-specific case-insensitive env (later slice).
- Auto-mapping `$HOME` → `%USERPROFILE%` on Windows (documented in actions.md; implemented when wiring env context).

## M4 — Plugin system (Stage A slicing)

**Status (2026-04-20)**: All 5 stages shipped. Stages A–D on `main` via PR #20 (commit `2175a09`); Stage E on `feat/m4-e-plugin-inventory` (commits `aa6dc10` + `3867d80`). See `progress.md` for commit SHAs + per-stage detail.

M3 landed the action executor and all 7 Tier 1 actions directly inside `grex-core::execute`. M4 formalizes plugin extensibility (trait + registry) and wires the lockfile idempotency path (`ExecResult::Skipped`) that PR #14 reserved. External plugin loading (dylib / WASM) stays deferred to v2; in-process registration is the only loading path in v1.

### Requirements

1. **`ActionPlugin` trait** at `crates/grex-core/src/plugin/mod.rs`. Method signatures (exact):
   - `fn name(&self) -> &str`
   - `fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError>`
   (2026-04-20 — aligned with shipped trait in M4-B review fix.) Sync `fn` (not `async`); takes the typed `&Action` (not raw `&Value`); returns the richer `ExecStep` envelope (not the retired `ActionOutcome`). The async + `&Value` shape is reserved for v2 external plugin loading (dylib/WASM) where the trait crosses an ABI boundary. Rollback is NOT on the trait surface; per-action inverse logic stays in the executor. Promoting rollback to a trait method is an M5+ decision if pack-type drivers require it.
2. **`Registry` struct** with methods:
   - `fn register<P: ActionPlugin + 'static>(&mut self, plugin: P)`
   - `fn get(&self, name: &str) -> Option<&dyn ActionPlugin>`
   - `fn bootstrap() -> Self` — returns a `Registry` pre-populated with all 7 built-ins via `register_builtins(&mut reg)`.
3. **Built-in re-export** (Stage A): the 7 current built-ins (`symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`) move behind the `ActionPlugin` trait and are re-exported from `grex-core::plugin`. Stage A keeps executor dispatch on the existing direct `Action` enum match — the `Registry` is a parallel surface exercised only by plugin-layer unit tests. **Executor dispatch swap is Stage B** (see §4a below), deferred 2026-04-20 because threading `Registry` through `FsExecutor` / `PlanExecutor` cascades into >50 test-constructor changes; cleaner as its own unit. The `Action` enum stays as the parsed form; post-Stage-B the trait layer is the execution form.
4. **Executor dispatch swap (Stage B)**: executor dispatch becomes `registry.get(action.name()).ok_or(UnknownAction)` in place of the direct match on the parsed `Action` enum. Landed together with lockfile idempotency (§4a) because both stages thread `Registry` / lockfile state through the executor constructors.
4a. **Lockfile `actions_hash`** (Stage B): computed per pack as sha256 of canonical JSON of the pack's `actions:` list plus the resolved commit sha. On sync, if the stored hash equals the recomputed hash the executor emits `ExecResult::Skipped { pack_path, actions_hash }` and performs no work for that pack. Stored in the existing lockfile JSONL via a new `Skipped` event; the variant was reserved in PR #14.
5. **Real predicate probes**:
   - `reg_key`: Windows uses the `winreg` crate (`RegOpenKeyEx` + `RegQueryValueEx`); non-Windows returns `PredicateNotSupported`.
   - `psversion`: Windows probes `$PSVersionTable.PSVersion` via `powershell.exe -NoProfile -Command`; non-Windows returns `PredicateNotSupported`.
   Both replace the conservative-false stubs flagged in M3 open questions.
6. **CLI additions**:
   - `--ref <sha|branch|tag>` — global override of a pack's default ref at sync time.
   - `--only <glob>` — filters `grex sync` to matching pack paths (glob matching via `globset`).
   - Lockfile is auto-read at sync start (already wired in M3; M4 formalizes the read path for `Skipped` comparison) and auto-written at sync end.
7. **Discovery**: M4-E lands `register_builtins(&mut Registry)` as the canonical registration path. Optional `inventory::submit!` auto-registration lives behind the feature flag `plugin-inventory` (default off in v1) so `grex-core` carries no hard `inventory` dependency. External dylib / WASM loading remains v2.

### Out of scope

- External plugin loading: dylib (`libloading`), WASM (`wasmtime` / `extism`), `abi_stable` wiring.
- Third-party crate plugin distribution (out-of-repo plugins).
- Rollback as a trait method — per-action inverse logic stays in the executor (the retired `ActionOutcome` shape is not the home; `ExecStep` is the v1 per-action envelope). Promote to trait in M5+ if pack-type drivers require it.
- `PackTypePlugin` trait work — that is M5 scope per `milestone.md`.

## M5 — Pack-Type Plugin System

**Status (2026-04-20)**: Spec drafted; implementation pending. M4 plugin system (`ActionPlugin` trait + `Registry` + dispatch swap) is the prerequisite and has landed on `main`. M5 adds the parallel pack-type layer and retires the enum-match dispatch on `PackManifest.r#type`.

M4 formalized action extensibility. M5 does the same for pack-types: the 3 built-ins (`meta`, `declarative`, `scripted`) move behind a `PackTypePlugin` trait, dispatch is by string lookup in a `PackTypeRegistry`, and teardown semantics are nailed down per pack-type. Gitignore synchronization moves to a per-pack managed-block writer so each pack owns its own ignore section without clobbering user edits. External pack-type plugin loading stays deferred to v2.

### Requirements

1. **`PackTypePlugin` trait** at `crates/grex-core/src/plugin/pack_type.rs`. Method signatures (exact, aligned to M4 ground-truth `ActionPlugin` shape):
   - `fn name(&self) -> &str`
   - `async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>`
   - `async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>`
   - `async fn teardown(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>`
   - `async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>`
   Async methods use 2024-edition native async-in-trait. Fallback to the `async_trait` macro only if a blocking toolchain issue surfaces during implementation; document the switch in the commit that flips it.
2. **`PackTypeRegistry` struct** parallel to the M4 action `Registry`. Methods:
   - `fn register<P: PackTypePlugin + 'static>(&mut self, plugin: P)`
   - `fn get(&self, name: &str) -> Option<&dyn PackTypePlugin>`
   - `fn bootstrap() -> Self` — returns a registry pre-populated with the 3 built-ins via `register_builtins(&mut reg)`.
3. **Built-in pack-types (3)**: `meta`, `declarative`, `scripted` register by default through `register_builtins`. No other pack-types ship in v1.
4. **Executor dispatch by string**: pack-type dispatch becomes `pack_type_registry.get(pack.r#type.as_str()).ok_or(UnknownPackType)`. The prior `match pack.r#type` on the `PackType` enum is retired at dispatch time. `PackManifest.r#type` stays the typed `PackType` enum in the parsed manifest (mirrors the post-M4-B action story: enum as parsed form, trait as execution form); the registry lookup reads its string view via `PackType::as_str()` or equivalent.
5. **`meta` semantics**: `meta` cooperates with the outer tree walker to achieve depth-first sequential traversal of children in registration order. In M5-1, the tree walker (`tree::walker`) performs the actual recursion and post-order dispatch — `MetaPlugin` emits synthesized envelope `ExecStep`s per child (one nested `noop_step` per `ChildRef`) for observability without itself re-entering the registry. Direct registry-based recursion from `MetaPlugin` via `load_child_manifest` + `ctx.pack_type_registry` lands in M5-2 together with cycle detection (`HashSet<PathBuf>` of visited pack roots) so the registry path cannot bypass the walker's existing cycle guard. A child error aborts the remaining chain and bubbles up; no partial-continue. Parallel meta execution is out of scope (R-M5-out-1). The trait method returns a `Future`, so parallelization can be added later without an API break.
6. **`declarative` semantics**: runs the manifest's `actions:` list through the M4 `ActionPlugin` registry. No new executor — declarative dispatch is a thin shim that walks `actions:` and delegates each action to `action_registry.get(action.name())`.
7. **`scripted` semantics**: executes OS-matched hooks under `.grex/hooks/` relative to the pack root:
   - `install` → `setup.sh` (Unix) / `setup.ps1` (Windows)
   - `sync` / `update` → `sync.sh` / `sync.ps1`
   - `teardown` → `teardown.sh` / `teardown.ps1`
   Missing hook file = no-op (success, no output). OS match is compile-time `cfg!(windows)` choosing `.ps1`, else `.sh`.
8. **Gitignore managed-block writer**: each pack owns a block in the target `.gitignore` delimited by:
   - Opening marker: `# >>> grex:<pack-name> >>>`
   - Closing marker: `# <<< grex:<pack-name> <<<`
   The writer creates the block at end of file if absent, replaces its body in place if present, and never touches bytes outside any grex-managed block. Per-pack block scope — one pack = one section. Teardown deletes only that pack's block (both markers and body).
9. **Teardown — declarative**: prefers an explicit `teardown:` block in `pack.yaml` when present (run as an action list via the M4 registry). When absent, falls back to auto-reverse: walk the `actions:` list in reverse order and apply the inverse of each action:
   - `copy` → delete target
   - `symlink` → unlink target
   - `mkdir` → `rmdir` target if empty (skip if non-empty — no recursive delete)
   - `write` → delete target file
   - `env`, `exec`, `when`, `require` — no-op on teardown (documented; no implicit reversal)
10. **Teardown — scripted**: runs `teardown.{sh,ps1}` matched to OS per R-M5-07. Missing hook = no-op.
11. **Teardown — meta**: runs children's teardown in reverse registration order (LIFO). A child teardown error aborts the remaining chain and bubbles up, matching the install-direction contract.
12. **Plugin-inventory integration**: the `plugin-inventory` feature flag (landed in M4-E for actions) extends to pack-type plugins. Each built-in pack-type submits via `inventory::submit!` under the same feature gate. Default-off in v1; `register_builtins` remains the canonical path.

### Trait signature sketch

```rust
pub trait PackTypePlugin: Send + Sync {
    fn name(&self) -> &str;
    async fn install(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;
    async fn update(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;
    async fn teardown(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;
    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &PackManifest) -> Result<ExecStep, ExecError>;
}
```

`ExecCtx<'_>` is the M4-shipped context struct (`crates/grex-core/src/execute/ctx.rs:96-146`) — carries `vars: &VarEnv` (env resolver), `pack_root: &Path`, `workspace: &Path`, `platform: Platform`, and `registry: Option<&Arc<Registry>>` (action registry handle for nested dispatch). M5 reuses it verbatim rather than introducing a parallel `Context` type. `ExecCtx` already has `registry: Option<&'a Arc<Registry>>` for action-plugin nested dispatch; M5 adds a parallel `pack_type_registry: Option<&'a Arc<PackTypeRegistry>>` field (or a unified registry bundle — implementation choice left to M5-1). `PackManifest` is the parsed manifest struct (`crates/grex-core/src/pack/mod.rs:171-197`) — the M4 ground-truth name is `PackManifest`, not `Pack`. `ExecStep` is the per-step result envelope shipped with M4's `ActionPlugin`; the pack-type trait returns it directly (same error type `ExecError`) rather than introducing a parallel `Outcome` type. Aggregation across multiple child steps (for `meta` and `declarative`) is an executor-level concern, not a trait-level one.

#### Ground-truth references

- M4 `ActionPlugin` trait: `crates/grex-core/src/plugin/mod.rs:49-62` (signature pattern reused by `PackTypePlugin`).
- `ExecCtx<'a>`: `crates/grex-core/src/execute/ctx.rs:96-146` (reused as-is; extended with pack-type registry handle).
- `PackManifest`: `crates/grex-core/src/pack/mod.rs:171-197` (parsed manifest struct — canonical name is `PackManifest`).
- `ExecStep` / `ExecError`: same module as `ActionPlugin` (`crates/grex-core/src/plugin/mod.rs`); reused verbatim for pack-type return type.
- `PackManifest.teardown: Option<Vec<Action>>` is **already parsed** by M4 (see `pack/mod.rs:186-193`); R-M5-09's "explicit `teardown:` path" just reads this field — no new parse work is required in M5.

### Gitignore block contract

**Before** (user-authored `.gitignore`):
```
# user ignores
node_modules/
*.log
```

**After `grex sync` of pack `dotfiles`**:
```
# user ignores
node_modules/
*.log
# >>> grex:dotfiles >>>
.config/nvim
.zshrc
# <<< grex:dotfiles <<<
```

**After additionally syncing pack `tools`**:
```
# user ignores
node_modules/
*.log
# >>> grex:dotfiles >>>
.config/nvim
.zshrc
# <<< grex:dotfiles <<<
# >>> grex:tools >>>
bin/custom-tool
# <<< grex:tools <<<
```

**After `grex rm dotfiles`**: the `grex:dotfiles` block (both markers and body) is deleted; the `grex:tools` block and all user content outside markers are preserved verbatim. Users may edit anything outside the `# >>> grex:<name> >>>` / `# <<< grex:<name> <<<` fences — the writer never reads or rewrites that content.

### Teardown decision table

| Pack-type | Strategy | Fallback | Missing-input behaviour |
|---|---|---|---|
| `meta` | Run each child's teardown in reverse registration order | — | Empty child list = no-op success |
| `declarative` | Run explicit `teardown:` action list if present | Auto-reverse `actions:` (R-M5-09) | No `actions:` and no `teardown:` = no-op success |
| `scripted` | Run `teardown.{sh,ps1}` matched to OS | — | Missing hook file = no-op success |

### Staging

**M5-1 — Trait + 3 built-ins + dispatch swap (PR #1)**
- Land `PackTypePlugin` trait, `PackTypeRegistry`, `register_builtins`.
- Port `meta`, `declarative`, `scripted` behind the trait.
- Swap executor dispatch from enum match to `registry.get(pack.r#type.as_str())`.
- `plugin-inventory` feature extended to pack-types.
- Acceptance: existing end-to-end sync tests for all 3 pack-types pass unchanged; unit tests cover `PackTypeRegistry::bootstrap` returns 3 registered names.

**M5-2 — Gitignore writer + teardown semantics (PR #2)**
- Gitignore managed-block writer (create / replace-in-place / delete-own-block).
- Teardown implementations per R-M5-09, R-M5-10, R-M5-11.
- `grex rm <pack>` wires through to `PackTypePlugin::teardown` + gitignore block deletion.
- Acceptance: cross-platform integration tests cover install→teardown round-trip for each pack-type; gitignore round-trip test asserts user content outside blocks is byte-identical before and after sync/teardown cycles.

### Out of scope for M5

- Parallel execution of `meta` children (sequential-only in v1; future work once a concurrency budget is modeled).
- Cross-pack teardown ordering beyond single-tree reverse-registration (e.g., global topological teardown across independent roots).
- Teardown dry-run (`grex rm --dry-run`) — deferred; current semantics are best-effort and documented as such.
- External dylib / WASM pack-type plugins — v2 along with action-plugin external loading.
- Non-`.grex/hooks/` hook search paths for `scripted` — path is fixed in v1.

