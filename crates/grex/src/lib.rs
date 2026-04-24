//! Library surface for `grex-cli`.
//!
//! The shipped product of this crate is the `grex` binary (see
//! `src/main.rs`). This `lib.rs` exists solely so out-of-tree tooling
//! (notably `crates/xtask`, the man-page generator) can reuse the
//! `clap::Command` tree defined in [`cli::args::Cli`] without
//! re-declaring the CLI surface — man pages stay a passive projection
//! of the derive tree per spec.
//!
//! Nothing here is part of the public API for end users; semver guards
//! the binary CLI surface, not these re-exports. Keep this file as thin
//! as possible so the shim rule (man pages never mutate the CLI) is
//! easy to audit.

pub mod cli;
