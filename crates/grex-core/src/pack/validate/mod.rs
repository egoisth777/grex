//! Plan-phase validators for [`PackManifest`].
//!
//! Stage B of M3 introduces **plan-phase** validation — checks that run
//! after [`crate::pack::parse`] succeeds but before any execute-time work
//! (variable expansion, filesystem touches, child-pack traversal). The
//! validators here operate on the already-parsed manifest in its
//! pre-expansion, literal form.
//!
//! # Framework shape
//!
//! A [`Validator`] receives an immutable [`PackManifest`] and returns a
//! `Vec<PackValidationError>` — never fail-first. [`run_all`] composes the
//! default validator set and concatenates their findings so callers see
//! the full diagnostic set in one pass. This slice ships one validator
//! ([`DuplicateSymlinkValidator`]); subsequent M3 slices (cycle detect,
//! cross-pack conflict, `depends_on` verification) plug into the same
//! surface without touching orchestrator code.
//!
//! # Non-goals for this slice
//!
//! * No filesystem IO, no git, no platform probing.
//! * No variable expansion — validators compare literal `dst` strings.
//! * No cross-pack reasoning (later slices).

use thiserror::Error;

use super::PackManifest;
use crate::tree::PackGraph;

pub mod child_path;
pub mod cycle;
pub mod depends_on;
pub mod dup_symlink;

pub use child_path::{ChildPathValidator, DupChildPathValidator};
pub use cycle::CycleValidator;
pub use depends_on::DependsOnValidator;
pub use dup_symlink::DuplicateSymlinkValidator;

/// Errors raised by plan-phase validators.
///
/// Marked `#[non_exhaustive]` so future slices (slices 3–6) can add variants
/// without breaking downstream `match` arms.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PackValidationError {
    /// Two `symlink` actions within the same pack resolve to the same
    /// literal `dst` string. `first` and `second` are indices in the
    /// flattened action-walk order (see
    /// [`PackManifest::iter_all_symlinks`]).
    #[error("duplicate symlink dst `{dst}` (actions at indices {first} and {second})")]
    DuplicateSymlinkDst {
        /// Literal `dst` string (pre-expansion).
        dst: String,
        /// Global index of the earlier action.
        first: usize,
        /// Global index of the later action.
        second: usize,
    },

    /// A cycle was detected in the assembled pack graph. `chain` lists the
    /// pack names from the outermost node down to the recurrence.
    #[error("cycle detected in pack graph: {chain:?}")]
    GraphCycle {
        /// Ordered chain of pack names that forms the cycle.
        chain: Vec<String>,
    },

    /// A `depends_on` entry could not be resolved against any node in the
    /// walked graph.
    #[error("pack `{pack}` depends on `{required}` but no such pack exists in the graph")]
    DependsOnUnsatisfied {
        /// Name of the pack that declared the dependency.
        pack: String,
        /// The unresolved `depends_on` entry (a pack name or url).
        required: String,
    },

    /// A `children[].path` value violates the bare-name rule
    /// (`^[a-z][a-z0-9-]*$`, no separators, no `.` / `..`, no empty).
    /// Enforced since v1.1.0 — see
    /// [`child_path::ChildPathValidator`].
    #[error("pack child `{child_name}` has invalid path `{path}`: {reason}")]
    ChildPathInvalid {
        /// Label of the offending child (its `path` field, or `url` as
        /// fallback).
        child_name: String,
        /// The rejected literal `path` value.
        path: String,
        /// One-line explanation of which sub-rule failed.
        reason: String,
    },

    /// Two or more `children[]` entries within the same parent
    /// resolve to the same `effective_path()`. Without this gate the
    /// second clone would silently overwrite the first's working
    /// tree, or — once both have a `.git` — collide on the
    /// dest-already-exists fast path and skip-fetch the wrong upstream.
    /// Enforced since v1.1.0; see [`child_path::DupChildPathValidator`].
    #[error("pack has duplicate children resolving to `{path}`: {urls:?}")]
    ChildPathDuplicate {
        /// The shared resolved path that two or more children claim.
        path: String,
        /// URLs of every colliding child, in declaration order.
        urls: Vec<String>,
    },
}

/// A single plan-phase validator.
///
/// Implementations run against a fully parsed manifest and return every
/// problem they observe — never `Result`, because aggregation across
/// validators is the point.
pub trait Validator {
    /// Stable human-readable identifier for diagnostics / allowlisting.
    fn name(&self) -> &'static str;

    /// Inspect `pack` and emit zero or more errors.
    fn check(&self, pack: &PackManifest) -> Vec<PackValidationError>;
}

/// Run every default validator against `pack`, concatenating their
/// findings.
///
/// The current default set is:
///
/// 1. [`DuplicateSymlinkValidator`] — two symlinks with the same literal
///    `dst`.
/// 2. [`ChildPathValidator`] — every `children[].path` matches the
///    bare-name regex (since v1.1.0).
/// 3. [`DupChildPathValidator`] — no two `children[]` entries within the
///    same parent share an `effective_path()` (since v1.1.0).
///
/// Later slices extend this list; callers should prefer
/// [`PackManifest::validate_plan`] over instantiating validators manually,
/// so the default set stays discoverable.
#[must_use]
pub fn run_all(pack: &PackManifest) -> Vec<PackValidationError> {
    let validators: [&dyn Validator; 3] =
        [&DuplicateSymlinkValidator, &ChildPathValidator, &DupChildPathValidator];
    let mut errs = Vec::new();
    for v in validators {
        errs.extend(v.check(pack));
    }
    errs
}

/// Plan-phase validator that operates on an assembled [`PackGraph`].
///
/// Separate trait from [`Validator`] on purpose: graph-level checks need
/// the full graph, not a single manifest, and mixing the two into one
/// trait would force every per-manifest validator to accept a graph it
/// doesn't need. Two traits keep each call site's surface minimal and
/// type-safe.
pub trait GraphValidator {
    /// Stable human-readable identifier.
    fn name(&self) -> &'static str;

    /// Inspect `graph` and emit zero or more errors.
    fn check(&self, graph: &PackGraph) -> Vec<PackValidationError>;
}

/// Run every default [`GraphValidator`] against `graph`, concatenating
/// their findings.
///
/// Current default set:
///
/// 1. [`CycleValidator`] — belt-and-suspenders for cycles the walker
///    should have caught.
/// 2. [`DependsOnValidator`] — verify every `depends_on` entry resolves.
#[must_use]
pub fn run_all_graph(graph: &PackGraph) -> Vec<PackValidationError> {
    let validators: [&dyn GraphValidator; 2] = [&CycleValidator, &DependsOnValidator];
    let mut errs = Vec::new();
    for v in validators {
        errs.extend(v.check(graph));
    }
    errs
}

impl PackGraph {
    /// Run the default graph-validator set over `self`.
    ///
    /// Mirrors [`PackManifest::validate_plan`] at the graph surface. Kept
    /// here (rather than in `tree::graph`) so the `tree` module does not
    /// depend on `pack::validate`; the dependency direction stays
    /// `validate -> tree` only.
    ///
    /// # Errors
    ///
    /// Returns the aggregated error list when any graph validator
    /// flags a problem.
    pub fn validate(&self) -> Result<(), Vec<PackValidationError>> {
        let errs = run_all_graph(self);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}
