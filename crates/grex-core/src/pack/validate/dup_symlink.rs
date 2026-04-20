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
//!
//! # Platform-aware collision key
//!
//! The bucket key is platform-folded before comparison:
//!
//! * **Windows** and **macOS** — ASCII-lowercased so `FOO` and `foo`
//!   collide (NTFS is case-insensitive by default; APFS and HFS+ on
//!   macOS default to case-insensitive too).
//! * **Other Unix** — byte-exact, matching typical case-sensitive
//!   filesystems.
//!
//! Full Unicode case-folding is not applied; the overhead is not
//! justified for the rare pack `dst` that relies on non-ASCII casing.
//! APFS can be reformatted case-sensitive; this validator stays
//! pessimistic in that rare configuration (it may flag a non-collision
//! the filesystem would actually accept). Probing filesystem
//! case-sensitivity is a future enhancement. The error message carries
//! the **original** authored `dst`, not the folded form.

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
        // Bucket (canonicalised_key, first_original_dst, indices) by the
        // platform-folded key. BTreeMap keeps emission order deterministic
        // on the folded key, which matters for snapshot tests and
        // reproducible CLI output. The stored original dst comes from the
        // first symlink in walk order so error messages echo what the
        // author actually wrote.
        let mut by_dst: BTreeMap<String, (&str, Vec<usize>)> = BTreeMap::new();
        for (idx, sym) in pack.iter_all_symlinks() {
            let key = canonical_dst(sym.dst.as_str());
            by_dst.entry(key).or_insert_with(|| (sym.dst.as_str(), Vec::new())).1.push(idx);
        }

        let mut errs = Vec::new();
        for (_key, (original_dst, indices)) in by_dst {
            if indices.len() < 2 {
                continue;
            }
            // All unordered pairs (i, j) with i < j — indices are already
            // in walk order so the pair is naturally ordered.
            for i in 0..indices.len() {
                for j in (i + 1)..indices.len() {
                    errs.push(PackValidationError::DuplicateSymlinkDst {
                        dst: original_dst.to_string(),
                        first: indices[i],
                        second: indices[j],
                    });
                }
            }
        }
        errs
    }
}

/// Canonicalise a `dst` literal for collision bucketing.
///
/// Case-folds on Windows and macOS (whose default filesystems are
/// case-insensitive), and passes bytes through unchanged elsewhere. See
/// the module docs for the full rationale and caveats.
fn canonical_dst(dst: &str) -> String {
    #[cfg(any(windows, target_os = "macos"))]
    {
        dst.to_ascii_lowercase()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        dst.to_string()
    }
}
