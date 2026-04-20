//! Pack-tree walker.
//!
//! Integrates the manifest parser (`pack`), the pluggable validator
//! framework (`pack::validate`), and the git backend (`git`) into a single
//! surface:
//!
//! 1. Load a root `pack.yaml` via a [`PackLoader`].
//! 2. For each `children:` entry, clone (or fetch + optionally checkout)
//!    via a [`crate::git::GitBackend`] and recurse into its manifest.
//! 3. Record `depends_on` references as graph edges (no walk).
//! 4. Return an immutable [`PackGraph`].
//!
//! Cycle detection runs during the walk; post-hoc graph validators live
//! under [`crate::pack::validate`] and run on the assembled graph.
//!
//! This module adds no new crate dependencies beyond those already pulled
//! in by slices 1–3.

pub mod error;
pub mod graph;
pub mod loader;
pub mod walker;

pub use error::TreeError;
pub use graph::{EdgeKind, PackEdge, PackGraph, PackNode};
pub use loader::{FsPackLoader, PackLoader};
pub use walker::Walker;
