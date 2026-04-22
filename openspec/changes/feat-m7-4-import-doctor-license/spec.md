# feat-m7-4 — `grex import` + `grex doctor` + licence decision

**Status**: draft
**Milestone**: M7 (see [`../../../milestone.md`](../../../milestone.md) §M7)
**Depends on**: M5 pack-type plugin system (dispatch for `import`); M6 managed-gitignore block const (re-used by `doctor` sync check); M3 corruption-resistant manifest reader (re-used by `doctor` schema check).

## Motivation

`milestone.md` §M7 lists four deliverables: the MCP server (covered by prior m7 specs), plus three independent items — legacy-workspace ingest, health check verb, and licence lock — that the prior m7 specs do not touch. This change covers those three remaining items in one proposal so M7 can close cleanly. Each sub-scope has its own stage block in `tasks.md` so they can land in separate PRs if preferred.

## Goal

1. `grex import --from-repos-json <path>` parses a legacy flat `REPOS.json` (`[{url, path}]`) and emits equivalent `grex add` operations against the target workspace via the core API, with `--dry-run` + skip-on-conflict semantics.
2. `grex doctor` runs three read-only pack-health checks by default (manifest schema / gitignore sync / on-disk drift — the three pack-health checks from `milestone.md` §M7), prints a tabular summary, and exits `0`/`1`/`2` by severity. A fourth opt-in check (`config lint` — `.omne/cfg/*.md` frontmatter + `openspec/config.yaml` YAML parse) runs only under `--lint-config`. `--fix` auto-heals gitignore drift only.

> Vocabulary note: `milestone.md` §M7 enumerates `doctor` as "manifest schema check, gitignore sync check, on-disk drift (paths in REPOS.json not on disk + vice versa), lint (pack.yaml schema validate)". M3 actually shipped `grex.jsonl` (event-log) as the manifest format — `pack.yaml` in the milestone text is stale. Our manifest-schema check operates on `grex.jsonl` and our on-disk-drift check operates on the `grex.jsonl`-tracked pack set (the post-M3 analogue of "paths in REPOS.json"). The behaviour matches milestone intent even where the vocabulary diverges.
3. Lock the licence as dual **`MIT OR Apache-2.0`** — root `LICENSE-MIT`, `LICENSE-APACHE`, `LICENSE` pointer; every workspace crate inherits via `[workspace.package] license = "..."`; README licence section; `deny.toml` verified.

## Design

### Sub-scope 1 — `grex import --from-repos-json <path>`

**Input schema** (from root-repo `E:\repos\CLAUDE.md`):
```json
[{"url": "https://…", "path": "pack-name"}, …]
```
`path` is always a bare name (no slashes — enforced by source project's linter; we re-validate).

**Module**: `crates/grex-core/src/import.rs` (new). Public entry:
```rust
pub struct ImportPlan {
    pub imported: Vec<ImportEntry>,
    pub skipped:  Vec<ImportSkip>,   // path-collision with existing manifest row
    pub failed:   Vec<ImportFailure>,
}
pub struct ImportOpts { pub dry_run: bool }

pub async fn import_from_repos_json(
    ctx: &ExecCtx<'_>,
    repos_json: &Path,
    opts: ImportOpts,
) -> Result<ImportPlan, ImportError>;
```

**Pack-type heuristic** (keep it dumb — extensible later):
- `url` field present + looks like a git URL (starts `http`, `https`, `git@`, or ends `.git`) → `scripted` pack-type with git backend (same default as `grex add <url>` CLI).
- `url` empty or looks like a filesystem path → `declarative` pack-type.
- No attempt to detect meta packs in M7 — document as non-goal.

**Conflict policy**: if `path` already in the target `grex.jsonl`, record an `ImportSkip { path, reason: PathCollision, existing_kind }` entry and continue. Never overwrite; no `--force` flag in M7.

**Dry-run**: when `opts.dry_run`, build the full `ImportPlan` but return before any `add::run` dispatch or manifest append. `imported` entries in a dry-run plan carry a `would_dispatch: true` flag — CLI prints them as "DRY-RUN: would add …".

**CLI wiring**: `crates/grex/src/cli/verbs/import.rs` — replace the existing stub with a thin wrapper that builds `ExecCtx`, calls `import_from_repos_json`, and renders the plan (human: table; `--json`: serialised plan).

### Sub-scope 2 — `grex doctor`

**Module**: `crates/grex-core/src/doctor.rs` (new). Public entry:
```rust
pub struct DoctorReport { pub findings: Vec<Finding> }
pub struct Finding { pub check: CheckKind, pub severity: Severity, pub pack: Option<PackId>, pub detail: String, pub auto_fixable: bool }
pub enum CheckKind { ManifestSchema, GitignoreSync, OnDiskDrift, ConfigLint }
pub enum Severity { Ok, Warning, Error }
pub struct DoctorOpts { pub fix: bool, pub lint_config: bool }

pub async fn run_doctor(ctx: &ExecCtx<'_>, opts: DoctorOpts) -> Result<DoctorReport, DoctorError>;
```

By default (`lint_config = false`), `run_doctor` runs only the three pack-health checks (manifest schema, gitignore sync, on-disk drift). `CheckKind::ConfigLint` runs **only** when the caller passes `lint_config = true` (CLI: `--lint-config`). Rationale: config-lint reads workspace-meta files (`.omne/cfg/`, `openspec/config.yaml`) that most users don't own and can't fix from within a packs workspace; opting in keeps the default `doctor` run focused on pack health.

**Check 1 — manifest schema**: stream `grex.jsonl` through the M3 corruption-resistant reader (`manifest::stream_rows` or equivalent). Any `Err(ManifestReadError::MalformedRow { line, .. })` becomes a `Finding { severity: Error, auto_fixable: false }`. Clean rows produce no finding.

**Check 2 — gitignore sync**: for every pack in the manifest with a managed gitignore block, read `<pack_path>/.gitignore`, locate the managed block (`# >>> grex …` / `# <<< grex …` markers from M5-2), and compare the body to the rendered output of M6's `DEFAULT_MANAGED_GITIGNORE_PATTERNS` + pack-declared extras. Drift → `Finding { severity: Warning, auto_fixable: true }`.

**Check 3 — on-disk drift**: for every pack in the manifest, `fs::symlink_metadata(pack_path)` and assert (a) it exists, (b) its kind matches the declared pack type (directory for declarative/scripted; directory for meta with children). Missing or wrong-kind → `Finding { severity: Error, auto_fixable: false }`.

**Check 4 — config lint (opt-in, `--lint-config`)**: skipped by default. When `--lint-config` is passed: if `.omne/cfg/` exists, walk `*.md` frontmatter / `*.yaml` and `serde_yaml::from_str::<serde_yaml::Value>` them — any parse error becomes `Finding { severity: Warning }`. Same for `openspec/config.yaml` if present. Absent files are no-ops (not findings). Without the flag, `CheckKind::ConfigLint` never appears in the report.

**Severity roll-up for exit code**:
- `0`: all findings are `Ok` or checks produced zero findings.
- `1`: at least one `Warning` but no `Error`.
- `2`: at least one `Error`.

**`--fix`**: only `CheckKind::GitignoreSync` findings with `auto_fixable = true` are healed (re-render the managed block with `DEFAULT_MANAGED_GITIGNORE_PATTERNS` + declared extras via the M5-2 writer). All other checks stay read-only. Post-fix, re-run the sync check to confirm; persisted findings after fix downgrade the exit code only if they flip severity tier.

**CLI wiring**: `crates/grex/src/cli/verbs/doctor.rs` — human output is a three-row table by default (`CHECK | STATUS | DETAIL`) covering the three pack-health checks; a fourth `ConfigLint` row appears only when `--lint-config` is passed. `--json` emits the `DoctorReport` directly (report shape is identical; findings list is simply shorter without `--lint-config`). New flag: `--lint-config` (bool).

### Sub-scope 3 — Licence decision

**Decision**: `MIT OR Apache-2.0` (dual). Rationale: matches `tokio`, `serde`, `clap`, and the wider Rust ecosystem; dual-licence gives downstream consumers the patent grant from Apache while keeping MIT brevity.

**Artifacts**:
- `LICENSE-MIT` — canonical MIT text, copyright `2026 Yueyang Li`.
- `LICENSE-APACHE` — canonical Apache-2.0 text (no NOTICE file needed; we assert no third-party attributions beyond what Cargo already tracks).
- `LICENSE` — three-line pointer: "Licensed under either of Apache-2.0 (see LICENSE-APACHE) or MIT (see LICENSE-MIT) at your option."
- `Cargo.toml` root: add `[workspace.package]` block with `license = "MIT OR Apache-2.0"`, `authors`, `edition`, `repository`. Every crate's `Cargo.toml` replaces its own `license` / `authors` / `edition` / `repository` with `.workspace = true`.
- `README.md`: append `## License` section citing both files + the contribution clause ("contributions are licensed under the same dual licence unless stated otherwise").
- `deny.toml`: if present, verify `[licenses].allow` includes both `MIT` and `Apache-2.0`; if not present, leave for a follow-up — do not add `cargo-deny` in this change.

## File / module targets

| Concrete path | Change |
|---|---|
| `crates/grex-core/src/import.rs` | New — `import_from_repos_json` + `ImportPlan`/`ImportEntry`/`ImportSkip`/`ImportFailure`/`ImportError`. |
| `crates/grex-core/src/lib.rs` | `pub mod import;` |
| `crates/grex/src/cli/verbs/import.rs` | Replace stub with real CLI; add `--from-repos-json <path>` + `--dry-run` + `--json` flags. |
| `crates/grex/src/cli/args.rs` | Wire `Import` verb arguments. |
| `crates/grex-core/src/doctor.rs` | New — four checks + `run_doctor` + `DoctorReport`/`Finding`/`CheckKind`/`Severity`. |
| `crates/grex-core/src/lib.rs` | `pub mod doctor;` |
| `crates/grex/src/cli/verbs/doctor.rs` | Replace stub with real CLI; add `--fix` + `--json` flags; map severity to exit code. |
| `crates/grex/src/cli/args.rs` | Wire `Doctor` verb arguments. |
| `LICENSE-MIT` | New — canonical MIT text. |
| `LICENSE-APACHE` | New — canonical Apache-2.0 text. |
| `LICENSE` | New — pointer file. |
| `Cargo.toml` (root) | New `[workspace.package]` block with licence + authors + edition + repository. |
| `crates/grex-core/Cargo.toml` | Inherit `license`/`authors`/`edition`/`repository` via `.workspace = true`. |
| `crates/grex/Cargo.toml` | Same. |
| `README.md` | Append `## License` section. |
| `deny.toml` | Verify-only; do not create. |

## Test plan

### Unit

`crates/grex-core/src/import.rs` `#[cfg(test)]`:
- `import_parses_flat_repos_json_schema` — happy path, 3 entries, asserts `ImportPlan.imported.len() == 3`.
- `import_rejects_malformed_repos_json` — trailing comma, missing `url`, array-of-strings; all produce `ImportError::Parse`.
- `import_heuristic_git_url_maps_to_scripted` — URL `https://github.com/x/y.git` → pack-type `scripted`.
- `import_heuristic_bare_path_maps_to_declarative` — `url: ""` + `path: "foo"` → pack-type `declarative`.
- `import_dry_run_produces_plan_without_manifest_write` — snapshot manifest before/after, assert byte-equal.
- `import_path_collision_produces_skip_not_error` — pre-seed manifest with `path: "foo"`, run import containing `foo`, assert `skipped.len() == 1`, `imported` omits it.

`crates/grex-core/src/doctor.rs` `#[cfg(test)]`:
- `doctor_schema_check_clean_manifest_zero_findings`
- `doctor_schema_check_malformed_row_becomes_error_finding` — seed `grex.jsonl` with a truncated row.
- `doctor_gitignore_check_detects_drift` — mutate a managed block body; assert `Warning` finding with `auto_fixable: true`.
- `doctor_gitignore_check_clean_block_zero_findings`
- `doctor_on_disk_check_missing_pack_dir_is_error`
- `doctor_config_lint_invalid_yaml_is_warning`
- `doctor_config_lint_absent_dir_is_noop` — no `.omne/cfg/` → no findings.
- `doctor_exit_code_roll_up` — table-test: `[Ok]→0`, `[Warn]→1`, `[Err]→2`, `[Warn,Err]→2`.
- `doctor_fix_heals_gitignore_drift` — pre-drift a block, run `--fix`, re-run without `--fix`, assert zero findings.
- `doctor_fix_does_not_touch_schema_or_on_disk_findings` — schema error present + `--fix`; assert manifest byte-unchanged.

### Integration

`crates/grex/tests/import_cli.rs` (new):
- `import_from_repos_json_end_to_end` — temp workspace, pre-written `REPOS.json` fixture with 2 git URLs + 1 declarative; run `grex import --from-repos-json ./REPOS.json`; assert manifest has 3 new rows of the right pack-types.
- `import_dry_run_prints_plan_and_leaves_manifest_untouched` — snapshot manifest before/after; assert stdout contains `DRY-RUN:` prefix.
- `import_skips_path_collision_and_exits_zero` — pre-seed manifest; assert skip-count in stdout; exit code `0`.

`crates/grex/tests/doctor_cli.rs` (new):
- `doctor_clean_workspace_exits_zero_and_prints_ok_rows` — three `OK` table rows by default (manifest schema / gitignore sync / on-disk drift).
- `doctor_lint_config_flag_adds_config_row` — pass `--lint-config` on a clean workspace; assert a fourth `ConfigLint` row appears and is `OK`.
- `doctor_warn_drift_exits_one` — drift one gitignore block; assert exit `1`; assert `GitignoreSync` row is `WARNING`.
- `doctor_err_missing_pack_exits_two` — delete one pack dir; assert exit `2`.
- `doctor_fix_heals_gitignore_and_exits_zero_on_retry` — drift → `--fix` → re-run without `--fix` → exit `0`.
- `doctor_fix_does_not_touch_error_findings` — missing pack dir + `--fix`; re-run; still exit `2`; pack dir still missing.

### Licence

`crates/grex/tests/license_metadata.rs` (new):
- `workspace_package_declares_dual_license` — parse root `Cargo.toml` via `toml::Value`; assert `workspace.package.license == "MIT OR Apache-2.0"`.
- `every_workspace_crate_inherits_license` — walk `crates/*/Cargo.toml`; assert every `[package]` has `license.workspace = true`.
- `root_license_files_present_and_nonempty` — assert `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE` exist with non-zero length.
- `readme_has_license_section` — grep `README.md` for `## License`.

## Non-goals

- **No `grex import --from-<other-format>`** — only flat `REPOS.json` in M7. Other ingest formats (cfg/, gitmodules, package.json workspaces) are M8+.
- **No `--force` overwrite for import path collisions** — skip-only policy; user runs `grex rm` manually then re-imports.
- **No meta-pack detection in import heuristic** — every imported entry becomes `scripted` or `declarative`. Users promote to meta manually post-import.
- **`grex doctor --fix` does NOT auto-migrate malformed manifest rows** — M8 carries a dedicated `grex repair` verb.
- **No on-disk backup before `--fix`** — gitignore drift is recoverable from git history; keep the flag surgical.
- **No licence compliance scanning of dependencies in this change** — `cargo-deny` already covers it in a separate pipeline; we only verify `deny.toml` accepts our dual choice.
- **No `NOTICE` file** — we carry no third-party attributions beyond what Cargo records.
- **No symlink pack-type doctor coverage in M7** — the speculative test `doctor_on_disk_check_symlink_pack_is_ok_when_declared_symlink` is deferred to M8 when a `symlink` pack type actually lands. The M7 on-disk check only covers dir-present / dir-kind-match / missing-dir for the currently-shipping `declarative`, `scripted`, `meta` pack types.

## Dependencies

- **Prior**: M5 pack-type plugins (import dispatches via `add::run` which goes through plugin registry); M6-2 managed-gitignore default block const (doctor sync check reads this); M3 corruption-resistant manifest reader (doctor schema check reuses it).
- **Sibling**: other m7 specs (MCP server) — independent; can land in any order.
- **Next**: M8 — `grex repair` for malformed manifest auto-migration; additional import formats.

## Acceptance

1. `import_cli.rs` tests green — ingest + dry-run + skip-on-collision.
2. `doctor_cli.rs` tests green — all four checks + `--fix` + exit-code roll-up.
3. `license_metadata.rs` tests green — dual licence declared + inherited + files present + README section.
4. `cargo test --workspace` green; no regressions on M6 baseline.
5. `cargo clippy --all-targets --workspace -- -D warnings` clean. Per-fn LOC ≤ 50; CBO ≤ 10.
6. `cargo fmt --check` clean.
7. Manual smoke: (a) run `grex import --from-repos-json` against this very repo's `E:\repos\REPOS.json` fixture and inspect the plan; (b) `grex doctor` on a clean workspace prints 3 OK rows (manifest schema / gitignore sync / on-disk drift); passing `--lint-config` on the same workspace adds a 4th OK row.

## Source-of-truth links

- [`milestone.md`](../../../milestone.md) §M7 — 4 deliverables enumeration.
- [`openspec/feat-grex/spec.md`](../../feat-grex/spec.md) — success criteria for `import`/`doctor`.
- [`.omne/cfg/architecture.md`](../../../.omne/cfg/architecture.md) §Workspace — directional guidance for module placement. Note: the §Workspace section still reads "Single crate `grex` (lib + bin). Sub-crates avoided in v1" — this is stale post-M5, which shipped a multi-crate workspace (`grex-core` + `grex`). We cite architecture.md for directional intent (module lives in the core library crate), not literal text; `import.rs` / `doctor.rs` land under `crates/grex-core/src/` per the actual shipped layout.
- [`E:\repos\CLAUDE.md`](../../../../CLAUDE.md) — legacy `REPOS.json` schema reference (flat array of `{url, path}`).
- Prior-change voice: [`../feat-m6-2-per-pack-lock/spec.md`](../feat-m6-2-per-pack-lock/spec.md).
