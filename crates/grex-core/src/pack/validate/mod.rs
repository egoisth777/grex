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

pub mod dup_symlink;

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
///
/// Later slices extend this list; callers should prefer
/// [`PackManifest::validate_plan`] over instantiating validators manually,
/// so the default set stays discoverable.
#[must_use]
pub fn run_all(pack: &PackManifest) -> Vec<PackValidationError> {
    let validators: [&dyn Validator; 1] = [&DuplicateSymlinkValidator];
    let mut errs = Vec::new();
    for v in validators {
        errs.extend(v.check(pack));
    }
    errs
}
