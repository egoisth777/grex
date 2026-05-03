//! v1.3.1 — pure model-level lockfile writer.
//!
//! This module mirrors the Lean theorem
//! `Grex.Lockfile.lockfile_branch_mirrors_manifest_ref` (B14, v1.3.1
//! Rule-8 gate) at the Rust impl boundary. It exposes a single
//! pure-data-transform function, [`write_entry`], that maps a parent
//! manifest's [`ChildRef`] into a freshly-shaped [`LockEntry`] whose
//! `branch` field carries `child.r#ref` (the manifest `ref:` value)
//! verbatim — the contract that v1.3.0 violated by emitting
//! `branch: ""` unconditionally.
//!
//! # Why a separate `writer` module
//!
//! The actual call-site that has historically been the bug is in
//! `crates/grex-core/src/sync.rs::upsert_lock_entry` (sister worker
//! W2+W4 owns the plumbing change that threads `manifest_ref` from the
//! walker down to that call site). Co-locating the pure data-transform
//! here keeps three goals decoupled:
//!
//! 1. **Lean fidelity.** `write_entry` is a pure function whose body
//!    line-for-line matches `Grex.Lockfile.write_entry` in
//!    `proof/Grex/Lockfile.lean`. Reviewers / theorem-prover bridges
//!    can compare the two without traversing sync orchestration code.
//! 2. **Testability.** The fix's contract is testable with no
//!    walker, no fixtures, no IO — just a `ChildRef` in, a
//!    `LockEntry` out (see `crates/grex-core/tests/lockfile_branch_carry.rs`).
//! 3. **Worker scope.** The v1.3.1 W6 worker owns this file
//!    exclusively; W2+W4 (sync-side plumbing) consumes the function
//!    via the public re-export from
//!    [`crate::lockfile::write_entry_from_child`].
//!
//! # Lean ↔ Rust correspondence
//!
//! | Lean (`Grex.Lockfile`)       | Rust (`grex_core::lockfile::writer`) |
//! |------------------------------|--------------------------------------|
//! | `branchOf : Option String → String` | [`branch_of`]               |
//! | `write_entry : ChildRef → LockEntryV131` | [`write_entry`]         |
//! | `c.ref`                      | `child.r#ref`                        |
//! | `branchOf c.ref`             | `branch_of(child.r#ref.as_deref())`  |
//!
//! The Lean theorem (`lockfile_branch_mirrors_manifest_ref`) is a
//! pure-data-transform statement. The Rust impl realises it; no new
//! bridge axiom is required (see Lean module docstring).

use chrono::{DateTime, Utc};

use super::entry::LockEntry;
use crate::pack::ChildRef;

/// Lift `ChildRef.r#ref: Option<String>` into the `String` field of
/// `LockEntry.branch`. Mirrors the Lean `branchOf` function:
/// `None ↦ ""`, `Some s ↦ s`.
///
/// Centralising this rule (rather than inlining
/// `child.r#ref.clone().unwrap_or_default()` at every call site) keeps
/// the Lean ↔ Rust correspondence one symbol-pair wide, so future
/// changes to the empty-default convention land in exactly one place.
#[must_use]
pub fn branch_of(manifest_ref: Option<&str>) -> String {
    manifest_ref.map(str::to_string).unwrap_or_default()
}

/// Build a [`LockEntry`] from a parent-manifest [`ChildRef`] plus the
/// resolved-state fields the walker / sync orchestrator hands in.
///
/// Implements the v1.3.1 fix for B14: the entry's `branch` field is
/// taken from `child.r#ref` (via [`branch_of`]). When the manifest
/// omits `ref:`, the empty string is recorded — matching the Lean
/// model's `branchOf : Option String → String` (`None ↦ ""`).
///
/// `id` and `path` are both seeded from the child's effective on-disk
/// directory name (the parent-relative POSIX path under which the
/// child's working tree lives). Callers that mount the same id under a
/// distinct path (rare, but legal under the v1.2.0 distributed
/// lockfile schema) should overwrite [`LockEntry::path`] after
/// construction — the field is `pub` and not `#[non_exhaustive]`-gated
/// from in-crate sites.
///
/// `synthetic` defaults to `false`. Plain-git-child synthesis (v1.1.1)
/// is plumbed through by sister worker W2+W4 at the sync call site,
/// not here — keeping this function a pure transform.
///
/// # Lean correspondence
///
/// ```text
/// def write_entry (c : ChildRef) : LockEntryV131 :=
///   { id     := idOf c,
///     url    := c.url,
///     branch := branchOf c.ref }
/// ```
///
/// The Rust impl carries the same three fields plus the resolved-state
/// columns (`sha`, `installed_at`, `actions_hash`, `schema_version`)
/// that the on-disk schema requires; the Lean model is intentionally
/// narrower since B14's contract is about the `branch` slot only.
#[must_use]
pub fn write_entry(
    child: &ChildRef,
    sha: impl Into<String>,
    installed_at: DateTime<Utc>,
    actions_hash: impl Into<String>,
    schema_version: impl Into<String>,
) -> LockEntry {
    let path = child.effective_path();
    let id = path.clone();
    let mut entry = LockEntry::new(
        id,
        sha,
        branch_of(child.r#ref.as_deref()),
        installed_at,
        actions_hash,
        schema_version,
    );
    entry.path = path;
    entry
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 5, 2, 10, 0, 0).unwrap()
    }

    #[test]
    fn branch_of_none_is_empty() {
        assert_eq!(branch_of(None), "");
    }

    #[test]
    fn branch_of_some_is_value() {
        assert_eq!(branch_of(Some("main")), "main");
        assert_eq!(branch_of(Some("")), "");
    }

    /// AC: a manifest child with `ref: main` produces an entry whose
    /// `branch` field is `"main"` — the v1.3.1 fix for B14.
    /// Construction goes through `pack::parse` because `ChildRef` is
    /// `#[non_exhaustive]` and would reject struct-literal use from
    /// integration-test crates; using the same path here keeps the
    /// in-crate test consistent with the integration test in
    /// `tests/lockfile_branch_carry.rs`.
    #[test]
    fn write_entry_carries_branch_ref() {
        let yaml = "schema_version: \"1\"\n\
                    name: parent\n\
                    type: meta\n\
                    children:\n\
                    \x20\x20- url: https://example.invalid/alpha.git\n\
                    \x20\x20\x20\x20ref: main\n";
        let manifest = crate::pack::parse(yaml).expect("fixture parses");
        let child = &manifest.children[0];
        let entry = write_entry(child, "deadbeef", ts(), "h", "1");
        assert_eq!(entry.branch, "main");
        assert_eq!(entry.id, "alpha");
        assert_eq!(entry.path, "alpha");
    }

    /// AC: a manifest child without `ref:` produces an entry with
    /// `branch: ""` — preserving the documented empty-default rule
    /// (Lean: `branchOf None = ""`).
    #[test]
    fn write_entry_missing_ref_yields_empty_branch() {
        let yaml = "schema_version: \"1\"\n\
                    name: parent\n\
                    type: meta\n\
                    children:\n\
                    \x20\x20- url: https://example.invalid/beta.git\n";
        let manifest = crate::pack::parse(yaml).expect("fixture parses");
        let child = &manifest.children[0];
        let entry = write_entry(child, "abc", ts(), "h", "1");
        assert_eq!(entry.branch, "");
    }
}
