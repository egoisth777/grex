//! grex-core — manifest, lockfile, scheduler, pack model, plugin traits.
//!
//! M2 lands: manifest event log + lockfile + atomic file primitives.

#![deny(unsafe_code)]

pub mod env;
pub mod execute;
pub mod fs;
pub mod git;
pub mod lockfile;
pub mod log;
pub mod manifest;
pub mod pack;
pub mod sync;
pub mod tree;
pub mod vars;

#[doc(hidden)]
pub use env::EnvResolver;
pub use execute::{
    ActionExecutor, ExecCtx, ExecError, ExecResult, ExecStep, FsExecutor, PlanExecutor, Platform,
    PredicateOutcome, StepKind,
};
pub use git::{ClonedRepo, GitBackend, GitError, GixBackend};
#[doc(hidden)]
pub use log::{ActionLogger, LogLevel, TracingLogger};
pub use pack::{
    run_all, Action, ChildRef, Combiner, EnvArgs, EnvScope, ExecOnFail, ExecSpec, MkdirArgs,
    OsKind, PackManifest, PackParseError, PackType, PackValidationError, Predicate, RequireOnFail,
    RequireSpec, RmdirArgs, SchemaVersion, SymlinkArgs, SymlinkKind, UnlinkArgs, Validator,
    WhenSpec,
};
pub use tree::{EdgeKind, FsPackLoader, PackGraph, PackLoader, PackNode, TreeError, Walker};
pub use vars::{expand, VarEnv, VarExpandError};

pub mod scheduler;

pub use scheduler::{Cancelled, Scheduler};

pub mod pack_lock;

pub use pack_lock::{PackLock, PackLockError, Tier, PACK_LOCK_FILE_NAME};

pub mod plugin;

pub use plugin::{
    register_builtins, ActionPlugin, EnvPlugin, ExecPlugin, MkdirPlugin, Registry, RequirePlugin,
    RmdirPlugin, SymlinkPlugin, UnlinkPlugin, WhenPlugin,
};
