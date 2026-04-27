# plugin-api

Stable trait contracts for v1 extension points. Post-v1 these are semver-protected: breaking changes require a major bump of `grex` itself.

## Three traits

1. `ActionPlugin` — implements one action name (e.g. `symlink`, `env`).
2. `PackTypePlugin` — implements one pack-type (`meta`, `declarative`, `scripted`).
3. `Fetcher` — implements one URL scheme (`git` in v1).

All three are `Send + Sync + 'static` async trait objects via `async_trait`.

Uniform `&str` across plugin traits (2026-04-20) — enables `String`-backed plugins in v2 (WASM/dylib); builtins return literals which coerce to `'static`-lifetime `&str` for zero alloc.

## `ActionPlugin`

```rust
use async_trait::async_trait;
use serde_json::Value;

#[async_trait]
pub trait ActionPlugin: Send + Sync {
    /// Stable action name, matches the YAML key.
    fn name(&self) -> &str;

    /// Execute the action. Args are the raw YAML sub-tree under the action key.
    async fn execute(
        &self,
        ctx: &ExecCtx<'_>,
        args: &Value,
    ) -> Result<ExecStep, ExecError>;
}
```

**M4-B shipped shape (2026-04-20):** the snippet above is the v2-facing target (WASM/dylib plugins consume raw `&Value`). The in-process v1 trait landed **sync** and takes the typed `&Action` instead of `&Value`:

```rust
pub trait ActionPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>)
        -> Result<ExecStep, ExecError>;
}
```

Rationale: the wet-run executor, planner, and scheduler are all synchronous today; the parse step has already validated shape + invariants so taking the typed `&Action` is zero-cost at the boundary. The async + `&Value` form is reserved for external plugin loading (M5+ / v2) where the trait crosses a dylib/WASM ABI boundary. Both shapes return `ExecStep` — that is stable across v1 and v2.

**Return type (v1):** `ExecStep` carries the per-action result envelope — `action_name`, `result` (ok/skipped/failed with diagnostics), `kind`, and related fields. `ActionOutcome` is superseded by `ExecStep` in v1 — richer shape carries diagnostics. Original `ActionOutcome { changed, message }` design retired 2026-04-20.

Rollback is **not** on the trait surface (decision 2026-04-20, matches `openspec/feat-grex/spec.md` §1). Rollback semantics remain where the M3 executor kept them (per-action inverse logic in the executor), not in an `ExecStep` field. A dedicated rollback protocol is deferred to M5+ when pack-type drivers may require it.

### `ExecCtx` (v1 realization of `PackCtx`)

`PackCtx` as originally drafted is v1-realized as `ExecCtx<'a>` in code. Fields present: `vars` (implements `EnvResolver`), `pack_root`, `workspace`, `platform` (typed as `Os` enum). Fields deferred to M5: `pack_id`, `dry_run`, explicit `logger: &dyn ActionLogger` wiring. The `ActionLogger` and `EnvResolver` traits are defined in `grex-core::{log, env}` and available for plugins to use directly; `ExecCtx` field wiring deferred.

```rust
pub struct ExecCtx<'a> {
    pub vars: &'a VarEnv,                // implements EnvResolver
    pub pack_root: &'a std::path::Path,
    pub workspace: &'a std::path::Path,
    pub platform: Os,                    // Windows | Linux | Macos
    // deferred to M5: pack_id, dry_run, logger: &dyn ActionLogger
}
```

## `PackTypePlugin`

> Updated 2026-04-20: M5-1 trait signature aligned with shipped M4 code patterns. The trait mirrors M4 `ActionPlugin` exactly — same `ExecCtx<'_>` context, same `Result<ExecStep, ExecError>` return envelope — so pack-type and action plugins share one result pipeline. The earlier `anyhow::Result<()>` + bare `Pack` draft is retired.

```rust
pub trait PackTypePlugin: Send + Sync {
    fn name(&self) -> &str;

    async fn install(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError>;

    async fn update(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError>;

    async fn teardown(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError>;

    async fn sync(
        &self,
        ctx: &ExecCtx<'_>,
        pack: &PackManifest,
    ) -> Result<ExecStep, ExecError>;
}
```

Ground-truth references (M4 shipped, 2026-04-20):
- M4 `ActionPlugin` trait: `crates/grex-core/src/plugin/mod.rs:49-62` — pattern `PackTypePlugin` reuses.
- `ExecCtx<'a>`: `crates/grex-core/src/execute/ctx.rs:96-146` — reused verbatim.
- `PackManifest`: `crates/grex-core/src/pack/mod.rs:171-197` — canonical name (not `Pack`).
- `ExecStep` / `ExecError`: `crates/grex-core/src/plugin/mod.rs` — same envelope as `ActionPlugin` return.

Async form: uses 2024-edition native async-in-trait; fall back to `#[async_trait]` only if a toolchain blocker surfaces at M5-1 implementation time.

### `PackManifest`

Parsed `.grex/pack.yaml`. Ground-truth struct from `crates/grex-core/src/pack/mod.rs:171-197`:

```rust
pub struct PackManifest {
    pub schema_version: SchemaVersion,   // literal "1"
    pub name: String,
    pub r#type: PackType,                // enum: Meta | Declarative | Scripted | plugin-name
    pub version: Option<String>,
    pub depends_on: Vec<String>,
    pub children: Vec<ChildRef>,
    pub actions: Vec<Action>,
    pub teardown: Option<Vec<Action>>,   // already parsed; R-M5-09 just reads it
    pub extensions: BTreeMap<String, serde_yaml::Value>,
}
```

Dispatch at M5 executor boundary: `registry.get(pack.r#type.as_str())`. The `r#type: PackType` enum stays in the parsed form; the string view is only consumed at registry lookup.

### Lifecycle semantics (required contract)

| Method | Required behavior |
|---|---|
| `install` | Idempotent. Running twice must be equivalent to running once. |
| `update` | Run only when lockfile `sha` or `actions_hash` changed (grex core decides; plugin just does the work when called). |
| `teardown` | Must attempt to reverse `install`. May be partial. |
| `sync` | May recurse into children. May no-op for leaf types. |

## `Fetcher`

```rust
#[async_trait]
pub trait Fetcher: Send + Sync {
    /// URL scheme this fetcher handles: "git".
    fn scheme(&self) -> &str;

    async fn clone(
        &self,
        url: &str,
        ref_spec: Option<&str>,
        dst: &std::path::Path,
    ) -> anyhow::Result<FetchReport>;

    async fn pull(
        &self,
        dst: &std::path::Path,
    ) -> anyhow::Result<FetchReport>;

    async fn current_sha(
        &self,
        dst: &std::path::Path,
    ) -> anyhow::Result<String>;
}

pub struct FetchReport {
    pub sha: Option<String>,
    pub branch: Option<String>,
    pub bytes: Option<u64>,
}
```

v1 ships one implementation (`fetchers::git`, either `gix` or `git2`). v2 may ship `rclone`, `s3`, `oci`, `http` behind the same trait.

## `Registry` struct

Canonical v1 registry holding the action plugins. Packtypes + fetchers retain their existing maps on `Registry`; the signature below covers the action surface added in M4:

```rust
pub struct Registry {
    actions: HashMap<String, Box<dyn ActionPlugin>>,
    // packtypes, fetchers: see existing fields
}
impl Registry {
    pub fn new() -> Self;
    pub fn register<P: ActionPlugin + 'static>(&mut self, plugin: P);
    pub fn get(&self, name: &str) -> Option<&dyn ActionPlugin>;
    pub fn bootstrap() -> Self;  // calls register_builtins internally
}
```

`bootstrap()` is the canonical entrypoint: it constructs an empty `Registry` and hands it to `register_builtins` for the 7 Tier 1 actions. Executor dispatch goes through `Registry::get(name)` (an unknown name yields `UnknownAction`) — the dispatch swap from direct `Action` enum match to `Registry::get` lands in **M4-B** (moved 2026-04-20 from M4-A; see `milestone.md` Stage order note and `openspec/feat-grex/spec.md` §4). In M4-A the `Registry` is shipped as a parallel surface and covered by plugin-layer unit tests while `FsExecutor` / `PlanExecutor` keep the existing enum-match dispatch.

### `register_builtins` free function

```rust
pub fn register_builtins(reg: &mut Registry);
```

Populates `reg` with all 7 Tier 1 plugins (`symlink`, `env`, `mkdir`, `rmdir`, `require`, `when`, `exec`). This is the canonical registration path in v1 — `inventory::submit!` auto-registration is optional (see feature flag below).

**Builtins crate location (2026-04-20):** v1 builtins live in `grex-core::plugin` (co-located for simplicity). `grex-plugins-builtin` is reserved for v2 third-party-facing extensions. Physical move deferred to M5+.

## Idempotency

`ExecResult::Skipped { pack_path: PathBuf, actions_hash: String }` is emitted when the lockfile-stored `actions_hash` for a pack equals the recomputed hash at sync time. Hash scope is canonical JSON of the pack's `actions:` list plus the resolved commit sha (consistent with the "lockfile `actions_hash` field name kept" open-question note; variant reserved in PR #14). On a `Skipped` emission the executor performs no work for that pack and writes no new per-action events for it.

**Hash algorithm (2026-04-20):** `actions_hash = sha256(b"grex-actions-v1\0" || canonical_json(actions) || b"\0" || commit_sha)`, lowercase hex. Computed per-pack; stored in `LockEntry.actions_hash`; compared at sync start; match emits `ExecResult::Skipped` and short-circuits the pack. Implemented in `grex-core::lockfile::hash::compute_actions_hash`.

## Feature flag `plugin-inventory`

Default: **off** in v1. When on, built-in action modules use `inventory::submit!` to auto-register and `Registry::bootstrap()` walks `inventory::iter::<BuiltinAction>()`. When off, `register_builtins` is the only path. Keeping `inventory` optional means `grex-core` carries no hard dependency on it; linker-based collection is a deployment concern per-consumer.

## Registration (v1 in-process)

Canonical path (decision 2026-04-20): explicit `register_builtins(reg: &mut Registry)`. `Registry::bootstrap()` constructs an empty `Registry` and hands it to `register_builtins`, which registers all 7 Tier 1 actions + 3 pack-types + the `git` fetcher. No `inventory` dependency is pulled into `grex-core` on the default path.

```rust
fn register_builtins(reg: &mut Registry) {
    reg.register_action(Box::new(actions::Symlink));
    reg.register_action(Box::new(actions::Env));
    // ... remaining 5 Tier 1 actions
    reg.register_pack_type(Box::new(packtypes::Meta));
    reg.register_pack_type(Box::new(packtypes::Declarative));
    reg.register_pack_type(Box::new(packtypes::Scripted));
    reg.register_fetcher(Box::new(fetchers::Git));
}
```

### Alternative: `inventory::submit!` (feature-gated, M4-E)

Opt-in compile-time auto-registration via the `inventory` crate, gated behind the `plugin-inventory` cargo feature (default **off**; see "Feature flag `plugin-inventory`" above). Lands in Stage M4-E as a discovery hook; not on the critical path for v1 and not required by any other stage.

```rust
pub struct BuiltinAction(pub fn() -> Box<dyn ActionPlugin>);
inventory::collect!(BuiltinAction);

pub struct BuiltinPackType(pub fn() -> Box<dyn PackTypePlugin>);
inventory::collect!(BuiltinPackType);

pub struct BuiltinFetcher(pub fn() -> Box<dyn Fetcher>);
inventory::collect!(BuiltinFetcher);
```

Each built-in module would then call `inventory::submit!` at file scope:

```rust
// src/actions/symlink.rs
pub struct Symlink;

#[async_trait::async_trait]
impl ActionPlugin for Symlink { /* ... */ }

inventory::submit! {
    crate::plugin::BuiltinAction(|| Box::new(Symlink))
}
```

When the feature is on, `Registry::bootstrap()` walks `inventory::iter::<BuiltinAction>()` (and the pack-type / fetcher collectors) instead of calling `register_builtins` directly. When the feature is off (default), `register_builtins` is the only path.

## Adding a new built-in plugin in v1

The flow for a v1 contributor wanting to add, say, a `pkg-install` action:

1. Create `src/actions/pkg_install.rs` implementing `ActionPlugin`.
2. `pub mod pkg_install;` in `src/actions/mod.rs`.
3. Add `inventory::submit!` block (or explicit register call).
4. Integration test under `tests/actions_pkg_install.rs`.
5. Docs entry in [actions.md](./actions.md).

No changes to trait crate; no ABI concerns. Core grex recompile required, but plugin author writes no glue code beyond the trait impl.

## v2 external plugin loading

Deferred. Two candidate routes:

### Option A: dylib via `libloading` + `abi_stable`
- Host loads `libgrex_plugin_foo.{so,dylib,dll}`.
- Plugin crate uses `abi_stable` for FFI-safe trait objects.
- Pros: native speed, same language.
- Cons: ABI versioning is strict; every trait tweak risks SIGSEGV on version skew.

### Option B: WASM via `wasmtime` / `extism`
- Host loads `foo.wasm`.
- Plugin compiled to wasm32-wasi.
- Pros: sandboxed, cross-platform binary, forward-compatible ABI.
- Cons: syscall surface must be bridged; filesystem access needs capability grants.

Decision in v2 alpha. ABI contract versioning strategy:

- `grex-plugin-api` crate (extracted in v2) carries its own semver.
- Plugin manifest declares `grex_plugin_api = "1.x"`.
- Host refuses load on major mismatch, warns on minor mismatch.
- Candidate extension: ABI hash baked into plugin binary, checked at load.

## Stability guarantees (v1)

Post-v1.0.0 the following are **frozen** until a v2.0.0:

- `ActionPlugin` method signatures.
- `PackTypePlugin` method signatures.
- `Fetcher` method signatures.
- `ExecCtx` field names & types (fields may be added; none removed or retyped).
- `ExecStep`, `FetchReport` struct layouts (additive).
- `PackManifest` struct (additive).
- Registration mechanism.

Breaking changes require a `grex` major bump; v2 re-extracts the plugin traits into a separately-versioned crate so host and plugin can move independently.
