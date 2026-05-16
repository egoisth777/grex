---
slug: feat-v-1-4-1-tasks
type: spec
status: backfilled
last_updated: 2026-05-16
topic: smoke-test-bug-bundle-tasks
depends_on: [feat-v1.4.1]
---

# feat-v1.4.1 — tasks

> **Backfilled 2026-05-16, post-implementation.** Every checkbox below is `[x]` because the work landed
> before the triplet was authored. See `proposal.md` § Backfilled banner for context.

## Branch + version

- [x] Branch `v1.4.1` off `main @ 1f7a380`.
- [x] Bump `Cargo.toml [workspace.package] version` from `1.4.0` → `1.4.1`.
- [x] Bump internal pins (`grex-core`, `grex-mcp`, `grex-plugins-builtin`) in `workspace.dependencies`.
- [x] Bump `crates/xtask/Cargo.toml`'s pin on `grex-cli`.
- [x] Bump `crates/xtask/tests/version_test.rs::EXPECTED_WORKSPACE_VERSION` → `"1.4.1"`.

## Core fixes

- [x] **B1.1** New module `crates/grex-core/src/pack/yaml_writer.rs` — raw `serde_yaml::Value` round-trip
  appender with skeleton synthesis and idempotency. 7 unit tests.
- [x] **B1.2** `crates/grex-core/src/add.rs::add_pack` — after `Event::Add` append, call
  `append_child_to_pack_yaml` with derived pack.yaml path. New `AddError::PackYaml` variant.
- [x] **B1.3** `crates/grex-core/src/import.rs::commit_plan` — same bridge. New `ImportError::PackYaml`
  variant. New `RawEntry.platform: Option<String>` field. New `compose_path` helper that prefixes
  `<platform>/` when platform is set + non-blank. 3 v1.4.1 unit tests.
- [x] **B2** Additive `pack_yaml_updated: bool` field on `AddReport`. Wired into
  `crates/grex/src/cli/verbs/add.rs::emit_json` so `--json` envelope surfaces it.

## CLI doc strings

- [x] `crates/grex/src/cli/args.rs` — rewrite the `Add` and `Import` enum-variant docs to document the
  bridge.

## Doctor refinements

- [x] **B4** `crates/grex-core/src/doctor/mod.rs::read_gitignore_top_level` reads the workspace
  `.gitignore` and returns a `BTreeSet<String>` of top-level dir names to skip from drift detection.
  `collect_disk_to_manifest_findings` consults it before flagging as `OnDiskDrift`.
- [x] **B5** `crates/grex/src/cli/verbs/doctor.rs::print_table` routes `OK` rows + header to stdout;
  `WARN` / `ERROR` rows to stderr. `doctor_lint_config_flag_runs_config_check` test updated to assert
  on stderr.

## Walker / sync engine

- [x] **B6** `Cargo.toml` workspace pin on `gix` — add `blocking-http-transport-reqwest-rust-tls`
  feature.
- [x] **B6.1** `deny.toml` — add `CDLA-Permissive-2.0` to the licenses allowlist.
- [x] **B7** `crates/grex-core/src/tree/graph_build.rs` — add `BuildOptions { tolerate_unsynced_children:
  bool }` (marked `#[non_exhaustive]`). Preserve `build_graph` as a thin forwarder; new entry point
  `build_graph_with`. Thread `opts` through `walk_recursive`/`process_children`/`handle_child` (all
  marked `#[allow(clippy::too_many_arguments)]` matching the existing walker.rs convention).
  Synthesize on `ManifestNotFound` when `tolerate_unsynced_children` is set, regardless of
  `dest.exists()` (handles Windows `\\?\C:\...` verbatim paths).
- [x] **B7.1** `crates/grex-core/src/sync.rs::build_and_validate_graph` accepts a new `dry_run: bool`
  arg; wires `BuildOptions { tolerate_unsynced_children: dry_run }`. Both call sites
  (`run`, `run_teardown`) updated; teardown passes `false` (it operates against materialized packs).
- [x] **B7.2** `crates/grex-core/src/tree/mod.rs` — re-export `build_graph_with` and `BuildOptions`.
- [x] **B8** `crates/grex-core/src/tree/walker.rs::dest_has_nongit_content` helper +
  `eprintln!("warning: declared child slot ... collides with pre-existing non-git content")` from the
  Phase 1 `Missing` arm.

## CLI integration tests

- [x] `crates/grex/tests/add_cli.rs` — 4 v1.4.1 CLI tests (`v141_add_materializes_pack_yaml_children_when_missing`,
  `v141_add_appends_to_existing_pack_yaml_children`, `v141_add_json_envelope_reports_pack_yaml_updated`,
  `v141_add_dry_run_leaves_pack_yaml_alone`).
- [x] `crates/grex/tests/import_cli.rs` — 3 v1.4.1 CLI tests (`v141_import_composes_platform_prefix_into_child_path`,
  `v141_import_materializes_pack_yaml_children`, `v141_import_ls_sees_imported_packs`).
- [x] `crates/grex/tests/doctor_cli.rs` — `doctor_lint_config_flag_runs_config_check` flipped to assert
  on stderr.

## Real-smoke offline tier (NEW)

- [x] `crates/real-smoke/src/seed.rs` — `seed_bare_repo`, `seed_pack_template`, `file_url`,
  `render_repos_json`. Hermetic, no network. Split into `prepare_work_dir` / `init_seed_commit` /
  `produce_bare_clone` helpers to fit cyclomatic budget.
- [x] `crates/real-smoke/src/journey.rs` — `Step`, `Journey`, `JourneyReport`, `Assertion` type alias.
  Reusable assertions: `stdout_contains_all`, `file_exists`, `path_absent`, `events_jsonl_add_count`,
  `pack_yaml_has_children`.
- [x] `crates/real-smoke/src/grex_cli.rs::locate_grex_bin` — probe `target/llvm-cov-target/` alongside
  the conventional `target/{debug,release}/`. Honour `CARGO_TARGET_DIR` env.
- [x] `crates/real-smoke/src/lib.rs` — re-export `seed`, `journey` modules.
- [x] `crates/real-smoke/tests/cfg_shape.rs` — `cfg_shape_offline_journey_covers_all_seven_v140_bugs`
  + `cfg_shape_idempotent_import_skips_duplicates`. Helpers (`step_init`, `step_import`, `step_ls`,
  `step_sync`, `assert_doctor_clean`) extracted to fit cyclomatic budget.

## Online-tier real-smoke regression updates

- [x] `crates/real-smoke/tests/regression.rs::t_b09_stub_verbs_exit_nonzero` — inverted assertion
  (v1.4.0 wired the verbs; tests now MUST succeed with no stub marker).
- [x] `crates/real-smoke/tests/regression.rs::t_b10_add_ref_flag` — spell `grex add` with URL as
  positional (was `--url <URL>` which clap correctly rejects).
- [x] `crates/real-smoke/tests/regression.rs::read_lockfile` — try `grex.lock.jsonl` first (the
  canonical resolution lockfile). Parse JSONL line-by-line into a `Value::Sequence`.
- [x] `crates/real-smoke/tests/regression.rs::t_b14_lockfile_branch_carries_ref` — discriminator on
  `{id, path, sha}` (legacy `url` field retired). Walker logic extracted to module-level helpers
  (`collect_lockfile_branch_state`, `walk_lockfile_value`, `inspect_mapping`) to fit cyclomatic budget.
  Root meta-pack entry exempted from `branch` non-empty check (no parent ChildRef to mirror — v1.3.1
  B14 contract).

## Out-of-tree fixture corrections

- [x] `egoisth777/grex-test-leaf @ d1203cf` — `type: pack` → `type: scripted`.
- [x] `egoisth777/grex-test-leaf-1` — NEW repo. `name: leaf-1`.
- [x] `egoisth777/grex-test-leaf-2` — NEW repo. `name: leaf-2`.
- [x] `egoisth777/grex-test-leaf-3` — NEW repo. `name: leaf-3`.
- [x] `egoisth777/grex-test-meta-flat @ deb7e89` — children URLs flipped to leaf-{1,2,3}.
- [x] `egoisth777/grex-test-meta-nested @ a05114c` — children paths use canonical fixture names.

## Man-page regen

- [x] `cargo run -p xtask -- gen-man` — regenerate `man/grex.1`, `man/grex-add.1`, `man/grex-import.1`
  for the v1.4.1 help text expansions.

## CHANGELOG

- [x] Add `## [1.4.1] - 2026-05-16` section per Keep-a-Changelog 1.1.0 — Fixed / Added / Changed
  subsections.

## CI verification

- [x] `cargo test --workspace` green locally (Windows).
- [x] `cargo clippy --workspace --all-targets -- -D warnings` clean.
- [x] `cargo fmt --all -- --check` clean.
- [x] `cargo doc --workspace --no-deps --all-features` clean under `RUSTDOCFLAGS="-D warnings"`.
- [x] All 20 active CI checks on PR #80 SUCCESS (build ubuntu/macos/windows, coverage, cargo-deny,
  cargo-audit, cargo-machete, typos, msrv 1.79, rustdoc, man-drift, code-metrics, Lean4 proof gate,
  MCP protocol conformance, release-plan, mdbook build, CodeRabbit, real-smoke).
- [x] Real-smoke workflow triggered via `real-smoke` PR label — green.

## Discipline remediations (post-merge)

- [x] **A** Branch `feat-v1.4.1-ssot-update` off `inst/main`.
- [x] **B** Append v1.4.1 endpoint to `inst/grad/progress.md`. Open inst-side PR.
- [x] **C** Backfill `openspec/changes/feat-v1.4.1/{proposal,design,tasks}.md` — THIS FILE LANDING IS
  STAGE C COMPLETING.
- [x] **D** `inst/grad/milestone.md` — no per-patch slot for v1.4.x. No update needed.
- [ ] **E** `git filter-branch --msg-filter` over `v1.4.1`'s 10 commits to strip `Co-Authored-By:`
  trailers. Update PR #80 body to remove the `🤖 Generated with [Claude Code]` line. `git push
  --force-with-lease`. Re-run CI; expect no-op since the diff is byte-identical.

## Squash-merge

- [ ] PR #80 squash-merged into `main` with the v1.4.1 endpoint's `Goal` section as the commit body.
- [ ] Tag `v1.4.1` on the squash commit; cargo-dist release workflow auto-fires on tag push.
- [ ] `inst/grad/progress.md` endpoint updated post-merge — flip the "Awaiting" block to "Squash commit
  `<sha>` on main; release tag pushed."
