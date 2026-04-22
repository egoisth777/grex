# feat-m7-2 — tasks (TDD)

**Convention**: tests first, fixtures second, harness helpers third, glue last. The saturation invariants in L4 and the budget bounds in L5 are load-bearing — do not loosen them under flake pressure; diagnose root cause.

**Layer scope**: L2 (3 stages — duplex handshake + per-OS real-pipe on Linux + macOS + Windows), L3 (2 stages — normaliser + parity loop), L4 (2 stages — stress baseline + saturation barrier), L5 (1 stage — cancel chaos). L1 belongs to feat-m7-1. L6 – L8 move to feat-m7-3.

## Stage 1 — L2 duplex handshake (red)

- [ ] 1.1 Create `crates/grex-mcp/tests/common/mod.rs` with empty `TestFixture` + `new_duplex_server()` helper stub (returns `unimplemented!()`).
- [ ] 1.2 Write `crates/grex-mcp/tests/handshake.rs` with **5 failing** `#[tokio::test]` cases — `handshake_ok`, `request_before_init_rejected`, `double_init_rejected`, `graceful_shutdown_drains`, `protocol_version_echoed`.
- [ ] 1.3 Wire `Cargo.toml` `[dev-dependencies]`: `tokio` (features `full`), `serde_json`, `tempfile`, `anyhow`.
- [ ] 1.4 Commit: `test(m7-2): L2 handshake red — 5 failing duplex cases`.
- [ ] 1.5 **Verify**: `cargo test -p grex-mcp --test handshake` reports exactly 5 failures, 0 passes.

## Stage 2 — L2 duplex harness + handshake (green)

- [ ] 2.1 Implement `common::new_duplex_server()` — pairs `tokio::io::duplex(4096)`, spawns server via `rmcp` `transport-io` framer on one half, returns client half.
- [ ] 2.2 Implement `common::Client` with `initialize`, `notify`, `call`, `shutdown` methods (thin JSON-RPC line writer/reader).
- [ ] 2.3 Make all 5 handshake tests pass. Error-code assertions reference `.omne/cfg/mcp.md` §Error codes — `-32002` / `data.kind = "init_state"` for init-state violations.
- [ ] 2.4 Commit: `feat(m7-2): L2 handshake green — duplex E2E harness`.
- [ ] 2.5 **Verify**: `cargo test -p grex-mcp --test handshake` — 5 passes, 0 failures.

## Stage 3 — L2 real-pipe per-OS guard

> **Note:** Stage 3 tests went GREEN on first run because m7-1's server contract
> already satisfies the back-pressure + stderr-null invariants. Tests are real
> (would fail on regression — strict JSON parsing, monotonic IDs, cumulative
> >64 KiB threshold, post-init liveness). RED-first discipline N/A here.

- [ ] 3.1 Write `crates/grex-mcp/tests/real_pipe_linux.rs` (`#[cfg(target_os = "linux")]`) with 2 failing cases — `large_response_crosses_pipe_buffer`, `client_stderr_close_does_not_panic_server`.
- [ ] 3.2 Write `crates/grex-mcp/tests/real_pipe_macos.rs` (`#[cfg(target_os = "macos")]`) with the same 2 cases.
- [ ] 3.3 Write `crates/grex-mcp/tests/real_pipe_windows.rs` (`#[cfg(target_os = "windows")]`) with the same 2 cases.
- [ ] 3.4 Add `assert_cmd` to `[dev-dependencies]`; implement helpers to spawn release `grex serve` via `Command::cargo_bin("grex")`.
- [ ] 3.5 Fixture for large-response: seed > 1 024 packs in a tempdir workspace so `ls` returns > 64 KiB.
- [ ] 3.6 Make per-OS tests pass locally on the host OS; CI covers the other two.
- [ ] 3.7 Commit: `test(m7-2): L2 real-pipe guard per OS (linux+macos+windows)`.
- [ ] 3.8 **Verify**: on Linux `cargo test -p grex-mcp --test real_pipe_linux` — 2 passes; on macOS `cargo test -p grex-mcp --test real_pipe_macos` — 2 passes; on Windows same for `real_pipe_windows`.

## Stage 4 — L3 normaliser (red → green)

- [ ] 4.1 Extend `common/mod.rs` with `normalize(value: Value) -> Value` — implements `<TS>` + `<PATH>` substitutions ONLY. No `<ID>`, `<PID>`, `<SHA>`.
- [ ] 4.2 Add `common::run_cli_json(fixture, verb, args) -> Value` — spawns `grex <verb> --json`, captures stdout, parses.
- [ ] 4.3 Add `common::run_mcp_tool(fixture, verb, params) -> Value` — drives a duplex server through `initialize` + `tools/call`.
- [ ] 4.4 Write 6 unit tests for `normalize()` under `#[cfg(test)]` in `common/mod.rs`: timestamp rewrite, absolute-path rewrite, nested object rewrite, no-op on scalars, mixed content, idempotent (`normalize(normalize(x)) == normalize(x)`).
- [ ] 4.5 Commit: `feat(m7-2): L3 normaliser + CLI/MCP helpers`.
- [ ] 4.6 **Verify**: normalize unit tests — 6 passes.

## Stage 5 — L3 per-verb parity (11 cases)

- [ ] 5.1 Export `pub const VERBS_EXPOSED: &[&str]` from `grex-mcp::lib` (skip if feat-m7-1 landed it).
- [ ] 5.2 Write `crates/grex-mcp/tests/parity.rs` with `async fn assert_parity(verb, args, mcp_params)` helper (**not** a macro).
- [ ] 5.3 Add **11 failing** `#[tokio::test]` cases — one per verb in `VERBS_EXPOSED`. Destructive verbs (`rm`, `run`, `exec`) get isolated tempdir fixtures.
- [ ] 5.4 Add `assert!(tools_list.tools.len() >= VERBS_EXPOSED.len(), …)` as a preflight check inside each test (cheap, catches drift early).
- [ ] 5.5 Make all 11 parity tests green. If a field cannot be reconciled, either fix the non-matching side or extend `normalize()` with a **new** placeholder AND document the justification in `common/mod.rs`.
- [ ] 5.6 Commit: `feat(m7-2): L3 CLI/MCP parity — 11 verbs green`.
- [ ] 5.7 **Verify**: `cargo test -p grex-mcp --test parity` — 11 passes, 0 failures.

## Stage 6 — L4 stress scaffold (red)

- [ ] 6.1 Add `#[cfg(test)]` barrier-wait hook inside `grex-mcp::handlers` — a `tokio::sync::Barrier` plumbed through `ExecCtx` via a test-only extension trait. Production path is untouched.
- [ ] 6.2 Write `crates/grex-mcp/tests/stress.rs` with 1 failing case — `stress_100x11_no_oversubscription` asserting `scheduler.high_water() >= PARALLEL` AND `<= PARALLEL`.
- [ ] 6.3 Initial wall-clock budget: **5 000 ms** (placeholder). Add a TODO comment linking the recalibration task (Stage 8.4).
- [ ] 6.4 Commit: `test(m7-2): L4 stress scaffold — saturation red`.
- [ ] 6.5 **Verify**: `cargo test -p grex-mcp --test stress` — 1 failure.

## Stage 7 — L4 stress green + same-pack serialisation + 3x repeat

- [ ] 7.1 Drive 100 clients × 11 verbs through disjoint `duplex` pairs against one shared `Arc<Server>`.
- [ ] 7.2 Make `stress_100x11_no_oversubscription` pass — both inequalities hold.
- [ ] 7.3 Add `stress_same_pack_serialises` — 8 concurrent `tools/call{name:"sync", arguments:{pack:"p1"}}`; assert `ActionStarted(p1, i+1)` strictly follows `ActionCompleted(p1, i)` for `i = 0..7`.
- [ ] 7.4 Add `stress_no_deadlock_across_3_iterations` — run both above 3× back-to-back in one `#[tokio::test]`; any iteration failure fails the test. This is the CI repeat policy from the spec, expressed in-code.
- [ ] 7.5 Commit: `feat(m7-2): L4 stress green — saturation + same-pack serialisation + 3x repeat`.
- [ ] 7.6 **Verify**: `cargo test -p grex-mcp --test stress` — 3 passes; run `cargo test --release` locally and confirm p50 / p99 fit under 5 s.

## Stage 8 — L5 cancellation chaos + budget recalibration

- [ ] 8.1 Write `crates/grex-mcp/tests/cancel.rs` with 11 parametric failing cases (one per `VERBS_EXPOSED` entry) — each sends `tools/call` then immediately `notifications/cancelled`; asserts `-32800 RequestCancelled` OR clean `CallToolResult`.
- [ ] 8.2 Add `cancel_permit_released_under_budget` — post-cancel, acquire `PARALLEL` permits within budget; fail if any `acquire` blocks.
- [ ] 8.3 Add `cancel_pack_lock_released_under_budget` — post-cancel, `PackLock::acquire(same_path)` within budget.
- [ ] 8.4 Budgets: **250 ms** on Linux / macOS, **500 ms** on Windows. Encode as `#[cfg]`-selected const; do NOT introduce a runtime flag.
- [ ] 8.5 Make all 13 cancel tests green. If a permit/pack-lock leak is found, fix the handler — do not relax the budget.
- [ ] 8.6 Recalibrate L4 wall-clock budget: pull p99 from the first CI green run's `cargo test --timings` artifact, multiply by 1.5, commit the new value.
- [ ] 8.7 Commit: `feat(m7-2): L5 cancel chaos green + L4 budget recalibrated`.
- [ ] 8.8 **Verify**: `cargo test -p grex-mcp` — all L2 / L3 / L4 / L5 suites green on Linux + Windows CI; 3× stress repeat clean; clippy `-D warnings` clean.

## Exit checklist

- [ ] Spec §Acceptance 1 – 7 all satisfied.
- [ ] No new top-level crate in workspace — `cargo metadata` diff shows only `[dev-dependencies]` additions under `grex-mcp`.
- [ ] `VERBS_EXPOSED.len() == 11`; `tools/list` returns `>= 11`; no regression on the inequality.
- [ ] Normaliser token set is still `{<TS>, <PATH>}` OR any addition is documented with a linked failing-test justification.
- [ ] CI job runs the stress harness 3× back-to-back and both `high_water` inequalities pass every iteration.
- [ ] L5 OS-specific budgets encoded as `#[cfg]` consts, not runtime flags.
- [ ] Update `progress.md` with the L2 – L5 close-out line; leave L6 – L8 as open for feat-m7-3.
