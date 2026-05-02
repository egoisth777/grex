//! Error taxonomy for the [`crate::tree`] walker.
//!
//! Errors carry `PathBuf` and `String` detail fields rather than boxing
//! underlying loader or parser errors. Keeping leaky types out of the public
//! surface means adding a new loader backend (IPC, in-memory, http) in a
//! future slice stays non-breaking.

use std::path::PathBuf;

use thiserror::Error;

use crate::git::GitError;

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
    #[error("failed to read pack manifest: {0}")]
    ManifestRead(String),

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
}
