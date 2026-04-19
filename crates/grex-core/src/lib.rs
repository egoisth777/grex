//! grex-core — manifest, lockfile, scheduler, pack model, plugin traits.
//!
//! M2 lands: manifest event log + lockfile + atomic file primitives.

#![forbid(unsafe_code)]

pub mod fs;
pub mod lockfile;
pub mod manifest;

pub mod pack {}
pub mod scheduler {}
pub mod plugin {}
