//! Duplicate-`dst` detector for a single pack's `symlink` actions.
//!
//! Per pack-spec patch 3, two or more `symlink` actions within the same
//! pack whose `dst` literals collide are a **plan-phase error**. The
//! check is pre-expansion: we compare the authored string verbatim, not
//! the post-variable-expansion filesystem path. Any environment-variable
//! mismatch (e.g. `$HOME/a` vs `/home/user/a`) is a separate slice.
//!
//! # Emission policy (all-pairs)
//!
//! For `n` symlinks sharing the same `dst`, the validator emits
//! `n * (n - 1) / 2` errors — one per unordered pair. This surfaces the
//! full conflict graph to the CLI so an author renaming one entry can see
//! exactly which others it was colliding against, rather than replaying
//! after each fix. Indices are the flattened action-walk positions
//! produced by [`PackManifest::iter_all_symlinks`].

use std::collections::BTreeMap;

use super::{PackValidationError, Validator};
use crate::pack::PackManifest;

/// Flags duplicate literal `dst` strings across all symlink actions in a
/// pack (including those nested inside `when` blocks).
///
/// See the module docs for the all-pairs emission rationale.
pub struct DuplicateSymlinkValidator;

impl Validator for DuplicateSymlinkValidator {
    fn name(&self) -> &'static str {
        "duplicate_symlink_dst"
    }

    fn check(&self, pack: &PackManifest) -> Vec<PackValidationError> {
        // Bucket indices by dst literal. BTreeMap keeps emission order
        // deterministic on the dst key, which matters for snapshot tests
        // and reproducible CLI output.
        let mut by_dst: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
        for (idx, sym) in pack.iter_all_symlinks() {
            by_dst.entry(sym.dst.as_str()).or_default().push(idx);
        }

        let mut errs = Vec::new();
        for (dst, indices) in by_dst {
            if indices.len() < 2 {
                continue;
            }
            // All unordered pairs (i, j) with i < j — indices are already
            // in walk order so the pair is naturally ordered.
            for i in 0..indices.len() {
                for j in (i + 1)..indices.len() {
                    errs.push(PackValidationError::DuplicateSymlinkDst {
                        dst: dst.to_string(),
                        first: indices[i],
                        second: indices[j],
                    });
                }
            }
        }
        errs
    }
}
