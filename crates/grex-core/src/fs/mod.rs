//! Filesystem primitives used by the manifest + lockfile layers.
//!
//! All OS-level file operations that the rest of `grex-core` depends on live
//! here so higher layers stay platform-agnostic.

pub mod atomic;
pub mod gitignore;
pub mod lock;

pub use atomic::atomic_write;
pub use gitignore::{
    read_managed_block, remove_managed_block, upsert_managed_block, GitignoreError,
};
pub use lock::{ManifestLock, ScopedLock};
