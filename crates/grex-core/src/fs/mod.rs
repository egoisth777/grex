//! Filesystem primitives used by the manifest + lockfile layers.
//!
//! All OS-level file operations that the rest of `grex-core` depends on live
//! here so higher layers stay platform-agnostic.

pub mod atomic;
pub mod lock;

pub use atomic::atomic_write;
pub use lock::{ManifestLock, ScopedLock};
