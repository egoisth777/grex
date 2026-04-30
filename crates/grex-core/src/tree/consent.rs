//! Phase 2 prune-safety + recursive consent walk (Stage 1.f).
//!
//! Implements [`recursive_consent_walk`] and [`phase2_prune`] which together
//! discharge the Lean theorem `prune_only_on_clean_consent`
//! (`proof/Grex/Consent.lean`) via the bridge axiom
//! `consent_walk_reflects_fs_state` (`proof/Grex/Bridge.lean` lines
//! 270–308). Phase 2 deletes a destination directory ONLY when the
//! recursive consent walk returns [`ConsentResult::Clean`].
//!
//! # Decision matrix (mirrors Lean axiom)
//!
//! | Walk result                  | `force_prune` | `force_prune_with_ignored` | Action  |
//! |------------------------------|---------------|----------------------------|---------|
//! | `Clean`                      | any           | any                        | execute |
//! | `DirtyTree`                  | `true`        | any                        | execute |
//! | `DirtyTree`                  | `false`       | `true`                     | execute |
//! | `DirtyTree`                  | `false`       | `false`                    | refuse  |
//! | `DirtyTreeWithIgnored`       | any           | `true`                     | execute |
//! | `DirtyTreeWithIgnored`       | any           | `false`                    | refuse  |
//! | `GitInProgress`              | any           | any                        | refuse  |
//! | `SubMetaWithDirtyChildren`   | `true`        | any                        | execute |
//! | `SubMetaWithDirtyChildren`   | `false`       | `true`                     | execute |
//! | `SubMetaWithDirtyChildren`   | `false`       | `false`                    | refuse  |
//!
//! `GitInProgress` is NEVER overridable — the operator must complete or
//! abort the rebase / merge / cherry-pick / bisect / revert before any
//! prune runs. This matches the Lean axiom enumeration of safe-prune
//! preconditions: a mid-flight git operation is by definition not a
//! safe state from which to wipe the working tree.
//!
//! # Confinement
//!
//! Prune execution opens the dest's PARENT directory as a
//! [`BoundedDir`] (Stage 1.d primitive) and removes the child by
//! relative name. cap-std refuses to follow symlinks across the parent
//! boundary, so a hostile pre-existing symlink at the dest slot cannot
//! redirect the deletion outside the workspace.

use std::path::{Path, PathBuf};

use crate::fs::boundary::BoundedDir;
use crate::manifest::event::Event;

use super::dest_class::git_in_progress_at;
use super::error::{DirtyTreeRefusalKind, TreeError};

/// Output of the Phase 2 recursive consent probe. Mirrors the Lean
/// `ConsentResult` enum (`proof/Grex/Types.lean` lines 380–399). Five
/// kinds are mutually exclusive: pruning proceeds iff the walk returns
/// [`ConsentResult::Clean`]; otherwise the lockfile and filesystem at
/// the dest are left untouched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsentResult {
    /// All probes passed; the dest may be pruned.
    Clean,
    /// Working tree at the dest (or any descendant) is dirty per
    /// `git status --porcelain` (tracked or untracked-non-ignored
    /// changes).
    DirtyTree,
    /// Working tree dirty *only* in `--ignored` paths (build
    /// artefacts, `target/`, `node_modules/`). Distinguished from
    /// [`ConsentResult::DirtyTree`] because `--force-prune-with-ignored`
    /// may consume this where it would NOT consume a tracked-file
    /// dirty tree.
    DirtyTreeWithIgnored,
    /// A git operation is mid-flight at the dest or a descendant
    /// (`.git/rebase-merge`, `.git/MERGE_HEAD`, `.git/CHERRY_PICK_HEAD`,
    /// `.git/REVERT_HEAD`, `.git/BISECT_LOG`). Always refused; never
    /// overridable.
    GitInProgress,
    /// The dest is itself a sub-meta and at least one of its
    /// descendants is dirty / in-progress. The sub-meta cannot be
    /// pruned without violating its own children's autonomy.
    SubMetaWithDirtyChildren,
}

impl ConsentResult {
    /// Map a [`ConsentResult`] to its corresponding
    /// [`DirtyTreeRefusalKind`]. `Clean` has no mapping — refusal kinds
    /// only apply when the walk did not return Clean.
    fn refusal_kind(self) -> Option<DirtyTreeRefusalKind> {
        match self {
            ConsentResult::Clean => None,
            ConsentResult::DirtyTree => Some(DirtyTreeRefusalKind::DirtyTree),
            ConsentResult::DirtyTreeWithIgnored => Some(DirtyTreeRefusalKind::DirtyTreeWithIgnored),
            ConsentResult::GitInProgress => Some(DirtyTreeRefusalKind::GitInProgress),
            ConsentResult::SubMetaWithDirtyChildren => {
                Some(DirtyTreeRefusalKind::SubMetaWithDirtyChildren)
            }
        }
    }
}

/// Probe `git status --porcelain` for the dest. Returns one of:
/// * `Some(false)` — the working tree carries tracked or
///   untracked-non-ignored changes.
/// * `Some(true)`  — the only dirtiness is in `--ignored` paths.
/// * `None`        — the working tree is clean (or the probe was not
///   applicable, e.g. dest is not a git repo / no `git` on PATH).
///
/// "Applicable" is decided by exit status: a non-success `git status`
/// is treated as "not a git repo" and surfaces as `None`. The caller
/// (the recursive walk) sequences this AFTER an `.git/` existence
/// check.
fn classify_status(dest: &Path) -> StatusVerdict {
    let porcelain = std::process::Command::new("git")
        .arg("-C")
        .arg(dest)
        .arg("status")
        .arg("--porcelain")
        .arg("--ignored=no")
        .output();
    let porcelain_dirty = matches!(
        porcelain,
        Ok(ref out) if out.status.success() && !out.stdout.is_empty()
    );
    if porcelain_dirty {
        return StatusVerdict::DirtyTree;
    }
    let with_ignored = std::process::Command::new("git")
        .arg("-C")
        .arg(dest)
        .arg("status")
        .arg("--porcelain")
        .arg("--ignored")
        .output();
    let ignored_dirty = matches!(
        with_ignored,
        Ok(ref out) if out.status.success() && !out.stdout.is_empty()
    );
    if ignored_dirty {
        StatusVerdict::DirtyTreeWithIgnored
    } else {
        StatusVerdict::Clean
    }
}

/// Internal status-probe verdict for one git directory. Keeps the
/// caller logic simple: just match three cases instead of two
/// stacked `Option<bool>` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StatusVerdict {
    Clean,
    DirtyTree,
    DirtyTreeWithIgnored,
}

/// Walk `dir` and every nested git working tree beneath it, returning
/// the first non-`Clean` verdict found.
///
/// # Order of precedence
///
/// 1. [`git_in_progress_at`] is checked FIRST at each git dir. An
///    in-progress operation always wins; even a tracked-dirty tree on
///    top of a mid-rebase surfaces as [`ConsentResult::GitInProgress`]
///    so the override flags cannot wipe state mid-flight.
/// 2. [`classify_status`] decides between
///    [`ConsentResult::DirtyTree`] and
///    [`ConsentResult::DirtyTreeWithIgnored`].
/// 3. Sub-directories are recursed into. If `dir` is itself a git
///    working tree (a meta) AND any descendant returns non-Clean, the
///    walk surfaces [`ConsentResult::SubMetaWithDirtyChildren`] (the
///    descendant's verdict is hoisted into the sub-meta verdict).
///
/// `dir` does not have to be a git repository. A plain directory with
/// no `.git/` returns Clean unless one of its descendants is itself a
/// git repo with a non-Clean verdict.
///
/// Errors during directory traversal (permission denied, gone) are
/// treated as Clean — the prune logic re-checks via cap-std before
/// any actual deletion, so a lossy traversal cannot escalate into a
/// false "execute" decision: cap-std would refuse the open.
#[must_use]
pub fn recursive_consent_walk(dir: &Path) -> ConsentResult {
    walk_inner(dir, /* root */ true)
}

fn walk_inner(dir: &Path, root: bool) -> ConsentResult {
    let dir_is_meta = dir.join(".git").exists();

    // 1. If `dir` is itself a git repo, run the in-progress probe
    //    UNCONDITIONALLY first. GitInProgress always wins over any
    //    other verdict, anywhere in the tree.
    if dir_is_meta && git_in_progress_at(dir) {
        return ConsentResult::GitInProgress;
    }

    // 2. Classify `dir`'s own working tree, if it is one.
    let self_verdict = if dir_is_meta {
        match classify_status(dir) {
            StatusVerdict::Clean => ConsentResult::Clean,
            StatusVerdict::DirtyTree => ConsentResult::DirtyTree,
            StatusVerdict::DirtyTreeWithIgnored => ConsentResult::DirtyTreeWithIgnored,
        }
    } else {
        ConsentResult::Clean
    };

    // 3. Recurse into sub-directories. We always descend so that an
    //    in-progress sub-meta or a dirty sub-meta hidden behind a
    //    gitignore line surfaces; the outer's own status alone can
    //    miss a sub-meta whose contents are gitignored at the parent
    //    level. The descent short-circuits on GitInProgress.
    let mut sub_meta_dirty = false;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(ft) = entry.file_type() else { continue };
            // We only recurse into real directories. Symlinks are
            // not followed — cap-std would refuse them on the prune
            // side anyway, and on the walk side following them risks
            // double-counting or escaping the workspace.
            if !ft.is_dir() {
                continue;
            }
            if path.file_name().is_some_and(|n| n == ".git") {
                continue;
            }
            let child_verdict = walk_inner(&path, /* root */ false);
            match child_verdict {
                ConsentResult::Clean => continue,
                ConsentResult::GitInProgress => return ConsentResult::GitInProgress,
                _ => {
                    sub_meta_dirty = true;
                }
            }
        }
    }

    // 4. Aggregate. If a descendant came back dirty AND `dir` is
    //    itself a meta, that's the SubMetaWithDirtyChildren shape
    //    regardless of `dir`'s own porcelain status. The wrap fires
    //    at every meta level in the recursion (not just the root),
    //    so an inner dirty sub-meta nested below a clean inner meta
    //    still surfaces correctly when the outer call sees the wrap.
    if sub_meta_dirty && dir_is_meta {
        return ConsentResult::SubMetaWithDirtyChildren;
    }
    if sub_meta_dirty {
        // `dir` is a plain directory holding a dirty sub-meta. Pass
        // the dirtiness signal upward as DirtyTree; an outer meta
        // will wrap it into SubMetaWithDirtyChildren on its own
        // aggregation step.
        return ConsentResult::DirtyTree;
    }
    // No descendant was dirty — `dir`'s own status is the verdict.
    let _ = root;
    self_verdict
}

/// Phase 2 prune-safety entry point. Discharges Lean theorem
/// `prune_only_on_clean_consent` via bridge axiom
/// `consent_walk_reflects_fs_state`.
///
/// Behavior:
///
/// 1. Run [`recursive_consent_walk`] against `dest`.
/// 2. If the walk returns [`ConsentResult::Clean`] → execute deletion.
/// 3. If non-Clean, decide based on the override flags (see module-
///    level matrix). If overridden → execute deletion. Otherwise →
///    return [`TreeError::DirtyTreeRefusal`] with the matching kind.
///
/// Deletion is performed via cap-std under [`BoundedDir`] so a
/// hostile pre-existing symlink at the dest slot cannot redirect the
/// rm-rf outside the workspace.
///
/// # Errors
///
/// * [`TreeError::DirtyTreeRefusal`] when the consent walk returned a
///   non-Clean verdict that the override flags do NOT cover.
/// * The function maps any underlying I/O / cap-std error encountered
///   during the actual `rm -rf` to the same refusal variant
///   ([`DirtyTreeRefusalKind::DirtyTree`]) — Phase 2 is a best-effort
///   prune, and propagating raw I/O errors out of the walker is the
///   job of Stage 1.g's wiring (see openspec/changes/feat-v1.2.0-
///   nested-children/design.md §"Concurrency"). For Stage 1.f the
///   primitive's contract is the consent decision; the cap-std open
///   itself only fails on a path that would have escaped the walker
///   boundary anyway.
pub fn phase2_prune(
    dest: &Path,
    force_prune: bool,
    force_prune_with_ignored: bool,
    audit_log: Option<&Path>,
) -> Result<(), TreeError> {
    let verdict = recursive_consent_walk(dest);
    if should_execute(verdict, force_prune, force_prune_with_ignored) {
        // v1.2.0 Stage 1.l — emit a postmortem audit event ONLY when
        // an override flag actually consumed a non-Clean verdict.
        // Clean prunes never write to the audit log; the event is a
        // forensic record of "the operator deliberately overrode
        // safety here, on this path, on this date". Failure to write
        // the event is logged via `tracing` but never aborts the
        // prune — best-effort, like the rest of the manifest writes.
        if let Some(log_path) = audit_log {
            if let Some(kind) = verdict.refusal_kind() {
                emit_force_prune_event(log_path, dest, kind, force_prune_with_ignored);
            }
        }
        return execute_prune(dest);
    }
    let kind = verdict
        .refusal_kind()
        .expect("Clean verdicts always pass should_execute and never reach the refusal arm");
    Err(TreeError::DirtyTreeRefusal { path: dest.to_path_buf(), kind })
}

/// Map a [`DirtyTreeRefusalKind`] to its stable lowercase audit tag.
/// `GitInProgress` is included for completeness even though the
/// override matrix never reaches this code path with that verdict.
fn refusal_kind_tag(kind: DirtyTreeRefusalKind) -> &'static str {
    match kind {
        DirtyTreeRefusalKind::DirtyTree => "dirty_tree",
        DirtyTreeRefusalKind::DirtyTreeWithIgnored => "dirty_tree_with_ignored",
        DirtyTreeRefusalKind::GitInProgress => "git_in_progress",
        DirtyTreeRefusalKind::SubMetaWithDirtyChildren => "sub_meta_with_dirty_children",
    }
}

/// Best-effort append of a [`Event::ForcePruneExecuted`] to the
/// supplied audit log. Failures are logged via `tracing::warn!` but do
/// not propagate — the audit-log write is informational, not
/// transactional. Callers always own the prune decision regardless of
/// whether the event made it to disk.
fn emit_force_prune_event(
    log_path: &Path,
    dest: &Path,
    kind: DirtyTreeRefusalKind,
    force_prune_with_ignored: bool,
) {
    let event = Event::ForcePruneExecuted {
        ts: chrono::Utc::now(),
        path: dest.display().to_string(),
        kind: refusal_kind_tag(kind).to_string(),
        force_prune_with_ignored,
    };
    if let Err(e) = crate::manifest::append::append_event(log_path, &event) {
        tracing::warn!(
            audit_log = %log_path.display(),
            error = %e,
            "failed to append ForcePruneExecuted audit event; prune still executed",
        );
    }
}

/// Decide whether to execute the prune given a consent verdict and
/// the override flags. Pure function — no I/O.
fn should_execute(
    verdict: ConsentResult,
    force_prune: bool,
    force_prune_with_ignored: bool,
) -> bool {
    match verdict {
        ConsentResult::Clean => true,
        ConsentResult::GitInProgress => false,
        ConsentResult::DirtyTree | ConsentResult::SubMetaWithDirtyChildren => {
            force_prune || force_prune_with_ignored
        }
        ConsentResult::DirtyTreeWithIgnored => force_prune_with_ignored,
    }
}

/// Execute the actual `rm -rf dest` via cap-std. Opens the parent as
/// a [`BoundedDir`] handle, then asks cap-std to remove the child by
/// relative name. cap-std refuses to follow symlinks across the
/// parent boundary, so the deletion is inode-confined.
fn execute_prune(dest: &Path) -> Result<(), TreeError> {
    let (parent, name) = match (dest.parent(), dest.file_name()) {
        (Some(p), Some(n)) if !p.as_os_str().is_empty() => (p, PathBuf::from(n)),
        _ => {
            // No usable parent (e.g. root or empty path) — fall back
            // to std::fs and refuse to follow symlinks at the top
            // level. cap-std cannot give us anything stronger than
            // the OS guarantees here.
            return std_fs_remove_with_refusal(dest);
        }
    };
    // Confirm the dest can be opened beneath the parent. The
    // BoundedDir handle is dropped before the actual remove call —
    // cap-std v3's Dir::remove_open_dir_all takes ownership of a
    // child Dir, and on Windows holding an extra handle to a
    // directory you're trying to remove triggers ERROR_SHARING_VIOLATION.
    // Opening + dropping is sufficient for the boundary check; the
    // subsequent remove call still goes through the parent dirfd.
    if BoundedDir::open(parent, &name).is_err() {
        // The dest doesn't exist (already pruned) or escapes the
        // parent. In either case there's nothing for us to remove
        // safely; treat absent-dest as success (idempotent prune)
        // and escape-dest as a refusal.
        if !dest.exists() {
            return Ok(());
        }
        return Err(TreeError::DirtyTreeRefusal {
            path: dest.to_path_buf(),
            kind: DirtyTreeRefusalKind::DirtyTree,
        });
    }

    // Open the parent dirfd and remove the child by relative name.
    let parent_dir = match cap_std::fs::Dir::open_ambient_dir(parent, cap_std::ambient_authority())
    {
        Ok(d) => d,
        Err(_) => return std_fs_remove_with_refusal(dest),
    };
    if parent_dir.remove_dir_all(&name).is_err() {
        // cap-std refuses to remove non-directory entries via
        // remove_dir_all. Try the file path; this also catches the
        // case where the dest disappeared between the consent walk
        // and the remove (idempotent).
        if !dest.exists() {
            return Ok(());
        }
        // Last resort: std::fs (still constrained by the parent
        // having been confirmed via BoundedDir above).
        return std_fs_remove_with_refusal(dest);
    }
    Ok(())
}

/// Fallback path for prune cases where the cap-std handle isn't
/// usable (no parent, or already-vanished dest). Maps any I/O error
/// to a generic [`DirtyTreeRefusalKind::DirtyTree`] refusal so the
/// walker surface stays narrow — Stage 1.g will surface fine-grained
/// I/O errors when wiring the call site.
fn std_fs_remove_with_refusal(dest: &Path) -> Result<(), TreeError> {
    if !dest.exists() {
        return Ok(());
    }
    std::fs::remove_dir_all(dest).map_err(|_| TreeError::DirtyTreeRefusal {
        path: dest.to_path_buf(),
        kind: DirtyTreeRefusalKind::DirtyTree,
    })
}

#[cfg(test)]
mod tests {
    //! Stage 1.f TDD tests. Each consent verdict and each flag
    //! combination from the module-level decision matrix is exercised.
    //!
    //! Tests that depend on a working `git` binary skip themselves
    //! when `git init` fails — same precedent as the
    //! `dest_class::tests` host-skip pattern (see
    //! `crates/grex-core/src/tree/dest_class.rs`).

    use super::*;
    use std::fs;
    use tempfile::tempdir;

    /// Initialise a directory as a git repo. Returns `true` if
    /// successful, `false` if the host has no `git`.
    fn try_git_init(dir: &Path) -> bool {
        let status =
            std::process::Command::new("git").arg("-C").arg(dir).arg("init").arg("-q").status();
        matches!(status, Ok(s) if s.success())
    }

    /// Set a deterministic identity so `git commit` works headlessly.
    fn try_git_identity(dir: &Path) -> bool {
        let a = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.email", "test@example.com"])
            .status();
        let b = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["config", "user.name", "Test"])
            .status();
        matches!((a, b), (Ok(sa), Ok(sb)) if sa.success() && sb.success())
    }

    fn try_git_commit_initial(dir: &Path) -> bool {
        fs::write(dir.join("README"), b"seed\n").unwrap();
        let add =
            std::process::Command::new("git").arg("-C").arg(dir).args(["add", "README"]).status();
        if !matches!(add, Ok(s) if s.success()) {
            return false;
        }
        let commit = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["commit", "-q", "-m", "init"])
            .status();
        matches!(commit, Ok(s) if s.success())
    }

    #[test]
    fn test_consent_clean_repo() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("clean");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        assert_eq!(recursive_consent_walk(&dest), ConsentResult::Clean);
    }

    #[test]
    fn test_consent_dirty_tree() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("dirty");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        // Untracked, non-ignored file → DirtyTree.
        fs::write(dest.join("scratch.txt"), b"changes").unwrap();
        assert_eq!(recursive_consent_walk(&dest), ConsentResult::DirtyTree);
    }

    #[test]
    fn test_consent_dirty_with_ignored() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("ignored-only");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        if !try_git_identity(&dest) || !try_git_commit_initial(&dest) {
            return;
        }
        // Add an ignore rule, commit it, then create the ignored file.
        fs::write(dest.join(".gitignore"), b"target/\n").unwrap();
        let add = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["add", ".gitignore"])
            .status();
        let commit = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["commit", "-q", "-m", "ignore"])
            .status();
        if !matches!(add, Ok(s) if s.success()) || !matches!(commit, Ok(s) if s.success()) {
            return;
        }
        fs::create_dir_all(dest.join("target")).unwrap();
        fs::write(dest.join("target/build.out"), b"artefact").unwrap();
        // `--porcelain` (no --ignored) should report clean; with
        // --ignored we should see the artefact → DirtyTreeWithIgnored.
        assert_eq!(recursive_consent_walk(&dest), ConsentResult::DirtyTreeWithIgnored);
    }

    #[test]
    fn test_consent_git_in_progress() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("midflight");
        fs::create_dir_all(dest.join(".git")).unwrap();
        // Drop a MERGE_HEAD marker; `git_in_progress_at` will pick
        // it up regardless of whether `git init` ran.
        fs::write(dest.join(".git/MERGE_HEAD"), b"deadbeef\n").unwrap();
        assert_eq!(recursive_consent_walk(&dest), ConsentResult::GitInProgress);
    }

    #[test]
    fn test_consent_sub_meta_dirty() {
        // Outer dir is a git repo (so the wrap fires). One of its
        // sub-directories is itself a git repo with a dirty tree.
        let tmp = tempdir().unwrap();
        let outer = tmp.path().join("outer");
        fs::create_dir(&outer).unwrap();
        if !try_git_init(&outer) {
            return;
        }
        let inner = outer.join("inner");
        fs::create_dir(&inner).unwrap();
        if !try_git_init(&inner) {
            return;
        }
        // Make `inner` dirty.
        fs::write(inner.join("scratch.txt"), b"changes").unwrap();
        // Outer itself sees `inner` as untracked and would be
        // DirtyTree; mute that by adding a `.gitignore` (committed)
        // so outer is clean and the only dirtiness is the descendant.
        if !try_git_identity(&outer) || !try_git_commit_initial(&outer) {
            return;
        }
        fs::write(outer.join(".gitignore"), b"inner/\n").unwrap();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&outer)
            .args(["add", ".gitignore"])
            .status();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&outer)
            .args(["commit", "-q", "-m", "ignore inner"])
            .status();
        // Now outer reports clean for its own status; the inner sub-
        // meta is dirty → SubMetaWithDirtyChildren.
        assert_eq!(recursive_consent_walk(&outer), ConsentResult::SubMetaWithDirtyChildren);
    }

    #[test]
    fn test_phase2_prune_clean_succeeds() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("clean");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        let res = phase2_prune(&dest, false, false, None);
        assert!(res.is_ok(), "clean prune must succeed: {res:?}");
        assert!(!dest.exists(), "dest must be removed after a clean prune");
    }

    #[test]
    fn test_phase2_prune_dirty_refuses_without_force() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("dirty");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        fs::write(dest.join("scratch.txt"), b"changes").unwrap();
        let res = phase2_prune(&dest, false, false, None);
        match res {
            Err(TreeError::DirtyTreeRefusal { kind: DirtyTreeRefusalKind::DirtyTree, .. }) => {}
            other => panic!("expected DirtyTreeRefusal{{DirtyTree}}, got {other:?}"),
        }
        assert!(dest.exists(), "dest must NOT be removed when refused");
    }

    #[test]
    fn test_phase2_prune_dirty_succeeds_with_force_prune() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("dirty");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        fs::write(dest.join("scratch.txt"), b"changes").unwrap();
        let res = phase2_prune(&dest, true, false, None);
        assert!(res.is_ok(), "force_prune must consume DirtyTree: {res:?}");
        assert!(!dest.exists(), "dest must be removed under force_prune");
    }

    #[test]
    fn test_phase2_prune_dirty_with_ignored_refuses_force_prune() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("ignored-only");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        if !try_git_identity(&dest) || !try_git_commit_initial(&dest) {
            return;
        }
        fs::write(dest.join(".gitignore"), b"target/\n").unwrap();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["add", ".gitignore"])
            .status();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["commit", "-q", "-m", "ignore"])
            .status();
        fs::create_dir_all(dest.join("target")).unwrap();
        fs::write(dest.join("target/build.out"), b"artefact").unwrap();
        // force_prune alone is INSUFFICIENT: ignored content needs
        // the stronger flag.
        let res = phase2_prune(&dest, true, false, None);
        match res {
            Err(TreeError::DirtyTreeRefusal {
                kind: DirtyTreeRefusalKind::DirtyTreeWithIgnored,
                ..
            }) => {}
            other => panic!("expected DirtyTreeRefusal{{DirtyTreeWithIgnored}}, got {other:?}"),
        }
        assert!(dest.exists(), "dest must NOT be removed when force_prune alone is insufficient");
    }

    #[test]
    fn test_phase2_prune_dirty_with_ignored_succeeds_with_force_prune_with_ignored() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("ignored-only");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        if !try_git_identity(&dest) || !try_git_commit_initial(&dest) {
            return;
        }
        fs::write(dest.join(".gitignore"), b"target/\n").unwrap();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["add", ".gitignore"])
            .status();
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(&dest)
            .args(["commit", "-q", "-m", "ignore"])
            .status();
        fs::create_dir_all(dest.join("target")).unwrap();
        fs::write(dest.join("target/build.out"), b"artefact").unwrap();
        let res = phase2_prune(&dest, true, true, None);
        assert!(res.is_ok(), "force_prune_with_ignored must consume ignored-only dirt: {res:?}");
        assert!(!dest.exists(), "dest must be removed under force_prune_with_ignored");
    }

    #[test]
    fn test_phase2_prune_in_progress_always_refuses() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("midflight");
        fs::create_dir_all(dest.join(".git")).unwrap();
        fs::write(dest.join(".git/MERGE_HEAD"), b"deadbeef\n").unwrap();
        // Even the strongest override flag must NOT consume an
        // in-progress git operation.
        let res = phase2_prune(&dest, true, true, None);
        match res {
            Err(TreeError::DirtyTreeRefusal {
                kind: DirtyTreeRefusalKind::GitInProgress, ..
            }) => {}
            other => panic!("expected DirtyTreeRefusal{{GitInProgress}}, got {other:?}"),
        }
        assert!(dest.exists(), "dest must NOT be removed during an in-progress git op");
    }

    /// Pure-function decision matrix sanity check. Independent of
    /// any FS state — guards against regression in the
    /// `should_execute` table.
    #[test]
    fn test_should_execute_matrix() {
        // Clean is unconditional execute.
        assert!(should_execute(ConsentResult::Clean, false, false));
        assert!(should_execute(ConsentResult::Clean, true, false));
        assert!(should_execute(ConsentResult::Clean, true, true));
        // GitInProgress is unconditional refuse.
        assert!(!should_execute(ConsentResult::GitInProgress, false, false));
        assert!(!should_execute(ConsentResult::GitInProgress, true, false));
        assert!(!should_execute(ConsentResult::GitInProgress, true, true));
        // DirtyTree needs force_prune (or stronger).
        assert!(!should_execute(ConsentResult::DirtyTree, false, false));
        assert!(should_execute(ConsentResult::DirtyTree, true, false));
        assert!(should_execute(ConsentResult::DirtyTree, false, true));
        assert!(should_execute(ConsentResult::DirtyTree, true, true));
        // DirtyTreeWithIgnored needs force_prune_with_ignored
        // SPECIFICALLY — force_prune alone is NOT enough.
        assert!(!should_execute(ConsentResult::DirtyTreeWithIgnored, false, false));
        assert!(!should_execute(ConsentResult::DirtyTreeWithIgnored, true, false));
        assert!(should_execute(ConsentResult::DirtyTreeWithIgnored, false, true));
        assert!(should_execute(ConsentResult::DirtyTreeWithIgnored, true, true));
        // SubMetaWithDirtyChildren parallels DirtyTree.
        assert!(!should_execute(ConsentResult::SubMetaWithDirtyChildren, false, false));
        assert!(should_execute(ConsentResult::SubMetaWithDirtyChildren, true, false));
        assert!(should_execute(ConsentResult::SubMetaWithDirtyChildren, false, true));
        assert!(should_execute(ConsentResult::SubMetaWithDirtyChildren, true, true));
    }

    /// Stage 1.l — when an override flag actually consumes a non-Clean
    /// verdict, `phase2_prune` MUST emit a `ForcePruneExecuted` event
    /// to the audit log so the postmortem trail records the override.
    #[test]
    fn test_phase2_prune_emits_audit_log_on_force() {
        use crate::manifest::append::read_all;
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("dirty");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        // Untracked-non-ignored content → DirtyTree.
        fs::write(dest.join("scratch.txt"), b"changes").unwrap();
        let log = tmp.path().join(".grex/events.jsonl");
        let res = phase2_prune(&dest, true, false, Some(log.as_path()));
        assert!(res.is_ok(), "force_prune must consume DirtyTree: {res:?}");
        assert!(!dest.exists(), "dest must be removed after override");
        let events = read_all(&log).expect("audit log readable");
        assert_eq!(events.len(), 1, "exactly one audit event must land");
        match &events[0] {
            Event::ForcePruneExecuted { kind, force_prune_with_ignored, path, .. } => {
                assert_eq!(kind, "dirty_tree", "kind tag must be dirty_tree");
                assert!(!force_prune_with_ignored, "stronger flag must NOT be in effect here");
                assert!(
                    path.contains("dirty"),
                    "path must reference the pruned dest, got {path:?}",
                );
            }
            other => panic!("expected ForcePruneExecuted, got {other:?}"),
        }
    }

    /// Stage 1.l — Clean prunes are routine and MUST NOT pollute the
    /// audit log. The `ForcePruneExecuted` event records "operator
    /// overrode a refusal"; a clean prune is not an override.
    #[test]
    fn test_phase2_prune_no_audit_when_clean() {
        let tmp = tempdir().unwrap();
        let dest = tmp.path().join("clean");
        fs::create_dir(&dest).unwrap();
        if !try_git_init(&dest) {
            return;
        }
        let log = tmp.path().join(".grex/events.jsonl");
        let res = phase2_prune(&dest, true, true, Some(log.as_path()));
        assert!(res.is_ok(), "clean prune must succeed: {res:?}");
        assert!(!dest.exists(), "dest must be removed after a clean prune");
        // No audit event was emitted, so the file MUST NOT exist.
        // (`append_event` would have created the parent dir + file.)
        assert!(
            !log.exists(),
            "audit log must NOT be created by a clean prune (was: {})",
            log.display(),
        );
    }
}
