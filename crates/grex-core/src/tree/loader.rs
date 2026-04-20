//! Pluggable [`PackLoader`] for the tree walker.
//!
//! The walker never reads the filesystem directly: every manifest arrives via
//! a [`PackLoader`] impl. This exists so:
//!
//! * Tests substitute an in-memory mock and avoid disk I/O entirely.
//! * A future plugin backend (e.g. an HTTP-fetched manifest or a cached
//!   manifest store) can slot in without touching walker logic.
//! * Tree-walk tests stay hermetic on CI.

use std::path::{Path, PathBuf};

use crate::pack::{parse, PackManifest};

use super::error::TreeError;

/// Strategy object for turning a path into a parsed manifest.
///
/// Implementors must be `Send + Sync` so the walker can be used behind an
/// `Arc` in a future parallel-walk slice.
pub trait PackLoader: Send + Sync {
    /// Resolve `path` and return the parsed manifest.
    ///
    /// # Path semantics
    ///
    /// * If `path` is a directory, the loader looks up
    ///   `path.join(".grex/pack.yaml")`.
    /// * If `path` ends in `.yaml` or `.yml`, it is read verbatim.
    ///
    /// The distinction is documented at the trait level so every backend
    /// observes the same contract.
    ///
    /// # Errors
    ///
    /// Returns [`TreeError::ManifestNotFound`] when no manifest exists at the
    /// resolved location, [`TreeError::ManifestRead`] for IO failures, and
    /// [`TreeError::ManifestParse`] for structural failures.
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError>;
}

/// Filesystem-backed [`PackLoader`] used by the real walker.
#[derive(Debug, Default)]
pub struct FsPackLoader;

impl FsPackLoader {
    /// Construct a new loader. Equivalent to [`FsPackLoader::default`].
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl PackLoader for FsPackLoader {
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError> {
        let manifest_path = resolve_manifest_path(path);
        if !manifest_path.is_file() {
            return Err(TreeError::ManifestNotFound(manifest_path));
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .map_err(|e| TreeError::ManifestRead(format!("{}: {e}", manifest_path.display())))?;
        parse(&raw)
            .map_err(|e| TreeError::ManifestParse { path: manifest_path, detail: e.to_string() })
    }
}

/// Resolve a user-supplied path to the concrete `pack.yaml` location.
///
/// Split out so cyclomatic budget on [`FsPackLoader::load`] stays tiny.
fn resolve_manifest_path(path: &Path) -> PathBuf {
    if has_yaml_extension(path) {
        path.to_path_buf()
    } else {
        path.join(".grex").join("pack.yaml")
    }
}

fn has_yaml_extension(path: &Path) -> bool {
    matches!(path.extension().and_then(|e| e.to_str()), Some("yaml" | "yml"))
}
