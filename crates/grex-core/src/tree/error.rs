//! Error taxonomy for the [`crate::tree`] walker.
//!
//! Errors carry `PathBuf` and `String` detail fields rather than boxing
//! underlying loader or parser errors. Keeping leaky types out of the public
//! surface means adding a new loader backend (IPC, in-memory, http) in a
//! future slice stays non-breaking.

use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::git::GitError;

/// MSRV-safe ENOTDIR detection. `io::ErrorKind::NotADirectory` stabilised
/// in Rust 1.83 but the workspace MSRV is pinned at 1.79. Detect via the
/// raw OS error code instead: POSIX `ENOTDIR` = 20, Windows
/// `ERROR_DIRECTORY` = 267.
///
/// Used by the manifest loader to route OS-level "not a directory"
/// failures into [`TreeError::ManifestNotADir`] without requiring a MSRV
/// bump.
#[must_use]
pub fn is_not_a_directory(err: &io::Error) -> bool {
    match err.raw_os_error() {
        #[cfg(unix)]
        Some(20) => true,
        #[cfg(windows)]
        Some(267) => true,
        _ => false,
    }
}

/// Errors raised during a pack-tree walk.
///
/// Marked `#[non_exhaustive]` so later slices (credentials, submodules,
/// partial walks) can add variants without breaking consumers.
#[non_exhaustive]
#[derive(Debug, Error)]
pub enum TreeError {
    /// The walker expected a `pack.yaml` at the given location but could not
    /// find one (or its enclosing `.grex/` directory was missing).
    #[error("pack manifest not found at `{0}`")]
    ManifestNotFound(PathBuf),

    /// The manifest file existed but could not be read from disk.
    ///
    /// Catch-all fallback for `io::ErrorKind` cases that do not match a
    /// categorised variant. Prefer [`TreeError::ManifestPermissionDenied`],
    /// [`TreeError::ManifestNotADir`], or [`TreeError::ManifestIo`] when
    /// the producer can route via `io::Error::kind()` /
    /// [`is_not_a_directory`]. Retained for back-compat: v1.2.0+
    /// downstream consumers may have matched this variant explicitly.
    #[error("failed to read pack manifest: {0}")]
    ManifestRead(String),

    /// Manifest existed but the OS denied read access (POSIX `EACCES` /
    /// Windows `ERROR_ACCESS_DENIED`). Operator-actionable: chmod /
    /// icacls to grant the running user read on the file.
    #[error("permission denied reading pack manifest at `{path}`")]
    ManifestPermissionDenied {
        /// On-disk location of the unreadable manifest.
        path: PathBuf,
    },

    /// Manifest path resolved to a non-directory entry where a directory
    /// was expected (or the parent of the manifest path is not a
    /// directory). Distinct from [`TreeError::ManifestNotFound`] — the
    /// path exists but has the wrong type. Surfaces as `ENOTDIR` /
    /// `ERROR_DIRECTORY` on the producer side; detection routed through
    /// [`is_not_a_directory`] for MSRV 1.79 compatibility.
    #[error("manifest path `{path}` is not a directory (or has wrong type)")]
    ManifestNotADir {
        /// On-disk location whose type prevented manifest read.
        path: PathBuf,
    },

    /// Generic IO failure reading a manifest, preserving the underlying
    /// [`io::Error`] for log routing without forcing the caller to
    /// re-open the file. The catch-all path before the loader falls
    /// through to [`TreeError::ManifestRead`] for kinds that don't match
    /// a categorised variant.
    #[error("I/O error reading pack manifest at `{path}`: {source}")]
    ManifestIo {
        /// On-disk location of the manifest whose read failed.
        path: PathBuf,
        /// Underlying OS error preserved for the [`std::error::Error`]
        /// source chain.
        #[source]
        source: io::Error,
    },

    /// The manifest file was read but did not parse as a valid `pack.yaml`.
    #[error("failed to parse pack manifest at `{path}`: {detail}")]
    ManifestParse {
        /// On-disk location of the manifest that failed to parse.
        path: PathBuf,
        /// Backend-provided failure detail.
        detail: String,
    },

    /// A git operation (clone, fetch, checkout, …) failed while hydrating a
    /// child pack. The underlying [`GitError`] is preserved in full.
    #[error("git error during walk: {0}")]
    Git(#[from] GitError),

    /// A cycle was detected during the walk. `chain` lists the pack URLs (or
    /// paths for the root) from the outermost pack down to the recurrence.
    #[error("{}", display_cycle_detected(chain))]
    CycleDetected {
        /// Ordered chain of pack identities that forms the cycle.
        chain: Vec<String>,
    },

    /// A cloned child's `pack.yaml` declared a `name` that does not match
    /// what the parent pack expected for that `children:` entry.
    #[error("pack name `{got}` does not match expected `{expected}` for child at `{path}`")]
    PackNameMismatch {
        /// Name declared in the child's own manifest.
        got: String,
        /// Name the parent expected (derived from the child entry's
        /// effective path).
        expected: String,
        /// On-disk location of the offending child.
        path: PathBuf,
    },

    /// A `children[].path` (or URL-derived tail) violated the bare-name
    /// rule. Surfaced by the walker BEFORE any clone of the offending
    /// child fires so a malicious `path: ../escape` in a parent pack
    /// cannot materialise a directory outside the pack root. This is a
    /// security boundary, not a soft validation concern — see
    /// `crates/grex-core/src/pack/validate/child_path.rs` for the shared
    /// rejection logic.
    #[error("pack child `{child_name}` has invalid path `{path}`: {reason}")]
    ChildPathInvalid {
        /// Label of the offending child (the explicit `path:` value, or
        /// the URL-derived tail when `path:` is omitted).
        child_name: String,
        /// The rejected literal value.
        path: String,
        /// One-line explanation of which sub-rule failed.
        reason: String,
    },

    /// A v1.1.1-shape lockfile was encountered without the
    /// `--migrate-lockfile` opt-in. v1.2.0 changed the on-disk lockfile
    /// schema; the operator must explicitly run the migrator to convert
    /// pre-existing lockfiles. Emitted by Stage 1.h walker entry-point
    /// before any pack-tree work begins. Dormant until 1.h wires the
    /// detector.
    #[error("v1.1.1 lockfile detected at {path}, run grex migrate-lockfile")]
    LegacyLockfileDetected {
        /// On-disk location of the legacy lockfile.
        path: PathBuf,
    },

    /// One or more declared children own a `.git/` directory but lack a
    /// `.grex/pack.yaml`, and the v1.2.0 nested-children semantics
    /// preclude the v1.1.1 "synthesize plain-git pack" fallback (e.g.
    /// because a sibling explicitly opted out, or the parent manifest
    /// disabled synthesis). Aggregated by Stage 1.e Phase 1; the walker
    /// reports every offender in one go so the operator can fix the
    /// manifest with a single pass.
    #[error(
        "untracked git repositories found: {}; either register them as packs or remove from manifest",
        paths.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")
    )]
    UntrackedGitRepos {
        /// Paths (relative to the parent pack root) of every offender.
        paths: Vec<PathBuf>,
    },

    /// Stage 1.f Phase 2 prune refused to remove a destination because
    /// the recursive consent walk returned a non-Clean verdict.
    /// `kind` discriminates the specific safety violation so the CLI
    /// can suggest the correct override flag.
    #[error("{}", display_dirty_tree_refusal(path, kind))]
    DirtyTreeRefusal {
        /// Pack-tree-relative path of the destination the walker
        /// refused to prune.
        path: PathBuf,
        /// Specific consent violation that triggered the refusal.
        kind: DirtyTreeRefusalKind,
    },

    /// Stage 1.c validator rejected a child manifest segment that
    /// resolved outside the parent pack root. Distinct from
    /// [`TreeError::ChildPathInvalid`] — that variant rejects the
    /// literal `path:` syntax (slashes, dots, absolute paths);
    /// `ManifestPathEscape` is the post-resolution boundary check that
    /// catches symlink-driven and platform-specific escapes.
    #[error("manifest path '{path}' escapes parent boundary: {reason}")]
    ManifestPathEscape {
        /// The literal manifest path that resolved out of bounds.
        path: String,
        /// One-line explanation of which rule the resolved path
        /// violated.
        reason: String,
    },

    /// v1.3.1 (B4) — a child's resolved on-disk destination has no
    /// usable UTF-8 `file_name` component. Surfaces during dry-run
    /// record construction when `dest.file_name()` returns `None` (e.g.
    /// the path ends in `..` or is a filesystem root) or the component
    /// is not UTF-8. Recording the child with an empty `id` would
    /// silently corrupt the dry-run plan, so the walker pushes this
    /// error into [`SyncMetaReport::errors`] instead and continues.
    #[error("invalid destination path `{path}`: {reason}")]
    InvalidDestination {
        /// On-disk destination path that lacked a usable file_name.
        path: PathBuf,
        /// One-line explanation of which rule the path violated.
        reason: String,
    },
}

/// Discriminator for [`TreeError::DirtyTreeRefusal`]. Each kind has its
/// own operator-facing Display string; consult the variant docs for the
/// exact wording.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DirtyTreeRefusalKind {
    /// Working tree has tracked-modified or untracked-non-ignored
    /// content. Default refusal — operator must commit, stash, or
    /// remove the changes manually.
    DirtyTree,
    /// Working tree is clean of tracked changes but holds ignored
    /// files. Override available via `--force-prune-with-ignored`.
    DirtyTreeWithIgnored,
    /// `.git/` carries a rebase / merge / cherry-pick state directory.
    /// Operator must finish or abort the operation before pruning.
    GitInProgress,
    /// The destination is itself a meta-repo (sub-pack-tree) and the
    /// recursive consent walk found at least one of its descendants is
    /// dirty. Operator must clean the descendant first.
    SubMetaWithDirtyChildren,
}

/// Format a [`TreeError::CycleDetected`] message. Renders the chain
/// arrow-joined for operator legibility (`a → b → c → a`) instead of
/// the debug-vec rendering. Defensive on empty chains so a malformed
/// caller cannot panic the error path.
fn display_cycle_detected(chain: &[String]) -> String {
    if chain.is_empty() {
        return "cycle detected in pack graph (empty chain)".to_string();
    }
    format!("cycle detected in pack graph: {}", chain.join(" → "))
}

/// Format a [`TreeError::DirtyTreeRefusal`] message. Extracted so the
/// `#[error]` attribute can reference a function call instead of a
/// trailing match expression.
fn display_dirty_tree_refusal(path: &std::path::Path, kind: &DirtyTreeRefusalKind) -> String {
    match kind {
        DirtyTreeRefusalKind::DirtyTree => {
            format!("refusing to prune {}: working tree dirty", path.display())
        }
        DirtyTreeRefusalKind::DirtyTreeWithIgnored => format!(
            "refusing to prune {}: working tree dirty (including ignored files); use --force-prune-with-ignored to override",
            path.display()
        ),
        DirtyTreeRefusalKind::GitInProgress => format!(
            "refusing to prune {}: in-progress git operation (rebase/merge/cherry-pick)",
            path.display()
        ),
        DirtyTreeRefusalKind::SubMetaWithDirtyChildren => format!(
            "refusing to prune {}: nested meta-repo has dirty children",
            path.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    //! v1.2.0 Stage 1.k — error-variant Display assertions.
    //!
    //! Pure construction + `to_string()` checks. Variants are dormant
    //! until later stages (1.c validator, 1.e walker Phase 1, 1.f Phase 2
    //! prune-safety, 1.h migrator) wire them into producers.
    use super::*;

    #[test]
    fn test_tree_error_legacy_lockfile_detected_display() {
        let err = TreeError::LegacyLockfileDetected {
            path: PathBuf::from("/repos/code/.grex/lock.yaml"),
        };
        assert_eq!(
            err.to_string(),
            "v1.1.1 lockfile detected at /repos/code/.grex/lock.yaml, run grex migrate-lockfile",
        );
    }

    #[test]
    fn test_tree_error_untracked_git_repos_display_single() {
        let err = TreeError::UntrackedGitRepos { paths: vec![PathBuf::from("alpha")] };
        assert_eq!(
            err.to_string(),
            "untracked git repositories found: alpha; either register them as packs or remove from manifest",
        );
    }

    #[test]
    fn test_tree_error_untracked_git_repos_display_multiple() {
        let err = TreeError::UntrackedGitRepos {
            paths: vec![PathBuf::from("alpha"), PathBuf::from("beta"), PathBuf::from("gamma")],
        };
        assert_eq!(
            err.to_string(),
            "untracked git repositories found: alpha, beta, gamma; either register them as packs or remove from manifest",
        );
    }

    #[test]
    fn test_tree_error_dirty_tree_refusal_display_dirty_tree() {
        let err = TreeError::DirtyTreeRefusal {
            path: PathBuf::from("alpha"),
            kind: DirtyTreeRefusalKind::DirtyTree,
        };
        assert_eq!(err.to_string(), "refusing to prune alpha: working tree dirty");
    }

    #[test]
    fn test_tree_error_dirty_tree_refusal_display_dirty_with_ignored() {
        let err = TreeError::DirtyTreeRefusal {
            path: PathBuf::from("alpha"),
            kind: DirtyTreeRefusalKind::DirtyTreeWithIgnored,
        };
        assert_eq!(
            err.to_string(),
            "refusing to prune alpha: working tree dirty (including ignored files); use --force-prune-with-ignored to override",
        );
    }

    #[test]
    fn test_tree_error_dirty_tree_refusal_display_git_in_progress() {
        let err = TreeError::DirtyTreeRefusal {
            path: PathBuf::from("alpha"),
            kind: DirtyTreeRefusalKind::GitInProgress,
        };
        assert_eq!(
            err.to_string(),
            "refusing to prune alpha: in-progress git operation (rebase/merge/cherry-pick)",
        );
    }

    #[test]
    fn test_tree_error_dirty_tree_refusal_display_sub_meta_dirty() {
        let err = TreeError::DirtyTreeRefusal {
            path: PathBuf::from("alpha"),
            kind: DirtyTreeRefusalKind::SubMetaWithDirtyChildren,
        };
        assert_eq!(err.to_string(), "refusing to prune alpha: nested meta-repo has dirty children",);
    }

    #[test]
    fn test_tree_error_manifest_path_escape_display() {
        let err = TreeError::ManifestPathEscape {
            path: "../escape".into(),
            reason: "child path escapes parent root".into(),
        };
        assert_eq!(
            err.to_string(),
            "manifest path '../escape' escapes parent boundary: child path escapes parent root",
        );
    }

    #[test]
    fn test_tree_error_manifest_permission_denied_display() {
        let err = TreeError::ManifestPermissionDenied {
            path: PathBuf::from("/repos/code/.grex/pack.yaml"),
        };
        assert_eq!(
            err.to_string(),
            "permission denied reading pack manifest at `/repos/code/.grex/pack.yaml`",
        );
    }

    #[test]
    fn test_tree_error_manifest_not_a_dir_display() {
        let err = TreeError::ManifestNotADir { path: PathBuf::from("/repos/code/.grex/pack.yaml") };
        assert_eq!(
            err.to_string(),
            "manifest path `/repos/code/.grex/pack.yaml` is not a directory (or has wrong type)",
        );
    }

    #[test]
    fn test_tree_error_manifest_io_display_and_source() {
        use std::error::Error as _;

        let underlying = io::Error::other("disk on fire");
        let err = TreeError::ManifestIo {
            path: PathBuf::from("/repos/code/.grex/pack.yaml"),
            source: underlying,
        };
        assert_eq!(
            err.to_string(),
            "I/O error reading pack manifest at `/repos/code/.grex/pack.yaml`: disk on fire",
        );
        // The `#[source]` attribute MUST preserve the underlying io::Error
        // so consumers can walk the chain and recover the original kind.
        let source = err.source().expect("ManifestIo carries a source");
        let downcast = source.downcast_ref::<io::Error>().expect("source downcasts to io::Error");
        assert_eq!(downcast.kind(), io::ErrorKind::Other);
    }

    #[test]
    fn test_is_not_a_directory_helper_matches_platform_code() {
        // Platform-specific raw_os_error() codes for ENOTDIR. The helper
        // is the MSRV-1.79-safe substitute for io::ErrorKind::NotADirectory
        // (stabilised only in 1.83).
        #[cfg(unix)]
        {
            let e = io::Error::from_raw_os_error(20);
            assert!(is_not_a_directory(&e), "POSIX ENOTDIR (20) must be detected");
        }
        #[cfg(windows)]
        {
            let e = io::Error::from_raw_os_error(267);
            assert!(is_not_a_directory(&e), "Windows ERROR_DIRECTORY (267) must be detected");
        }
    }

    #[test]
    fn test_is_not_a_directory_helper_rejects_unrelated_codes() {
        // PermissionDenied and NotFound must not be misclassified as
        // ENOTDIR — the loader routes them to other variants.
        let perm = io::Error::from(io::ErrorKind::PermissionDenied);
        assert!(!is_not_a_directory(&perm));
        let nf = io::Error::from(io::ErrorKind::NotFound);
        assert!(!is_not_a_directory(&nf));
        // Synthetic io::Error without any raw_os_error must be rejected.
        let other = io::Error::other("no os code");
        assert!(!is_not_a_directory(&other));
    }
}
