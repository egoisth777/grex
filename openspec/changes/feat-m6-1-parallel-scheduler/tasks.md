# feat-m6-1 — tasks (TDD)

**Convention**: tests first, then production code, then green.

## Stage 1 — CLI surface

- [ ] Write `cli_parse.rs` tests (6 cases from spec §Test plan — all currently fail).
- [ ] Add `--parallel N` to `SyncArgs` in `crates/grex/src/cli.rs`; honour `GREX_PARALLEL` env when absent.
- [ ] Verify the 6 CLI-parse tests pass.

## Stage 2 — Scheduler module

- [ ] Write `crates/grex-core/src/scheduler.rs` `#[cfg(test)]` unit tests (4 cases — all currently fail).
- [ ] Implement `Scheduler::new / permits / acquire`.
- [ ] Add `num_cpus` dep to `crates/grex-core/Cargo.toml` if missing.
- [ ] Re-export `scheduler` from `lib.rs`.
- [ ] Verify the 4 unit tests pass.

## Stage 3 — `ExecCtx` plumbing

- [ ] Extend `ExecCtx<'a>` with `scheduler: Option<&'a Arc<Semaphore>>` (field-level `#[non_exhaustive]` not needed — struct already is).
- [ ] Update every `ExecCtx` constructor site (expect ~4: `sync::run`, `sync::run_actions`, `plan::plan_actions`, test helpers).
- [ ] `cargo check --workspace` green.

## Stage 4 — Integration + stress

- [ ] Write `tests/sync_parallel.rs` — 2 new tests (serial-order, 100-pack race).
- [ ] Write `tests/sync_stress.rs` — 1 `#[ignore]` test.
- [ ] Wire `sync::run_actions` to construct `Scheduler::new(opts.parallel)` and plumb into `ExecCtx`.
- [ ] Verify integration tests pass; stress test passes under `cargo test -- --ignored`.

## Stage 5 — Polish + acceptance

- [ ] `cargo fmt --check` clean.
- [ ] `cargo clippy --all-targets --workspace -- -D warnings` clean (note: `scheduler.rs` must stay under per-fn 50-LOC / CBO-10 workspace lints).
- [ ] `cargo test --workspace` green — no regressions on M5 baseline.
- [ ] `cargo test --workspace --features grex-core/plugin-inventory` green.
- [ ] Update `progress.md` "Last endpoint" with commit SHA + test count delta.
