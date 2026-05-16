//! `real-smoke` — black-box real-smoke test harness for the `grex` CLI.
//!
//! This crate is a workspace member that intentionally does **not** depend on
//! `grex-core` or `grex-cli`. It drives a pre-built `grex` executable as a
//! subprocess (via [`grex_cli::run`]) against pre-provisioned GitHub fixture
//! repositories using git worktrees ([`worktree::WorktreeGuard`]).
//!
//! Architecture:
//! - [`worktree`] — RAII git-worktree manager. Caches a base clone per repo
//!   URL and adds throwaway worktrees per test invocation.
//! - [`grex_cli`] — thin `Command::output()` driver capturing stdout / stderr
//!   separately. No grex-internal types — strings + ints only.
//! - [`assertions`] — fs and string-shape assertion helpers consumed by the
//!   regression test crate (W2c).
//! - [`fixtures`] — hardcoded fixture URL registry (six pre-provisioned repos
//!   under `egoisth777` on GitHub).
//!
//! The actual regression test cases live in `crates/real-smoke/tests/` (owned
//! by W2c). The `real-smoke` binary in `src/main.rs` is a thin runner that
//! exits 0 with usage info — discovery / orchestration is delegated to
//! `cargo test -p real-smoke`.

pub mod assertions;
pub mod fixtures;
pub mod grex_cli;
pub mod journey;
pub mod seed;
pub mod worktree;
