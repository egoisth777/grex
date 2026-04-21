# feat-m6-2 — tasks (TDD)

**Convention**: tests first, then production, then green. Lock-ordering is load-bearing — do not skip the stress tests.

## Stage 1 — PackLock primitive

- [ ] Write `concurrency/packlock.rs` `#[cfg(test)]` unit tests (5 cases from spec §Unit — all fail).
- [ ] Implement `PackLock::acquire` + `PackLockError`.
- [ ] Add `ExecError::PackLockAcquire(PackLockError)` variant.
- [ ] Verify 5 unit tests pass.

## Stage 2 — Gitignore default-block extension

- [ ] Write gitignore test `pack_lock_file_auto_added_to_gitignore_managed_block` (fails).
- [ ] Extend default managed-block body with `.grex-lock` line.
- [ ] Verify test passes; ensure M5-2 gitignore round-trip tests still green (no duplicate-line regression).

## Stage 3 — Plugin integration

- [ ] Write `tests/sync_pack_lock.rs` — 3 integration tests (all fail at start).
- [ ] Add `PackLock::acquire` prologue to `MetaPlugin::{install,update,sync,teardown}`.
- [ ] Same for `DeclarativePlugin` + `ScriptedPlugin`.
- [ ] Verify integration tests pass.
- [ ] Sanity: M5 teardown-round-trip tests still green.

## Stage 4 — Stress + cross-process

- [ ] Extend `tests/sync_stress.rs` with `stress_overlapping_trees_no_deadlock` + `stress_100_concurrent_pack_ops_overlapping_roots` (both `#[ignore]`).
- [ ] Write `tests/sync_cross_process.rs` (`#[ignore]`).
- [ ] Run `cargo test -- --ignored` locally on Linux + Windows; both pass in < 60 s / run.

## Stage 5 — Optional property cover

- [ ] If time permits: `proptest` model in `tests/property_pack_lock.rs` — random schedules, no-overlap-per-path invariant. Mirrors feat-m6-3 theorem.

## Stage 6 — Polish + acceptance

- [ ] `cargo fmt --check` clean.
- [ ] `cargo clippy --all-targets --workspace -- -D warnings` clean. Per-fn LOC ≤ 50; CBO ≤ 10.
- [ ] `cargo test --workspace` green. M5 baseline unchanged.
- [ ] `cargo test --workspace --features grex-core/plugin-inventory` green.
- [ ] Manual two-terminal smoke: workspace lock errors fast on the second invocation.
- [ ] Update `progress.md`.
