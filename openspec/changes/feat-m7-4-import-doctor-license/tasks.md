# feat-m7-4 — tasks (TDD)

**Convention**: tests first, then production, then green. Three sub-scopes are independent — stages 1-2 (import), 3-5 (doctor), 6 (licence) can land in separate PRs if preferred. Do not skip the exit-code roll-up test or the gitignore-drift cover — they gate `--fix` safety.

## Stage 1 — `grex import` unit tests + core

- [ ] Write `crates/grex-core/src/import.rs` `#[cfg(test)]` — 6 unit cases from spec §Unit (all fail).
- [ ] Implement `ImportPlan` / `ImportEntry` / `ImportSkip` / `ImportFailure` / `ImportError` + `ImportOpts`.
- [ ] Implement `import_from_repos_json` — parse, heuristic, conflict skip, dry-run short-circuit, dispatch via `add::run`.
- [ ] `pub mod import;` in `crates/grex-core/src/lib.rs`.
- [ ] Verify 6 unit tests pass.

## Stage 2 — `grex import` CLI wiring

- [ ] Write `crates/grex/tests/import_cli.rs` — 3 integration tests (all fail at start).
- [ ] Replace `crates/grex/src/cli/verbs/import.rs` stub with real wrapper; render `ImportPlan` (human table + `--json`).
- [ ] Wire `Import` subcommand args in `crates/grex/src/cli/args.rs` (`--from-repos-json <path>`, `--dry-run`, `--json`).
- [ ] Verify 3 integration tests pass.

## Stage 3 — `grex doctor` unit tests + core

- [ ] Write `crates/grex-core/src/doctor.rs` `#[cfg(test)]` — 9 unit cases from spec §Unit covering all 4 check kinds + exit-code roll-up (all fail).
- [ ] Implement `DoctorReport` / `Finding` / `CheckKind` / `Severity` / `DoctorOpts` / `DoctorError`.
- [ ] Implement four check functions (manifest schema via M3 reader; gitignore sync via M6 const; on-disk drift via `fs::symlink_metadata`; config lint via `serde_yaml`).
- [ ] Implement `run_doctor` orchestrator (sequential, read-only, build `DoctorReport`).
- [ ] `pub mod doctor;` in `crates/grex-core/src/lib.rs`.
- [ ] Verify 9 unit tests pass.

## Stage 4 — `grex doctor` CLI wiring

- [ ] Write `crates/grex/tests/doctor_cli.rs` — 3 baseline integration tests: `doctor_clean_workspace_exits_zero_and_prints_ok_rows`, `doctor_warn_drift_exits_one`, `doctor_err_missing_pack_exits_two` (all fail).
- [ ] Replace `crates/grex/src/cli/verbs/doctor.rs` stub with real wrapper; render `DoctorReport` (4-row table + `--json`); map severity to process exit code.
- [ ] Wire `Doctor` subcommand args in `crates/grex/src/cli/args.rs` (`--fix`, `--json`).
- [ ] Verify 3 integration tests pass.

## Stage 5 — `grex doctor --fix` auto-heal

- [ ] Write unit tests `doctor_fix_heals_gitignore_drift` + `doctor_fix_does_not_touch_schema_or_on_disk_findings` (fail).
- [ ] Write integration tests `doctor_fix_heals_gitignore_and_exits_zero_on_retry` + `doctor_fix_does_not_touch_error_findings` (fail).
- [ ] Implement `--fix` branch: iterate findings, for `CheckKind::GitignoreSync` with `auto_fixable = true` call the M5-2 gitignore writer with `DEFAULT_MANAGED_GITIGNORE_PATTERNS` + declared extras; skip all other kinds.
- [ ] Post-fix: re-run sync check to confirm; persist new severity in report.
- [ ] Verify 4 tests pass.
- [ ] Sanity: M5-2 gitignore round-trip tests still green (no duplicate-line regression).

## Stage 6 — Licence files + metadata + README + CI

- [ ] Write `crates/grex/tests/license_metadata.rs` — 4 cases from spec §Licence (all fail).
- [ ] Create `LICENSE-MIT` (canonical MIT text, copyright `2026 Yueyang Li`).
- [ ] Create `LICENSE-APACHE` (canonical Apache-2.0 text; no NOTICE).
- [ ] Create `LICENSE` pointer (3-line dual reference).
- [ ] Add `[workspace.package]` block to root `Cargo.toml` (`license = "MIT OR Apache-2.0"` + `authors` + `edition` + `repository`).
- [ ] For every crate in `crates/*/Cargo.toml`: replace per-crate `license`/`authors`/`edition`/`repository` with `.workspace = true`.
- [ ] Append `## License` section to `README.md` (cite both files + contribution clause).
- [ ] If `deny.toml` present: verify `[licenses].allow` includes `MIT` + `Apache-2.0`; if not, leave for follow-up (do not add `cargo-deny` in this change).
- [ ] Verify 4 licence tests pass.

## Stage 7 — Polish + acceptance

- [ ] `cargo fmt --check` clean.
- [ ] `cargo clippy --all-targets --workspace -- -D warnings` clean. Per-fn LOC ≤ 50; CBO ≤ 10.
- [ ] `cargo test --workspace` green. M6 baseline unchanged.
- [ ] Manual smoke: (a) `grex import --from-repos-json E:\repos\REPOS.json --dry-run` — inspect plan; (b) `grex doctor` on a clean workspace — 4 OK rows.
- [ ] Update `progress.md` + mark M7 items 2-4 closed.
