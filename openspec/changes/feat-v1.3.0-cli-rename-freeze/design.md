---
slug: feat-v1.3.0-cli-rename-freeze-design
type: design
status: active
last_updated: 2026-05-02
---

# feat-v1.3.0 — design

**Status**: active
**Spec**: [`proposal.md`](./proposal.md) · [`tasks.md`](./tasks.md)
**SSOT**: `.omne/cfg/cli.md` (CLI surface — target of doc-noun rewrite + flag alias) · `.omne/cfg/api-contract.md` (behavior contract surface — gets freeze table cross-link) · `.omne/cfg/mcp.md` (MCP wire — gets SyncParams `pack` field doc) · `.omne/cfg/walker.md` (ExecCtx field doc) · `.omne/cfg/plugin-api.md` (UNSTABLE callout target) · `.omne/cfg/freeze-v1.3.0.md` (NEW — 13-row freeze table) · `.omne/cfg/migration-v1.3.0.md` (NEW — operator + Rust consumer migration guide)

## Why

v1.2.x stabilization closed the headline correctness + concurrency + filesystem-hardening + error-taxonomy gaps. v1.3.0 = the long-planned MINOR cut that converts the `pack` noun (already canonical internally) into the operator-facing surface, AND freezes 13 STABLE behavior contracts so downstream Rust consumers + plugin authors + operators have a stable target.

Bundling rename + freeze in one MINOR is preferable to two cuts:

- The freeze table documents what the rename produces (CLI flag set on 4 verbs, JSON envelope keys, MCP SyncParams fields). Doing them apart means the freeze table publishes against a moving target.
- v1.4.0 needs runway for deprecated-symbol removal (`PackLock::acquire` sync, `Scheduler::permits`, `DEFAULT_MANAGED_GITIGNORE_PATTERNS`) AND Plugin-API freeze. Combining rename+freeze in v1.3.0 keeps v1.4.0's scope clean.

Each scope item is small, additive, and independent in file scope; they partition cleanly across parallel workers without write-set conflict.

## Architectural context

**CLI surface** lives in `crates/grex/src/`:
- `args.rs` — workspace-shared clap helper macros (currently empty for `--workspace`; add the `--pack` alias macro here).
- `verbs/sync.rs` — `SyncArgs::workspace: Option<PathBuf>` (line ~30, alias target).
- `verbs/serve.rs` — `ServeArgs::workspace: Option<PathBuf>` (line ~25, alias target).
- `verbs/migrate_lockfile.rs` — `MigrateLockfileArgs::workspace: Option<PathBuf>` (line ~20, alias target).
- `verbs/teardown.rs` — `TeardownArgs::workspace: Option<PathBuf>` (line ~22, alias target).

clap's `#[arg(long, alias = "workspace")]` mechanism (clap 4.5) lets `--pack` become canonical while `--workspace` continues to parse to the same field. `visible_alias` would surface both in `--help`; `alias` (hidden) preserves help-text cleanliness AND emits a deprecation diagnostic on use.

**JSON envelope surface** in `crates/grex/src/verbs/ls.rs` and `crates/grex/src/verbs/doctor.rs`:
- `ls.rs` envelope: `{ "workspace": "<path>", "metas": [...] }` (line ~150).
- `doctor.rs` envelope: `{ "workspace": "<path>", "report": {...} }` (line ~200).

Dual-emit means both envelopes get a sibling `"pack": "<path>"` key with the SAME value. Order: `workspace` first, `pack` second — preserves byte-stability for diff-friendly consumers reading `workspace` and lets new consumers read `pack` without breaking.

**MCP wire surface** in `crates/grex-mcp/src/sync.rs`:
- `SyncParams { workspace: Option<PathBuf>, ... }` (line ~30).

Add `pack: Option<PathBuf>` as additive sibling field. Resolution: `params.pack.or(params.workspace)` — if both present, `pack` wins (caller responsibility to not send both); if only `workspace` present, value used (back-compat); if neither, current default.

**ExecCtx surface** in `crates/grex-core/src/exec.rs` (or wherever `ExecCtx` lives):
- `ExecCtx { workspace: PathBuf, ... }` — internal to grex-core; consumed by walker.

v1.3.0 ADDS `pack: PathBuf` as additive sibling field initialized identically to `workspace`. Both coexist; v2 removes `workspace`. Plugin authors reading `ExecCtx::pack` get future-proof code; existing readers of `ExecCtx::workspace` keep working.

**Plugin-API surface** in `crates/grex-plugins-builtin/src/plugin/mod.rs`:
- Crate-level rustdoc comment.
- `Cargo.toml` `description` field.
- `.omne/cfg/plugin-api.md` (SSOT, separate repo).

v1.3.0 adds an UNSTABLE warning to all three; no API change. Signals to downstream that Plugin-API contract is NOT frozen in v1.3.0 (v1.4.0 freeze candidate).

## Algorithm sketches

### Clap alias mechanism

```rust
// crates/grex/src/verbs/sync.rs (BEFORE v1.3.0)
#[derive(clap::Args, Debug, Clone)]
pub struct SyncArgs {
    /// Path to the workspace root (defaults to CWD if omitted)
    #[arg(long)]
    pub workspace: Option<PathBuf>,
    // ...
}

// AFTER v1.3.0
#[derive(clap::Args, Debug, Clone)]
pub struct SyncArgs {
    /// Path to the pack root (defaults to CWD if omitted).
    /// `--workspace` is supported as a deprecated alias.
    #[arg(long = "pack", alias = "workspace")]
    pub workspace: Option<PathBuf>,  // field name unchanged (additive `pack` accessor optional; we keep field as-is to minimize blast radius)
    // ...
}
```

Field name stays `workspace` to keep blast radius minimal — only the FLAG name changes. Internal Rust code that reads `args.workspace` continues to work unchanged.

Deprecation warn-once: requires a `OnceLock<()>` per process or a thread-local check at args resolution (`crates/grex/src/main.rs` after `Cli::parse()`). On detection that the original CLI input contained `--workspace`, emit `eprintln!("warning: --workspace is deprecated; use --pack instead")` exactly once. Detection requires inspecting `std::env::args()` because clap collapses both flags to the same field. Helper:

```rust
// crates/grex/src/deprecation.rs (NEW)
use std::sync::OnceLock;

static WORKSPACE_DEPRECATION_WARNED: OnceLock<()> = OnceLock::new();

pub fn warn_workspace_deprecation_if_used() {
    if std::env::args().any(|a| a == "--workspace" || a.starts_with("--workspace=")) {
        WORKSPACE_DEPRECATION_WARNED.get_or_init(|| {
            eprintln!("warning: --workspace is deprecated; use --pack instead. \
                       The alias will be removed in a future major release.");
        });
    }
}
```

Called once at the top of `main()` after `Cli::parse()`.

### JSON dual-emit

```rust
// crates/grex/src/verbs/ls.rs (BEFORE v1.3.0)
#[derive(serde::Serialize)]
struct LsEnvelope {
    workspace: PathBuf,
    metas: Vec<MetaSummary>,
}

// AFTER v1.3.0
#[derive(serde::Serialize)]
struct LsEnvelope {
    workspace: PathBuf,
    pack: PathBuf,  // dual-emit; identical value to workspace
    metas: Vec<MetaSummary>,
}

// Construction:
let workspace = resolve_workspace_root(&args)?;
let envelope = LsEnvelope {
    workspace: workspace.clone(),
    pack: workspace.clone(),
    metas: collected,
};
```

Order matters for byte-stable JSON output: `workspace` first because existing diff-tooling expects it. Same pattern for `doctor.rs` envelope.

### MCP precedence

```rust
// crates/grex-mcp/src/sync.rs (BEFORE v1.3.0)
#[derive(serde::Deserialize)]
pub struct SyncParams {
    pub workspace: Option<PathBuf>,
    // ...
}

// AFTER v1.3.0
#[derive(serde::Deserialize)]
pub struct SyncParams {
    pub workspace: Option<PathBuf>,
    pub pack: Option<PathBuf>,  // additive sibling field
    // ...
}

impl SyncParams {
    /// Resolve the pack root with `pack` taking precedence over `workspace`.
    /// Equivalent to `pack.or(workspace)`.
    pub fn resolved_pack_root(&self) -> Option<&PathBuf> {
        self.pack.as_ref().or(self.workspace.as_ref())
    }
}
```

Precedence rule: `pack.or(workspace)`. If both present, `pack` wins. If only `workspace` present (back-compat callers), value used. If neither, downstream default applies.

### ExecCtx additive `pack` field

```rust
// crates/grex-core/src/exec.rs (or current location)
pub struct ExecCtx {
    pub workspace: PathBuf,
    pub pack: PathBuf,  // additive sibling — initialized identical to workspace at construction
    // ...
}

impl ExecCtx {
    pub fn new(root: PathBuf, /* ... */) -> Self {
        Self {
            workspace: root.clone(),
            pack: root,  // identical value
            // ...
        }
    }
}
```

Both fields coexist. Plugin authors reading `ExecCtx::pack` get future-proof code; existing readers of `ExecCtx::workspace` keep working. v2 removes `workspace`.

### Edge cases

- **Both `--pack` and `--workspace` on same invocation.** clap collapses them to the same field; the LAST one wins per clap's standard semantics. Document in deprecation warning that mixing is undefined-but-deterministic.
- **Both `pack` and `workspace` in MCP SyncParams JSON.** `pack` wins per `or` precedence; document in MCP doc.
- **JSON envelope consumers using strict schemas.** Adding a new key (`pack`) to an envelope can break consumers using schema validation with `additionalProperties: false`. Surface this as a known migration in `.omne/cfg/migration-v1.3.0.md` operator section. Mitigation: existing schemas should already use `additionalProperties: true` for forward-compat; if not, the consumer needs a one-line schema update.
- **Manpage regen drift.** `cargo xtask gen-man` rewrites all 4 affected manpages with `<pack>` doc-noun. Verify diff is rename-only (no flag removal).

### Idempotence

- Clap parsing is pure: same input args produces same `SyncArgs` (alias collapse is deterministic).
- JSON dual-emit is pure: same input envelope state produces same JSON bytes.
- MCP precedence is pure: same `(pack, workspace)` Option pair produces same resolved path.
- Deprecation warn-once is process-scoped: the `OnceLock` ensures exactly one diagnostic per process lifetime.

## Lean obligation: NONE (Rule 8 simple exemption)

Per `.omne/schemas/rules.md` Rule 8: non-simple algorithms (concurrent, multi-phase invariants, scheduler/locking primitives) MUST have a Lean4 proof BEFORE production code. v1.3.0 has ZERO such items.

Per-item exemption justification:

- **CLI rename via clap alias** — pure surface rename. clap's alias mechanism is documented + tested upstream. No new algorithm; same clap parser.
- **Doc-noun rename** — doc text only; zero code path change.
- **JSON dual-emit** — pure additive field on a serde-derived struct. Equivalent to `workspace == pack` invariant at construction time; trivially satisfied by `let pack = workspace.clone();`.
- **MCP `pack` param + precedence** — `Option::or` is a stdlib function with documented semantics. Pure routing.
- **Behavior contract freeze** — documentation only. No code behavior change.
- **Deprecation deferrals** — documentation; symbols remain in place with `#[deprecated]` attribute (no removal until v1.4.0).
- **NEW SSOT files** — net-add documentation; no code touched.
- **Plugin-API UNSTABLE marker** — doc + Cargo.toml metadata; no code behavior change.
- **e2e smoke extension** — extends an existing test with one assertion; no new algorithmic obligation (the test exercises the existing walker which is already proven via `sync_meta_no_cycle_infinite_clone` + `cancellation_terminates_promptly`).
- **MSRV unchanged** — no language feature dependency added.

Conclusion: zero new algorithmic behavior. Rule 8 simple exemption applies to all 10 scope items. Document the exemption explicitly in `tasks.md` Stage 1.

### Bridge axiom budget

**Target: 0 new bridge axioms. Current count target: 9 unchanged.**

(Current v1.2.6 budget: Bridge ≤ 12 with cap-std axiom optionally added. v1.3.0 zero algorithmic change → zero bridge delta. Hard ceiling: unchanged from v1.2.6 outcome — no new ceiling required.)

Verify post-impl that `#print axioms` for the 5 headline theorems shows the same axiom set as v1.2.6.

## Migration tables

### Table M1: CLI flag migration (operator-facing)

| Before (v1.2.x) | After (v1.3.0) | Notes |
|---|---|---|
| `grex sync --workspace <p>` | `grex sync --pack <p>` | `--workspace` still works; emits warn-once deprecation |
| `grex serve --workspace <p>` | `grex serve --pack <p>` | same |
| `grex migrate-lockfile --workspace <p>` | `grex migrate-lockfile --pack <p>` | same |
| `grex teardown --workspace <p>` | `grex teardown --pack <p>` | same |
| `--help` mentions `<workspace>` | `--help` mentions `<pack>` | doc-noun update only |

### Table M2: JSON envelope migration (consumer-facing)

| Surface | Before (v1.2.x) | After (v1.3.0) | Notes |
|---|---|---|---|
| `grex ls --json` | `{"workspace": "<p>", "metas": [...]}` | `{"workspace": "<p>", "pack": "<p>", "metas": [...]}` | `pack` key added; identical value |
| `grex doctor --json` | `{"workspace": "<p>", "report": {...}}` | `{"workspace": "<p>", "pack": "<p>", "report": {...}}` | same |

Strict-schema consumers (`additionalProperties: false`) need a one-line schema update to add `pack`. Document in `.omne/cfg/migration-v1.3.0.md`.

### Table M3: Rust API migration (downstream consumer-facing)

| Surface | Before (v1.2.x) | After (v1.3.0) | Notes |
|---|---|---|---|
| `ExecCtx::workspace` | `pub workspace: PathBuf` | `pub workspace: PathBuf` (unchanged) | preserved |
| `ExecCtx::pack` | (does not exist) | `pub pack: PathBuf` | additive sibling; identical value to `workspace` at construction |
| `SyncParams::workspace` (MCP) | `pub workspace: Option<PathBuf>` | `pub workspace: Option<PathBuf>` (unchanged) | preserved |
| `SyncParams::pack` (MCP) | (does not exist) | `pub pack: Option<PathBuf>` | additive sibling; precedence `pack.or(workspace)` |

### Table F1: Behavior contract freeze table (full 13 rows)

| # | Contract | Surface | SemVer-class | Owner |
|---|---|---|---|---|
| C1 | `pack.yaml` schema | file-format | additive-only post-freeze (new keys allowed; existing keys MUST not change semantics) | `cfg/pack-spec.md` |
| C2 | `grex.lock.jsonl` schema | file-format | additive-only (new fields allowed; existing fields frozen) | `cfg/manifest.md` (lockfile section) |
| C3 | `events.jsonl` event variants | file-format | additive-only (new event types allowed under existing JSONL pattern) | `cfg/manifest.md` (events catalog) |
| C4 | `TreeError` variant set | API | additive-only (`#[non_exhaustive]` since v1.2.0; new variants OK) | `crates/grex-core/src/tree/error.rs` |
| C5 | `--workspace` / `--pack` CLI flag set on 4 verbs | CLI | additive-only (alias mechanism preserves both; new flags OK) | `cfg/cli.md` |
| C6 | JSON envelope keys (ls/doctor) | wire | additive-only (`workspace` + `pack` dual-emit; new keys OK) | `cfg/cli.md` |
| C7 | MCP `SyncParams` field set | wire | additive-only (new fields OK; existing fields frozen) | `cfg/mcp.md` |
| C8 | `ExecCtx::workspace` field | API | additive-only (`pack` shadow allowed; rename gated to v2) | `cfg/walker.md` |
| C9 | ChildPath validation rules | behavior | no semantic change without MAJOR | `cfg/walker.md` (ChildPath section) |
| C10 | lockfile sentinel files (`.grex/`, `pack-id.txt`, `head.txt`) | file-format | no removal without MAJOR | `cfg/manifest.md` |
| C11 | `--retain-days` semantics (`grex doctor`) | behavior | no semantic change without MAJOR | `cfg/quarantine.md` |
| C12 | cap-std root capability semantics on walker FS surface | behavior | cap-std runtime contract = STABLE | `cfg/walker.md` (TOCTOU section) + `cfg/toctou.md` |
| C13 | Quarantine GC + restore semantics (v1.2.5) | behavior | no semantic change without MAJOR | `cfg/quarantine.md` |
| (excluded) | Plugin-API surface | API | UNSTABLE — v1.4.0 freeze candidate | `cfg/plugin-api.md` (gets WARNING callout) |

## Files touched

**Rust (production):**

- `crates/grex/src/verbs/sync.rs` — `SyncArgs`: add `#[arg(long = "pack", alias = "workspace")]`; rewrite doc-comment.
- `crates/grex/src/verbs/serve.rs` — same pattern on `ServeArgs`.
- `crates/grex/src/verbs/migrate_lockfile.rs` — same pattern on `MigrateLockfileArgs`.
- `crates/grex/src/verbs/teardown.rs` — same pattern on `TeardownArgs`.
- `crates/grex/src/verbs/ls.rs` — `LsEnvelope`: add `pack` field; construction sets `pack: workspace.clone()`.
- `crates/grex/src/verbs/doctor.rs` — `DoctorEnvelope`: add `pack` field; same pattern.
- `crates/grex/src/deprecation.rs` (NEW) — `warn_workspace_deprecation_if_used()` helper with `OnceLock`.
- `crates/grex/src/main.rs` — call `warn_workspace_deprecation_if_used()` after `Cli::parse()`.
- `crates/grex-mcp/src/sync.rs` — `SyncParams`: add `pack: Option<PathBuf>`; add `resolved_pack_root()` helper.
- `crates/grex-core/src/exec.rs` — `ExecCtx`: add `pack: PathBuf` field; constructor sets `pack: workspace.clone()`.
- `crates/grex-plugins-builtin/src/plugin/mod.rs` — add UNSTABLE warning to crate-level rustdoc.
- `crates/grex-plugins-builtin/Cargo.toml` — append "(UNSTABLE — Plugin-API frozen in v1.4.0)" to `description`.

**Rust (tests):**

- `crates/grex/tests/cli_alias.rs` (NEW) — `cli_workspace_pack_alias_parses_both` for all 4 verbs; `cli_workspace_deprecation_warns_once_per_process`.
- `crates/grex/tests/cli_json.rs` (NEW or extend existing) — `cli_ls_doctor_json_envelopes_dual_emit_workspace_pack`.
- `crates/grex-mcp/tests/sync_pack.rs` (NEW) — `mcp_sync_pack_or_workspace_precedence` for all 4 precedence cases.
- `crates/grex/tests/sync_e2e.rs` — extend `e2e_v1_3_0_readiness_smoke` with the deprecation warn-once assertion.

**Doc / SSOT (separate repo per Rule 7):**

- `.omne/cfg/freeze-v1.3.0.md` (NEW) — 4-column freeze table per Table F1 above; G2 frontmatter (`type: design`, `status: active`); cross-link from `cfg/api-contract.md`.
- `.omne/cfg/migration-v1.3.0.md` (NEW) — operator section (Tables M1, M2) + Rust consumer section (Table M3); G2 frontmatter (`type: migration`, `status: active`).
- `.omne/cfg/cli.md` — update doc-noun in flag descriptions; cross-link to migration doc.
- `.omne/cfg/api-contract.md` — cross-link to freeze table.
- `.omne/cfg/mcp.md` — document `SyncParams::pack` field + precedence rule.
- `.omne/cfg/walker.md` — document `ExecCtx::pack` additive field.
- `.omne/cfg/plugin-api.md` — add UNSTABLE WARNING callout at top of doc.
- `.omne/INDEX.yaml` — auto-regenerated by `scripts/build_index.py` (Rule 12).

**Versioning:**

- `Cargo.toml` (workspace root): `version = "1.2.6"` → `"1.3.0"`.
- `crates/xtask/Cargo.toml`: `grex-cli = { ... version = "1.2.6" }` → `"1.3.0"`.
- `crates/xtask/tests/version_test.rs`: `EXPECTED_WORKSPACE_VERSION` → `"1.3.0"`.
- 3 internal path-deps (grex-core, grex-mcp, grex-plugins-builtin) bumped to 1.3.0.

**Manpages:**

- `cargo xtask gen-man` to regenerate. Expected: 4 manpages get `<workspace>` → `<pack>` doc-noun rewrite + new `--pack` flag listing (with `--workspace` in the alias section).

**Changelog/history:**

- `CHANGELOG.md` — append `[1.3.0] - 2026-05-XX` MILESTONE entry.
- `.omne/cfg/history.md` — append v1.3.0 MILESTONE entry (separate SSOT repo per Rule 7).

## Acceptance criteria

1. `cd proof && lake build` exits 0; zero `sorry`, zero `admit`. Axiom counts unchanged from v1.2.6 (Bridge ≤ 12, Types = 4, Other = 0).
2. `#print axioms` for the 5 headline theorems matches the v1.2.6 baseline (no new axioms).
3. `cargo build --workspace`, `cargo test --workspace`, `cargo fmt --all -- --check`, `cargo doc --no-deps --workspace -D warnings`, `cargo clippy --workspace --all-targets -- -D warnings` all exit 0. (`dispatch_parallel.rs` integration test continues to be excluded per pre-existing Windows UAC os error 740.)
4. New tests pass: `cli_workspace_pack_alias_parses_both`, `cli_workspace_deprecation_warns_once_per_process`, `cli_ls_doctor_json_envelopes_dual_emit_workspace_pack`, `mcp_sync_pack_or_workspace_precedence`. Plus the 380+ existing lib tests, the v1.2.4 cancellation test, the v1.2.3 e2e cycle test, the v1.2.5 quarantine GC/restore tests, the v1.2.6 cap-std hardening + TreeError split tests, AND the extended `e2e_v1_3_0_readiness_smoke` (with warn-once assertion) all continue to pass.
5. SemVer label: MINOR (1.2.6 → 1.3.0). Per Rule 6 maintainer has the call; technical reasoning supports MINOR because all changes are additive at API/wire/file-format/behavior layers.
6. Plugin-API UNSTABLE marker visible: `cargo doc --no-deps -p grex-plugins-builtin` shows the WARNING callout in the lib-level docs; `cargo metadata` shows the description suffix.
7. Manpage diff after regen: 4 manpages affected (sync, serve, migrate-lockfile, teardown); each shows `<workspace>` → `<pack>` doc-noun rewrite + `--pack` listed as canonical flag with `--workspace` in alias position.
8. SSOT files committed in separate repo: `.omne/cfg/freeze-v1.3.0.md` + `.omne/cfg/migration-v1.3.0.md` (NEW) + 5 existing doc updates per Round 3 gap list. INDEX.yaml regenerated by aggregator.

## Migration note for changelog

```
## [1.3.0] — 2026-05-XX

### Added

- New CLI flag `--pack <path>` on `grex sync`, `grex serve`,
  `grex migrate-lockfile`, `grex teardown`. The previous `--workspace`
  flag is preserved as a deprecated alias and emits a one-time
  warn-on-use diagnostic per process. Both flags resolve to the same
  internal field; mixing them on a single invocation is supported but
  not recommended.
- New JSON envelope key `"pack"` on `grex ls --json` and
  `grex doctor --json` responses. The existing `"workspace"` key is
  preserved with the identical value (dual-emit). Strict-schema
  consumers may need to add `"pack"` to their schema.
- New MCP `SyncParams` field `pack: Option<PathBuf>` alongside existing
  `workspace`. Precedence: `pack.or(workspace)` — `pack` wins if both
  present.
- New additive field `ExecCtx::pack: PathBuf` alongside existing
  `workspace`. Both fields are initialized identically at construction.
  Plugin authors should prefer `pack` for future-proofing; `workspace`
  removal is planned for v2.
- New SSOT documents: `.omne/cfg/freeze-v1.3.0.md` (13-row behavior
  contract freeze table) + `.omne/cfg/migration-v1.3.0.md` (operator +
  Rust consumer migration guide). Both ship through the SSOT repo.

### Changed

- CLI doc-noun in `--help` output and manpages updated from
  `<workspace>` to `<pack>` across all 4 affected verbs. Internal field
  names unchanged for blast-radius minimization.

### Deprecated

- `--workspace` CLI flag (replaced by `--pack`; warn-once).
- `PackLock::acquire` synchronous variant (removal deferred to v1.4.0).
- `Scheduler::permits()` (removal deferred to v1.4.0).
- `DEFAULT_MANAGED_GITIGNORE_PATTERNS` const (removal deferred to v1.4.0).

### Frozen (behavior contracts)

- 13 STABLE behavior contracts frozen per `.omne/cfg/freeze-v1.3.0.md`:
  pack.yaml schema, grex.lock.jsonl schema, events.jsonl variants,
  TreeError variant set, CLI flag set on 4 verbs, JSON envelope keys,
  MCP SyncParams fields, ExecCtx::workspace field, ChildPath validation
  rules, lockfile sentinels, --retain-days semantics, cap-std root
  capability semantics, quarantine GC + restore semantics.
- Plugin-API explicitly stays UNSTABLE — v1.4.0 freeze candidate.

### Tests

- New unit + integration tests:
  `cli_workspace_pack_alias_parses_both`,
  `cli_workspace_deprecation_warns_once_per_process`,
  `cli_ls_doctor_json_envelopes_dual_emit_workspace_pack`,
  `mcp_sync_pack_or_workspace_precedence`.
- Extended `e2e_v1_3_0_readiness_smoke` with deprecation warn-once
  assertion.

### Notes

- No public API removal. All v1.3.0 surface changes are additive.
- MSRV unchanged (1.79).
- Plugin-API surface remains UNSTABLE; freeze candidate for v1.4.0.
```
