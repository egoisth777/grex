//! grex-core — manifest, lockfile, scheduler, pack model, plugin traits.
//!
//! M2 lands: manifest event log + lockfile + atomic file primitives.

#![forbid(unsafe_code)]

pub mod fs;
pub mod lockfile;
pub mod manifest;
pub mod pack;
pub mod vars;

pub use pack::{
    Action, ChildRef, Combiner, EnvArgs, EnvScope, ExecOnFail, ExecSpec, MkdirArgs, OsKind,
    PackManifest, PackParseError, PackType, Predicate, RequireOnFail, RequireSpec, RmdirArgs,
    SchemaVersion, SymlinkArgs, SymlinkKind, WhenSpec,
};
pub use vars::{expand, VarEnv, VarExpandError};

pub mod scheduler {}
pub mod plugin {}
