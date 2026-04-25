# architecture

Crate layout, trait surfaces, and data-flow for `grex` v1.

## Workspace

Single crate `grex` (lib + bin). Sub-crates avoided in v1 to keep the plugin trait crate vendored in the same compilation unit. v2 may split `grex-plugin-api` into its own crate for ABI stability.

```
grex/
├── Cargo.toml
├── rust-toolchain.toml
├── src/
│   ├── main.rs                # thin bin entrypoint
│   ├── lib.rs                 # public surface re-exports
│   ├── cli/
│   │   ├── mod.rs             # clap::Command composition
│   │   ├── init.rs            # grex init
│   │   ├── add.rs             # grex add
│   │   ├── rm.rs              # grex rm
│   │   ├── ls.rs              # grex ls
│   │   ├── status.rs          # grex status
│   │   ├── sync.rs            # grex sync
│   │   ├── update.rs          # grex update
│   │   ├── doctor.rs          # grex doctor
│   │   ├── serve.rs           # grex serve --mcp
│   │   ├── import.rs          # grex import
│   │   ├── run.rs             # grex run <action>
│   │   ├── exec.rs            # grex exec <cmd>
│   │   └── output.rs          # all print! / table / color
│   ├── manifest/
│   │   ├── mod.rs
│   │   ├── event.rs           # intent events
│   │   ├── state.rs           # folded pack state
│   │   ├── fold.rs            # event stream → HashMap<Id, State>
│   │   ├── lock.rs            # grex.lock.jsonl
│   │   ├── io.rs              # atomic temp+rename, fd-lock
│   │   └── compact.rs
│   ├── pack/
│   │   ├── mod.rs             # Pack struct, tree walk
│   │   ├── schema.rs          # pack.yaml schema v1
│   │   └── discovery.rs       # load/resolve children
│   ├── plugin/
│   │   ├── mod.rs             # registries, trait re-exports, v1 co-located builtins
│   │   ├── action.rs          # ActionPlugin trait
│   │   ├── packtype.rs        # PackTypePlugin trait
│   │   └── fetcher.rs         # Fetcher trait (git backend)
│   ├── log.rs                 # ActionLogger trait (plugin diagnostics)
│   ├── env.rs                 # EnvResolver trait ($VAR expansion surface)
│   ├── lockfile/
│   │   └── hash.rs            # compute_actions_hash (sha256 over canonical actions+sha)
│   ├── actions/               # 7 built-in action plugins
│   │   ├── symlink.rs
│   │   ├── env.rs
│   │   ├── mkdir.rs
│   │   ├── rmdir.rs
│   │   ├── require.rs
│   │   ├── when.rs
│   │   └── exec.rs
│   ├── packtypes/             # 3 built-in pack-type plugins
│   │   ├── meta.rs
│   │   ├── declarative.rs
│   │   └── scripted.rs
│   ├── fetchers/
│   │   └── git.rs             # gix or git2 behind Fetcher trait
│   ├── gitignore/
│   │   └── mod.rs             # managed-block read/write
│   ├── mcp/
│   │   ├── mod.rs             # stdio JSON-RPC 2.0 loop
│   │   ├── methods.rs         # verb → method dispatch
│   │   └── schema.rs
│   └── concurrency/
│       ├── mod.rs             # tokio runtime bootstrap
│       ├── scheduler.rs       # semaphore + per-pack lock
│       └── packlock.rs        # <path>/.grex-lock
├── tests/
│   ├── integration_add.rs
│   ├── integration_rm.rs
│   ├── sync_recursive.rs
│   ├── sync_parallel.rs
│   ├── gitignore_preserves_user_lines.rs
│   ├── crash_recovery.rs
│   ├── mcp_stdio.rs
│   ├── import_legacy.rs
│   ├── doctor_drift.rs
│   ├── pack_types_end_to_end.rs
│   └── property_manifest.rs
├── lean/
│   ├── lakefile.lean
│   └── Grex/
│       └── Scheduler.lean
└── .github/workflows/
    ├── ci.yml
    ├── lean.yml
    └── release.yml
```

## Core trait sketches

Full contracts in [plugin-api.md](./plugin-api.md). Condensed here:

```rust
use async_trait::async_trait;
use serde_json::Value;
use std::path::Path;

pub enum Os { Windows, Linux, Macos }

// v1: PackCtx is realized as ExecCtx in code (2026-04-20).
pub struct ExecCtx<'a> {
    pub vars: &'a VarEnv,                // implements EnvResolver
    pub pack_root: &'a Path,
    pub workspace: &'a Path,
    pub platform: Os,                    // type-safe; decision 2026-04-20
    // deferred to M5: pack_id, dry_run, logger: &dyn ActionLogger
}

// v1 shipped shape (2026-04-20 — aligned with shipped trait in M4-B review fix).
// Sync fn, typed &Action (not &Value), returns ExecStep. Async + &Value form is
// the v2-facing target reserved for external plugin loading (dylib/WASM).
pub trait ActionPlugin: Send + Sync {
    fn name(&self) -> &str;
    fn execute(&self, action: &Action, ctx: &ExecCtx<'_>) -> Result<ExecStep, ExecError>;
}

#[async_trait]
pub trait PackTypePlugin: Send + Sync {
    fn name(&self) -> &str;
    async fn install(&self, ctx: &ExecCtx<'_>, pack: &Pack) -> anyhow::Result<()>;
    async fn update(&self, ctx: &ExecCtx<'_>, pack: &Pack)  -> anyhow::Result<()>;
    async fn teardown(&self, ctx: &ExecCtx<'_>, pack: &Pack) -> anyhow::Result<()>;
    async fn sync(&self, ctx: &ExecCtx<'_>, pack: &Pack)    -> anyhow::Result<()>;
}

pub struct FetchReport {
    pub sha: Option<String>,
    pub branch: Option<String>,
}

#[async_trait]
pub trait Fetcher: Send + Sync {
    fn scheme(&self) -> &str;            // "git"
    async fn clone(&self, url: &str, dst: &Path) -> anyhow::Result<FetchReport>;
    async fn pull(&self, dst: &Path)              -> anyhow::Result<FetchReport>;
}
```

## Verb → module map

| CLI verb | Entry module | Primary collaborators |
|---|---|---|
| `init` | `cli::init` | `manifest::io`, `gitignore`, `concurrency` |
| `add` | `cli::add` | `manifest`, `pack::discovery`, `plugin::packtype`, `fetchers::git`, `gitignore` |
| `rm` | `cli::rm` | `manifest` (tombstone), `plugin::packtype::teardown`, `gitignore` |
| `ls` | `cli::ls` | `manifest::fold`, `manifest::lock` |
| `status` | `cli::status` | `manifest`, per-pack-type `status` dispatch |
| `sync` | `cli::sync` | `fetchers::git`, `concurrency::scheduler`, recursion |
| `update` | `cli::update` | `sync` + `pack-type.install` if lockfile delta |
| `doctor` | `cli::doctor` | `manifest` integrity, `gitignore` diff, schema validate |
| `serve` | `cli::serve` | `mcp::*` |
| `import` | `cli::import` | legacy `REPOS.json` ingest → `manifest::event::Add` |
| `run` | `cli::run` | `plugin::action`, `cli::output` |
| `exec` | `cli::exec` | `tokio::process`, `concurrency::scheduler` |

## Data flow (ASCII)

```
        ┌──────────────┐
argv ──►│  clap parse  │
        └──────┬───────┘
               │ verb + args
               ▼
        ┌──────────────┐     ┌────────────────────┐
        │  dispatcher  │────►│ manifest::load     │
        └──────┬───────┘     │  fold events       │
               │             └────────┬───────────┘
               │                      │ HashMap<PackId, State>
               ▼                      │
        ┌──────────────┐              │
        │ pack::walk   │◄─────────────┘
        │ (load .grex/ │
        │  pack.yaml,  │
        │  recurse     │
        │  children)   │
        └──────┬───────┘
               │ PackTree
               ▼
        ┌──────────────┐
        │ concurrency  │  tokio runtime
        │  scheduler   │  semaphore(N)
        └──────┬───────┘  per-pack .grex-lock
               │
   ┌───────────┼───────────┐
   ▼           ▼           ▼
 fetcher   packtype    action
 (git       plugin    plugin
  pull)     dispatch  exec
               │
               ▼
        ┌──────────────┐
        │ manifest::   │  atomic temp+rename
        │  append      │  fd-lock RW
        └──────┬───────┘
               │
               ▼
        ┌──────────────┐
        │ lockfile     │  resolved state
        │  update      │
        └──────┬───────┘
               │
               ▼
        ┌──────────────┐
        │ gitignore    │  managed-block sync
        │  sync        │
        └──────┬───────┘
               │
               ▼
        ┌──────────────┐
        │ cli::output  │  pretty | plain | json
        └──────────────┘
```

`pack::walk` traverses **two distinct edges** in the pack graph:

- `children` edge — ownership. The walker clones missing children, recurses into them, and applies their lifecycle transitively.
- `depends_on` edge — verification only. The walker checks each named/URL'd prerequisite resolves to a present, satisfied pack in the workspace; it does NOT clone or recurse. Unresolved `depends_on` entries are a hard error at plan phase, before the scheduler dispatches any action. See [pack-spec.md §`children` vs `depends_on`](./pack-spec.md#children-vs-depends_on--ownership-split).

## Runtime invariants

- **I1** (Lean4 v1 proof): scheduler never holds two concurrent locks on the same pack path.
- **I2**: every manifest append is preceded by acquiring the global fd-lock.
- **I3**: `.gitignore` managed-block sync is idempotent — running it twice is a no-op on disk.
- **I4**: compaction output is fold-equivalent to its input.
- **I5**: pack tree walk terminates (cycle detection).

See [concurrency.md](./concurrency.md) for I1's Lean4 formalization.
