//! grex-plugins-builtin — built-in action primitives and pack-type handlers.
//!
//! Stub crate reserved for v2 third-party plugin loading. In v1, all
//! builtins live in `grex-core::plugin` and are re-exported here for
//! forward-compat import paths.
//!
//! M4-A: the Tier-1 action plugins (`symlink`, `env`, `mkdir`, `rmdir`,
//! `require`, `when`, `exec`) live inside `grex-core::plugin` because the
//! wet-run logic is already there; hoisting the structs alone into this
//! crate would either force the executor free functions out of
//! `grex-core` or create a circular `grex-core` ↔ `grex-plugins-builtin`
//! dependency. This crate therefore re-exports the canonical
//! [`register_builtins`] path and the per-action plugin types so
//! downstream CLI crates can bind to `grex_plugins_builtin::...` even
//! while the implementations co-locate with the executor.

#![forbid(unsafe_code)]

pub use grex_core::plugin::{
    register_builtins, ActionPlugin, EnvPlugin, ExecPlugin, MkdirPlugin, Registry, RequirePlugin,
    RmdirPlugin, SymlinkPlugin, WhenPlugin,
};
