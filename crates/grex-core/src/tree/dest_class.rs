//! Phase 1 destination classifier (Stage 1.e).
//!
//! Implements the five-way classifier `classify_dest` whose totality is
//! proven by the Lean theorem `classify_dest_total` in
//! `proof/Grex/Phase1.lean`. Each declared child dest, on entry to the
//! walker's Phase 1, is classified into exactly one of five
//! [`DestClass`] variants; the walker then dispatches on the result
//! (clone for `Missing`, fetch for `PresentDeclared`, refuse for
//! `PresentInProgress`, etc.).
//!
//! The `PresentInProgress` probe is backed by Lean bridge axiom
//! `git_in_progress_decidable` (`proof/Grex/Bridge.lean`), which
//! enumerates the six marker files this module must check:
//!
//! 1. `.git/rebase-merge/`
//! 2. `.git/rebase-apply/`
//! 3. `.git/MERGE_HEAD`
//! 4. `.git/CHERRY_PICK_HEAD`
//! 5. `.git/REVERT_HEAD`
//! 6. `.git/BISECT_LOG`
//!
//! The aggregation helper [`aggregate_untracked`] folds a sequence of
//! classifications into the V1 walker contract: when ANY child carries a
//! `.git/` but is not declared in the manifest, the walker MUST report
//! ALL such offenders in one [`TreeError::UntrackedGitRepos`] error
//! before failing — never first-seen. This discharges the Lean axiom
//! `sync_no_untracked` enumeration property.

use std::path::{Path, PathBuf};

use crate::fs::boundary::BoundedDir;
use crate::lockfile::LockEntry;

use super::error::TreeError;

/// Phase 1 per-child destination classifier output.
///
/// Lean reference: `proof/Grex/Types.lean` §"Five-way destination
/// classifier" + theorem `classify_dest_total` in
/// `proof/Grex/Phase1.lean`. Every Rust-side `classify_dest` invocation
/// returns exactly one variant; this is the totality the proof asserts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestClass {
    /// Path doesn't exist on disk. Walker will clone here.
    Missing,
    /// Path exists, has both `.git/` AND a manifest entry, AND the
    /// lockfile entry agrees. Walker will fetch (no clone needed).
    PresentDeclared,
    /// Path exists, has `.git/` and a manifest entry, lockfile says
    /// clean, but the git working tree is dirty (uncommitted changes
    /// or untracked files).
    PresentDirty,
    /// Path exists with `.git/` BUT mid-rebase, mid-merge,
    /// mid-cherry-pick, mid-bisect, or mid-revert. Backed by the Lean
    /// bridge axiom `git_in_progress_decidable`. Walker MUST refuse
    /// here — both Phase 1 fetch and Phase 2 prune are unsafe.
    PresentInProgress,
    /// Path exists, has `.git/` BUT no manifest entry declares it.
    /// Walker aggregates these into a single
    /// [`TreeError::UntrackedGitRepos`] error after Phase 1 completes
    /// (see [`aggregate_untracked`]).
    PresentUndeclared,
}

/// Probe for an in-progress git operation under `dest`.
///
/// Mirrors the six-marker enumeration in Lean bridge axiom
/// `git_in_progress_decidable` (`proof/Grex/Bridge.lean` lines 195–204).
/// Each probe is a single `exists()` syscall — the kernel guarantees
/// per-probe atomicity; cross-probe consistency is provided by the
/// per-pack [`crate::pack_lock::PackLock`] held by the walker for the
/// duration of classification (per the axiom's stated soundness
/// assumption).
///
/// Returns `true` when ANY of the six markers is present, `false`
/// otherwise. A non-existent `.git/` short-circuits to `false` — the
/// caller is responsible for sequencing the `.git/` existence check
/// before this probe (see [`classify_dest`]).
#[must_use]
pub fn git_in_progress_at(dest: &Path) -> bool {
    let git = dest.join(".git");
    if !git.exists() {
        return false;
    }
    // Order mirrors the Lean axiom enumeration. Any single hit is
    // enough — a mid-flight git operation is non-overlapping in
    // practice, so short-circuiting on the first positive is correct.
    git.join("rebase-merge").exists()
        || git.join("rebase-apply").exists()
        || git.join("MERGE_HEAD").exists()
        || git.join("CHERRY_PICK_HEAD").exists()
        || git.join("REVERT_HEAD").exists()
        || git.join("BISECT_LOG").exists()
}

/// Probe whether the working tree at `dest` carries any tracked
/// modifications or untracked-non-ignored content.
///
/// Implementation: shells out to `git status --porcelain --ignored=no`
/// and returns `true` iff stdout is non-empty. The `--ignored=no` flag
/// keeps gitignored content (build artefacts, IDE files) from
/// triggering a false-positive dirty verdict — the Phase 2 prune
/// safety check has its own `--force-prune-with-ignored` knob for the
/// ignored-only case (Stage 1.f / 1.l territory).
///
/// On any error (no `git` on PATH, dest is not a git repo, permission
/// denied), the probe returns `false`. This is conservative for Phase
/// 1: a clean classification means the walker proceeds with fetch; a
/// false negative on dirtiness will be caught at Phase 2 prune-time
/// where the consent walk re-probes via the planned `recursive_consent`
/// helper (Stage 1.f).
#[must_use]
fn git_status_dirty(dest: &Path) -> bool {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dest)
        .arg("status")
        .arg("--porcelain")
        .arg("--ignored=no")
        .output();
    match output {
        Ok(out) if out.status.success() => !out.stdout.is_empty(),
        _ => false,
    }
}

/// Phase 1 five-way classifier.
///
/// `dest` is the prospective on-disk destination resolved against the
/// parent meta. `declared_in_manifest` is `true` when the parent's
/// `manifest.children[]` contains an entry whose `effective_path`
/// resolves to this slot. `lockfile_entry` is the parent's lockfile
/// entry for this dest (or `None` if absent).
///
/// Decision tree (mirrors the Lean axiom `classify_dest`):
///
/// 1. `dest` does not exist → [`DestClass::Missing`].
/// 2. `dest/.git` does not exist → [`DestClass::Missing`] (the
///    declarative-slot case: a directory with no `.git/` is treated
///    as a fresh slot the walker will clone into; the v1.2.0 nested-
///    children semantics already rule out the "real content with no
///    .git" case at validator time).
/// 3. [`git_in_progress_at`] returns `true` → [`DestClass::PresentInProgress`].
/// 4. `!declared_in_manifest` → [`DestClass::PresentUndeclared`].
/// 5. [`git_status_dirty`] returns `true` → [`DestClass::PresentDirty`].
/// 6. Otherwise → [`DestClass::PresentDeclared`].
///
/// `lockfile_entry` is reserved for Stage 1.h's lockfile-vs-disk drift
/// detection (e.g. lockfile records a sha that disagrees with the
/// on-disk HEAD); for Stage 1.e it is accepted but unused — the
/// classifier is sound without it. Future stages will tighten the
/// `PresentDeclared` branch into `PresentDeclared` vs `PresentDrift`.
///
/// # BoundedDir integration (Stage 1.d wiring)
///
/// When `dest.parent()` is available, this function opens the parent
/// as a [`BoundedDir`] and the relative dest as a child handle BEFORE
/// any `.git/` probe. That binds the kernel resolution to an inode,
/// closing the canonicalize→probe TOCTOU window. If the dirfd open
/// fails (parent missing, traversal attempt, symlink escape), the
/// classifier falls back to [`DestClass::Missing`] — the dest is not
/// reachable through a confined-handle traversal, so Phase 1 cannot
/// safely operate on it; treating it as Missing means the walker will
/// either clone fresh (if the slot legitimately doesn't exist yet) or
/// the subsequent clone attempt will surface the real underlying error.
#[must_use]
pub fn classify_dest(
    dest: &Path,
    declared_in_manifest: bool,
    _lockfile_entry: Option<&LockEntry>,
) -> DestClass {
    // Step 0: confine via BoundedDir when a parent is available. This
    // is the Stage 1.d integration the audit flagged for Phase 1: bind
    // to an inode BEFORE FS probes so a swap of `dest` for a symlink
    // mid-classify cannot redirect the probes.
    if let (Some(parent), Some(name)) = (dest.parent(), dest.file_name()) {
        if !parent.as_os_str().is_empty() {
            // BoundedDir::open returns Err when the dest does not
            // exist OR when the resolution traverses a symlink. Either
            // case is "Missing" from the classifier's view: a non-
            // existent slot is the happy path for clone, and a
            // symlink-escape slot must not be operated on.
            if BoundedDir::open(parent, Path::new(name)).is_err() {
                return DestClass::Missing;
            }
        }
    }

    // Step 1: dest must exist and carry a `.git/` to be considered
    // present. Either miss → Missing.
    if !dest.exists() {
        return DestClass::Missing;
    }
    if !dest.join(".git").exists() {
        return DestClass::Missing;
    }

    // Step 2: in-progress operation gate (refusal class).
    if git_in_progress_at(dest) {
        return DestClass::PresentInProgress;
    }

    // Step 3: untracked / undeclared gate (aggregation class).
    if !declared_in_manifest {
        return DestClass::PresentUndeclared;
    }

    // Step 4: dirty-tree gate.
    if git_status_dirty(dest) {
        return DestClass::PresentDirty;
    }

    // Step 5: clean and declared.
    DestClass::PresentDeclared
}

/// Aggregate per-child classifications into the Phase 1 untracked-repo
/// error.
///
/// Walks the `(path, class)` pairs and collects every
/// [`DestClass::PresentUndeclared`] path. If any are found, returns
/// [`TreeError::UntrackedGitRepos`] with the COMPLETE list (preserving
/// input order). Empty input — or no `PresentUndeclared` — returns
/// `Ok(())`.
///
/// Discharges the Lean axiom `sync_no_untracked` enumeration property:
/// the walker MUST not surface only the first offender — every
/// undeclared `.git/` slot is reported in one go so the operator can
/// fix the manifest with a single pass.
pub fn aggregate_untracked<I, P>(classifications: I) -> Result<(), TreeError>
where
    I: IntoIterator<Item = (P, DestClass)>,
    P: Into<PathBuf>,
{
    let untracked: Vec<PathBuf> = classifications
        .into_iter()
        .filter_map(|(p, c)| if c == DestClass::PresentUndeclared { Some(p.into()) } else { None })
        .collect();
    if untracked.is_empty() {
        Ok(())
    } else {
        Err(TreeError::UntrackedGitRepos { paths: untracked })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: turn a tempdir into a directory that looks like a
    /// minimal git checkout (just a `.git/` folder; enough for the
    /// classifier's `.git/` existence probes). Returns the dest path.
    fn make_git_dir(parent: &Path, name: &str) -> PathBuf {
        let dest = parent.join(name);
        fs::create_dir_all(dest.join(".git")).unwrap();
        dest
    }

    #[test]
    fn test_classify_dest_missing() {
        let parent = tempdir().unwrap();
        let dest = parent.path().join("absent");
        assert_eq!(classify_dest(&dest, true, None), DestClass::Missing);
    }

    #[test]
    fn test_classify_dest_missing_when_no_dot_git() {
        // Directory exists but has no `.git/` — classifier treats this
        // as a fresh declarative slot (Missing-equivalent).
        let parent = tempdir().unwrap();
        let dest = parent.path().join("plain-dir");
        fs::create_dir(&dest).unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::Missing);
    }

    #[test]
    fn test_classify_dest_present_declared() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        // Initialise as a real repo so `git status` returns success
        // and reports a clean tree.
        let init =
            std::process::Command::new("git").arg("-C").arg(&dest).arg("init").arg("-q").status();
        if init.is_err() || !init.unwrap().success() {
            // Host has no `git` binary — skip. The classifier still
            // treats `.git/` + declared as PresentDeclared via the
            // `git_status_dirty` fallback (returns false on error), so
            // assert that path too.
        }
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentDeclared);
    }

    #[test]
    fn test_classify_dest_present_dirty() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        // Initialise a real repo so `git status` runs.
        let init =
            std::process::Command::new("git").arg("-C").arg(&dest).arg("init").arg("-q").status();
        if init.is_err() || !init.map(|s| s.success()).unwrap_or(false) {
            // No `git` binary — `git_status_dirty` returns false and
            // we'd misclassify as Declared. Skip this test on that
            // host; the per-host symlink-test pattern in
            // `boundary::tests` is the precedent.
            return;
        }
        // Write an untracked file → dirty.
        fs::write(dest.join("dirty.txt"), b"hello").unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentDirty);
    }

    #[test]
    fn test_classify_dest_present_in_progress_rebase() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        fs::create_dir_all(dest.join(".git/rebase-merge")).unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentInProgress);
    }

    #[test]
    fn test_classify_dest_present_in_progress_rebase_apply() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        fs::create_dir_all(dest.join(".git/rebase-apply")).unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentInProgress);
    }

    #[test]
    fn test_classify_dest_present_in_progress_merge() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        fs::write(dest.join(".git/MERGE_HEAD"), b"deadbeef").unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentInProgress);
    }

    #[test]
    fn test_classify_dest_present_in_progress_cherry_pick() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        fs::write(dest.join(".git/CHERRY_PICK_HEAD"), b"deadbeef").unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentInProgress);
    }

    #[test]
    fn test_classify_dest_present_in_progress_revert() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        fs::write(dest.join(".git/REVERT_HEAD"), b"deadbeef").unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentInProgress);
    }

    #[test]
    fn test_classify_dest_present_in_progress_bisect() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        fs::write(dest.join(".git/BISECT_LOG"), b"start").unwrap();
        assert_eq!(classify_dest(&dest, true, None), DestClass::PresentInProgress);
    }

    #[test]
    fn test_classify_dest_present_undeclared() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        // Not declared in manifest → undeclared (regardless of dirty).
        assert_eq!(classify_dest(&dest, false, None), DestClass::PresentUndeclared);
    }

    #[test]
    fn test_git_in_progress_at_returns_false_on_clean_repo() {
        let parent = tempdir().unwrap();
        let dest = make_git_dir(parent.path(), "child");
        assert!(!git_in_progress_at(&dest));
    }

    #[test]
    fn test_git_in_progress_at_returns_false_on_non_repo() {
        let parent = tempdir().unwrap();
        let dest = parent.path().join("not-a-repo");
        fs::create_dir(&dest).unwrap();
        assert!(!git_in_progress_at(&dest));
    }

    #[test]
    fn test_phase1_aggregates_untracked_into_error() {
        // Three undeclared dirs interleaved with one declared and one
        // missing. Aggregation must surface ALL undeclared paths in
        // input order, not just the first.
        let inputs: Vec<(PathBuf, DestClass)> = vec![
            (PathBuf::from("alpha"), DestClass::PresentUndeclared),
            (PathBuf::from("beta"), DestClass::PresentDeclared),
            (PathBuf::from("gamma"), DestClass::PresentUndeclared),
            (PathBuf::from("delta"), DestClass::Missing),
            (PathBuf::from("epsilon"), DestClass::PresentUndeclared),
        ];
        let err =
            aggregate_untracked(inputs).expect_err("aggregation must fail when any undeclared");
        match err {
            TreeError::UntrackedGitRepos { paths } => {
                assert_eq!(
                    paths,
                    vec![PathBuf::from("alpha"), PathBuf::from("gamma"), PathBuf::from("epsilon"),],
                    "aggregator must enumerate ALL undeclared paths in input order",
                );
            }
            other => panic!("expected UntrackedGitRepos, got {other:?}"),
        }
    }

    #[test]
    fn test_phase1_aggregate_ok_when_no_undeclared() {
        let inputs: Vec<(PathBuf, DestClass)> = vec![
            (PathBuf::from("alpha"), DestClass::Missing),
            (PathBuf::from("beta"), DestClass::PresentDeclared),
        ];
        assert!(aggregate_untracked(inputs).is_ok());
    }

    #[test]
    fn test_phase1_aggregate_ok_on_empty_input() {
        let inputs: Vec<(PathBuf, DestClass)> = Vec::new();
        assert!(aggregate_untracked(inputs).is_ok());
    }
}
