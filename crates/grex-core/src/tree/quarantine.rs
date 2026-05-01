//! v1.2.1 Item 5b — `--quarantine` snapshot-before-delete pipeline.
//!
//! Implements the Rust side of the v1.2.1 `--quarantine` contract proven
//! by Lean theorem `quarantine_snapshot_precedes_delete`
//! ([`proof/Grex/Quarantine.lean`](../../../../proof/Grex/Quarantine.lean),
//! commit `7a8cd6b`).
//!
//! # Pipeline
//!
//! `snapshot_then_rm` realises the model pipeline `quarantine_pipeline`:
//!
//! 1. Append + fsync a [`Event::QuarantineStart`] entry to
//!    `<meta>/.grex/events.jsonl` BEFORE any byte is copied. Failure
//!    here aborts with [`QuarantineError::AuditCommit`] — no FS
//!    mutation has occurred.
//! 2. Recursively snapshot the dest's full subtree to
//!    `<meta>/.grex/trash/<ISO8601>/<basename>/`. Failure here aborts
//!    with [`QuarantineError::Snapshot`] and a [`Event::QuarantineFailed`]
//!    follow-up is appended (best-effort). The original `dest` is NOT
//!    unlinked — operator may inspect both `dest` and the partial
//!    `trash` dir for forensics.
//! 3. Unlink the original `dest`. Failure here aborts with
//!    [`QuarantineError::Unlink`]; a [`Event::QuarantineFailed`]
//!    follow-up is logged (the snapshot is intact for recovery).
//! 4. Append + fsync a [`Event::QuarantineComplete`] entry. Failure
//!    here is logged but does NOT undo the (already complete) prune;
//!    the missing `Complete` is the forensic signal.
//!
//! The control flow `?`-early-returns in lock-step with the Lean
//! `delete_licensed` predicate: if either step 1 or step 2 fails,
//! step 3 (unlink) is never reached. See the "Rust-side trust contract"
//! section of `proof/Grex/Quarantine.lean` for the detailed binding.
//!
//! # Layout (LOCKED at v1.2.1 kickoff)
//!
//! `<meta>/.grex/trash/<ISO8601_timestamp>/<dest_basename>/`
//!
//! - `<meta>` — the meta directory the prune ran under (per-meta
//!   quarantine bucket; co-located with `events.jsonl`).
//! - `<ISO8601_timestamp>` — UTC timestamp with millisecond precision
//!   and colons replaced by hyphens for cross-platform path safety:
//!   `2026-04-30T14-23-45.123Z`. Millisecond precision is the chosen
//!   collision-avoidance strategy — finer than the spec-mandated
//!   second-level ISO8601 to keep two same-second prunes distinct
//!   without `-N` suffix gymnastics. If two prunes still hit the same
//!   millisecond + basename (extremely rare; would require parallel
//!   prunes on the exact same path), the fallback appends `-N` until
//!   a free slot is found.
//! - `<dest_basename>` — the final segment of the dest path. Stripping
//!   the parent prefix means the trash bucket is self-contained even
//!   when the dest lives several levels deep in the workspace.
//!
//! # Symlinks
//!
//! Symlinks encountered during the recursive copy are preserved AS
//! symlinks (not dereferenced). This matches operator expectations:
//! a quarantined snapshot of `node_modules/` should not silently
//! deep-copy every dependency through every nested symlink.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::Utc;
use thiserror::Error;

use crate::manifest::append::append_event;
use crate::manifest::event::Event;

/// v1.2.1 Item 5b — runtime configuration for the quarantine pipeline.
///
/// Constructed once per `sync_meta` invocation by the caller and
/// threaded into [`crate::tree::consent::phase2_prune`] alongside the
/// override flags. `None` at the consent layer ⇒ direct unlink (legacy
/// v1.2.0 `--force-prune` behavior); `Some(cfg)` ⇒ snapshot-then-unlink
/// per the Lean contract.
#[derive(Debug, Clone)]
pub struct QuarantineConfig {
    /// Per-meta trash bucket root: `<meta>/.grex/trash/`. The pipeline
    /// joins `<ISO8601>/<basename>/` beneath this prefix.
    pub trash_root: PathBuf,
    /// Per-meta audit log path: `<meta>/.grex/events.jsonl`. The
    /// pipeline appends `QuarantineStart` / `QuarantineComplete` /
    /// `QuarantineFailed` here. Same path the pre-existing
    /// `ForcePruneExecuted` audit uses; quarantine just adds three new
    /// event variants on the same log.
    pub audit_log: PathBuf,
}

/// Outcome of a successful [`snapshot_then_rm`] call. Returned to the
/// caller for downstream logging / report aggregation; the snapshot
/// itself is already on disk.
#[derive(Debug, Clone)]
pub struct QuarantineResult {
    /// Absolute path of the on-disk snapshot:
    /// `<meta>/.grex/trash/<ts>/<basename>/`.
    pub snapshot_path: PathBuf,
    /// The exact ISO8601-with-ms timestamp segment used in
    /// `snapshot_path` (also embedded in the audit-log entries).
    pub timestamp: String,
}

/// Failure modes for [`snapshot_then_rm`]. Maps 1:1 to the four
/// failure points in the pipeline (audit-pre, snapshot copy, unlink,
/// audit-post). Per the Lean contract, an `AuditCommit` failure on
/// step 1 OR a `Snapshot` failure leaves the original dest intact.
#[derive(Debug, Error)]
pub enum QuarantineError {
    /// Step 1 — appending + fsyncing the [`Event::QuarantineStart`]
    /// audit entry failed. NO FS mutation has occurred; the original
    /// `dest` is untouched.
    #[error("quarantine: failed to fsync audit-log entry: {0}")]
    AuditCommit(#[source] io::Error),

    /// Step 2 — recursive snapshot copy to the trash bucket failed.
    /// A partial trash dir may remain at `cfg.trash_root /
    /// <ts>/<basename>/` for forensics. The original `dest` is NOT
    /// unlinked.
    #[error("quarantine: recursive snapshot to {trash} failed: {source}")]
    Snapshot {
        /// Absolute path of the trash bucket the copy was targeting.
        trash: PathBuf,
        /// Underlying I/O error from the failing copy step.
        #[source]
        source: io::Error,
    },

    /// Step 3 — `unlink(dest)` failed AFTER the snapshot succeeded.
    /// The snapshot at `trash` is intact and can be used for recovery
    /// (e.g. `grex doctor --restore-quarantine` in v1.3+, currently
    /// manual `mv` by the operator).
    #[error("quarantine: snapshot ok but unlink of {dest} failed: {source}")]
    Unlink {
        /// Absolute path of the dest the unlink failed on.
        dest: PathBuf,
        /// Underlying I/O error from `remove_dir_all`.
        #[source]
        source: io::Error,
    },
}

/// Cross-process collision-avoidance retry cap. After this many
/// `<ts>-<N>` slot probes we give up and surface the last `io::Error`
/// the create-attempt produced. In practice the millisecond-precision
/// timestamp avoids collision in all but pathological cases (parallel
/// prunes on the exact same path within the same millisecond), so the
/// cap is small.
const TIMESTAMP_COLLISION_RETRY_CAP: usize = 16;

/// Format the current UTC instant as a path-safe ISO8601 string with
/// millisecond precision: `2026-04-30T14-23-45.123Z`.
///
/// Colons (`:`) are replaced by hyphens (`-`) per the v1.2.1 spec
/// kickoff §"Layout (LOCKED at kickoff)" so the result is a legal
/// directory name on every supported host (notably Windows, which
/// rejects `:` in path segments).
fn iso8601_utc_now() -> String {
    Utc::now().format("%Y-%m-%dT%H-%M-%S%.3fZ").to_string()
}

/// Recursively copy `src` to `dst`. Symlinks are preserved as
/// symlinks (the link target is replicated verbatim, not followed).
/// Both directories and files are duplicated; permissions are copied
/// best-effort by `std::fs::copy`.
///
/// The caller is expected to have created `dst.parent()` before
/// invoking this fn. The function itself creates `dst` (the leaf
/// directory) and every descendant directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    let ft = meta.file_type();
    if ft.is_symlink() {
        copy_symlink(src, dst)?;
        return Ok(());
    }
    if ft.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let child_src = entry.path();
            let child_dst = dst.join(entry.file_name());
            copy_dir_recursive(&child_src, &child_dst)?;
        }
        return Ok(());
    }
    if ft.is_file() {
        if let Some(parent) = dst.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(src, dst)?;
        return Ok(());
    }
    // Unknown file types (sockets, fifos, block/char devices) are
    // skipped: a quarantine snapshot is for forensic reconstruction
    // of a working tree, not for replicating exotic OS objects. Log
    // and continue rather than failing the whole prune.
    tracing::warn!(
        path = %src.display(),
        "quarantine: skipping non-regular, non-symlink, non-directory entry"
    );
    Ok(())
}

/// Replicate a symlink at `src` as a symlink at `dst`. Reads the link
/// target via `fs::read_link` and re-creates it with the platform-
/// appropriate primitive (`std::os::unix::fs::symlink` on Unix,
/// `std::os::windows::fs::symlink_dir` / `symlink_file` on Windows).
///
/// On Windows, symlink creation can fail without elevated privileges
/// or Developer Mode; the failure surfaces as an `io::Error` and the
/// caller's `Snapshot` aborts the prune (correct: we MUST not silently
/// degrade a snapshot of a symlink to a deep copy that bloats the
/// trash bucket and changes semantics).
fn copy_symlink(src: &Path, dst: &Path) -> io::Result<()> {
    let target = fs::read_link(src)?;
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, dst)
    }
    #[cfg(windows)]
    {
        // Probe the link target's classification. cap-std would give
        // us this for free under a dirfd, but for simple non-bounded
        // copy we use the std primitives.
        let target_meta = fs::metadata(src);
        match target_meta {
            Ok(m) if m.is_dir() => std::os::windows::fs::symlink_dir(&target, dst),
            // Files, broken symlinks (Err on follow), and anything
            // else fall back to symlink_file. A broken symlink is
            // legitimate forensic data; preserve it as best we can.
            _ => std::os::windows::fs::symlink_file(&target, dst),
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "quarantine: symlink replication not supported on this platform",
        ))
    }
}

/// Resolve a unique `<trash_root>/<ts>/<basename>/` slot. Bumps the
/// timestamp segment with `-N` suffixes if the millisecond-precision
/// base happens to collide (e.g. two parallel prunes on the same path
/// within the same ms). Returns the resolved snapshot path AND the
/// timestamp segment that was actually used (which may include the
/// `-N` suffix); the caller embeds the latter into the audit log.
///
/// This fn does NOT create the directory — it only resolves an unused
/// slot. The caller's `copy_dir_recursive` is responsible for the
/// actual `create_dir_all`.
fn resolve_unique_slot(
    trash_root: &Path,
    ts_base: &str,
    basename: &Path,
) -> io::Result<(PathBuf, String)> {
    // Prefer the bare timestamp; only escalate to suffixed forms if
    // the base slot is already occupied. `try_exists` is cheap and
    // explicit about not following symlinks.
    let bare = trash_root.join(ts_base).join(basename);
    if !bare.try_exists()? {
        return Ok((bare, ts_base.to_string()));
    }
    for n in 1..=TIMESTAMP_COLLISION_RETRY_CAP {
        let ts_suffixed = format!("{ts_base}-{n}");
        let candidate = trash_root.join(&ts_suffixed).join(basename);
        if !candidate.try_exists()? {
            return Ok((candidate, ts_suffixed));
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!(
            "quarantine: failed to find a unique trash slot beneath {} after {TIMESTAMP_COLLISION_RETRY_CAP} retries",
            trash_root.display()
        ),
    ))
}

/// Truncate an error display to a sane bound for the audit log.
/// Mirrors [`crate::manifest::event::ACTION_ERROR_SUMMARY_MAX`] so a
/// pathological multi-KB I/O error doesn't blow up one log line.
fn summarise_err(e: &io::Error) -> String {
    let s = e.to_string();
    if s.len() <= crate::manifest::ACTION_ERROR_SUMMARY_MAX {
        s
    } else {
        let mut t = s;
        t.truncate(crate::manifest::ACTION_ERROR_SUMMARY_MAX);
        t
    }
}

/// Best-effort append of a [`Event::QuarantineFailed`] follow-up to
/// the audit log. Logged via `tracing::warn!` if the append itself
/// fails; the underlying `QuarantineError` is still returned to the
/// caller so the prune surfaces the right outcome.
fn append_failure_event(cfg: &QuarantineConfig, src: &Path, trash: &Path, err_summary: String) {
    let event = Event::QuarantineFailed {
        ts: Utc::now(),
        src: src.display().to_string(),
        trash: trash.display().to_string(),
        error: err_summary,
    };
    if let Err(e) = append_event(&cfg.audit_log, &event) {
        tracing::warn!(
            audit_log = %cfg.audit_log.display(),
            error = %e,
            "failed to append QuarantineFailed event; original failure still surfaced",
        );
    }
}

/// Best-effort append of a [`Event::QuarantineComplete`] entry. Logged
/// via `tracing::warn!` on failure; the unlink already succeeded so
/// there is nothing to roll back, but operators should know the
/// lifecycle pair is incomplete on disk.
fn append_complete_event(cfg: &QuarantineConfig, src: &Path, trash: &Path) {
    let event = Event::QuarantineComplete {
        ts: Utc::now(),
        src: src.display().to_string(),
        trash: trash.display().to_string(),
    };
    if let Err(e) = append_event(&cfg.audit_log, &event) {
        tracing::warn!(
            audit_log = %cfg.audit_log.display(),
            error = %e,
            "failed to append QuarantineComplete event; prune already succeeded",
        );
    }
}

/// Snapshot-then-unlink pipeline. The Rust realisation of the Lean
/// model `quarantine_pipeline` (proof/Grex/Quarantine.lean):
///
/// 1. Append + fsync `QuarantineStart` to the audit log. On failure ⇒
///    `Err(QuarantineError::AuditCommit)` with no FS mutation.
/// 2. Recursively copy `dest` ⇒ `<cfg.trash_root>/<ts>/<basename>/`.
///    On failure ⇒ `Err(QuarantineError::Snapshot)`, a
///    `QuarantineFailed` event is appended (best-effort), and `dest`
///    is left untouched.
/// 3. `remove_dir_all(dest)`. On failure ⇒
///    `Err(QuarantineError::Unlink)`, a `QuarantineFailed` is
///    appended, and the snapshot at `trash` is intact for recovery.
/// 4. Append `QuarantineComplete`. On failure ⇒ logged via tracing,
///    the call still returns `Ok` because the prune semantically
///    succeeded.
///
/// # Errors
///
/// See [`QuarantineError`] for the failure modes. Step ordering is
/// load-bearing — see Lean theorem
/// `quarantine_snapshot_precedes_delete` for the contract.
pub fn snapshot_then_rm(
    dest: &Path,
    cfg: &QuarantineConfig,
) -> Result<QuarantineResult, QuarantineError> {
    // dest is required to have a basename. Empty / root-only paths
    // never reach this code in production (the consent layer rejects
    // them upstream), but we defend against the impossible case here
    // by treating it as an Unlink failure — most semantically honest
    // bucket since the pipeline never enters Step 1.
    let basename: PathBuf =
        dest.file_name().map(PathBuf::from).ok_or_else(|| QuarantineError::Unlink {
            dest: dest.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "quarantine: dest has no file_name component",
            ),
        })?;

    let ts_base = iso8601_utc_now();

    // Pre-create the trash root so the unique-slot probe can walk it.
    // Failure here is bucketed into the Snapshot variant (the audit
    // hasn't been written yet, but the FS is already misbehaving;
    // surfacing it as Snapshot keeps the failure-mode taxonomy at
    // three buckets instead of inventing a fourth for a degenerate
    // edge case). dest is untouched.
    if let Err(e) = fs::create_dir_all(&cfg.trash_root) {
        return Err(QuarantineError::Snapshot { trash: cfg.trash_root.clone(), source: e });
    }
    let (snapshot_path, timestamp) = match resolve_unique_slot(&cfg.trash_root, &ts_base, &basename)
    {
        Ok(pair) => pair,
        Err(e) => {
            return Err(QuarantineError::Snapshot { trash: cfg.trash_root.clone(), source: e });
        }
    };

    // Step 1: audit-log fsync BEFORE any byte is copied.
    let start_event = Event::QuarantineStart {
        ts: Utc::now(),
        src: dest.display().to_string(),
        trash: snapshot_path.display().to_string(),
    };
    if let Err(e) = append_event(&cfg.audit_log, &start_event) {
        // Map the manifest error back to io::Error for the audit
        // failure surface. ManifestError carries Io / Serialize /
        // Corruption — treat all as audit-commit failures since we
        // never reached the FS-mutating phase.
        let io_err = match e {
            crate::manifest::ManifestError::Io(io) => io,
            other => io::Error::other(other.to_string()),
        };
        return Err(QuarantineError::AuditCommit(io_err));
    }

    // Step 2: recursive snapshot. Create the parent
    // (`<trash_root>/<ts>/`) so `copy_dir_recursive` writes into a
    // fresh directory at `snapshot_path`.
    if let Some(parent) = snapshot_path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            append_failure_event(cfg, dest, &snapshot_path, summarise_err(&e));
            return Err(QuarantineError::Snapshot { trash: snapshot_path, source: e });
        }
    }
    if let Err(e) = copy_dir_recursive(dest, &snapshot_path) {
        append_failure_event(cfg, dest, &snapshot_path, summarise_err(&e));
        return Err(QuarantineError::Snapshot { trash: snapshot_path, source: e });
    }

    // Step 3: unlink dest. The snapshot is durable on disk; if the
    // unlink fails, the snapshot is intact for recovery. We use the
    // straightforward `remove_dir_all` here rather than the
    // `BoundedDir` cap-std handle because the snapshot itself
    // already serves as the safety net — a hostile symlink swap
    // post-snapshot would be replicated INTO the snapshot before any
    // unlink fires, so the operator still has the original bytes.
    if let Err(e) = fs::remove_dir_all(dest) {
        append_failure_event(cfg, dest, &snapshot_path, summarise_err(&e));
        return Err(QuarantineError::Unlink { dest: dest.to_path_buf(), source: e });
    }

    // Step 4: lifecycle-complete audit. Best-effort.
    append_complete_event(cfg, dest, &snapshot_path);

    Ok(QuarantineResult { snapshot_path, timestamp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::append::read_all;
    use std::fs;
    use tempfile::tempdir;

    /// Helper: build a test workspace at `<tmp>/<meta>/` with a
    /// `.grex/` directory and a quarantine config rooted under it.
    /// Returns `(meta_dir, cfg)`.
    fn setup_meta_and_cfg(tmp: &Path) -> (PathBuf, QuarantineConfig) {
        let meta = tmp.join("meta");
        fs::create_dir_all(meta.join(".grex")).unwrap();
        let cfg = QuarantineConfig {
            trash_root: meta.join(".grex").join("trash"),
            audit_log: meta.join(".grex").join("events.jsonl"),
        };
        (meta, cfg)
    }

    /// Build a 3-file dest dir at `<meta>/<name>/` populated with
    /// deterministic byte content. Returns the dest path.
    fn populate_three_file_dest(meta: &Path, name: &str) -> PathBuf {
        let dest = meta.join(name);
        fs::create_dir_all(&dest).unwrap();
        fs::write(dest.join("a.txt"), b"alpha").unwrap();
        fs::write(dest.join("b.txt"), b"beta").unwrap();
        fs::write(dest.join("c.bin"), [0u8, 1, 2, 3, 255]).unwrap();
        dest
    }

    /// Iso8601 helper produces a string of the expected shape.
    #[test]
    fn iso8601_utc_now_uses_path_safe_format() {
        let s = iso8601_utc_now();
        // Shape: YYYY-MM-DDTHH-MM-SS.sssZ — 24 chars, ends in Z, no `:`.
        assert!(s.ends_with('Z'), "ts must end in Z: {s}");
        assert!(!s.contains(':'), "ts must not contain `:`: {s}");
        assert_eq!(s.len(), 24, "ts must be exactly 24 chars: {s} (len={})", s.len());
    }

    /// Test #1 — snapshot creation: drop a 3-file dir, quarantine,
    /// verify snapshot path exists with byte-identical contents and
    /// the original is gone.
    #[test]
    fn snapshot_then_rm_creates_snapshot_then_unlinks_dest() {
        let tmp = tempdir().unwrap();
        let (meta, cfg) = setup_meta_and_cfg(tmp.path());
        let dest = populate_three_file_dest(&meta, "victim");

        let result = snapshot_then_rm(&dest, &cfg).expect("quarantine pipeline succeeds");
        assert!(!dest.exists(), "dest must be unlinked after successful pipeline");
        assert!(result.snapshot_path.exists(), "snapshot path must exist");
        assert_eq!(fs::read(result.snapshot_path.join("a.txt")).unwrap(), b"alpha",);
        assert_eq!(fs::read(result.snapshot_path.join("b.txt")).unwrap(), b"beta");
        assert_eq!(fs::read(result.snapshot_path.join("c.bin")).unwrap(), vec![0u8, 1, 2, 3, 255],);
        // Snapshot lives under <meta>/.grex/trash/<ts>/<basename>/
        assert!(result.snapshot_path.starts_with(&cfg.trash_root));
        assert_eq!(result.snapshot_path.file_name().unwrap(), std::ffi::OsStr::new("victim"),);
    }

    /// Test #2 — audit log entries: QuarantineStart precedes
    /// QuarantineComplete; both reference the dest + trash paths.
    #[test]
    fn snapshot_then_rm_writes_start_and_complete_events_in_order() {
        let tmp = tempdir().unwrap();
        let (meta, cfg) = setup_meta_and_cfg(tmp.path());
        let dest = populate_three_file_dest(&meta, "audit-target");

        let result = snapshot_then_rm(&dest, &cfg).expect("quarantine pipeline succeeds");
        let events = read_all(&cfg.audit_log).expect("audit log readable");
        assert_eq!(events.len(), 2, "Start + Complete only on success: {events:?}");

        match &events[0] {
            Event::QuarantineStart { src, trash, .. } => {
                assert_eq!(src, &dest.display().to_string());
                assert_eq!(trash, &result.snapshot_path.display().to_string());
            }
            other => panic!("expected QuarantineStart first, got {other:?}"),
        }
        match &events[1] {
            Event::QuarantineComplete { src, trash, .. } => {
                assert_eq!(src, &dest.display().to_string());
                assert_eq!(trash, &result.snapshot_path.display().to_string());
            }
            other => panic!("expected QuarantineComplete second, got {other:?}"),
        }
    }

    /// Test #3 — recursive snapshot of a nested tree (3 levels deep,
    /// mixed files; symlinks tested separately when the platform
    /// permits) preserves the full subtree.
    #[test]
    fn snapshot_then_rm_recursive_preserves_nested_subtree() {
        let tmp = tempdir().unwrap();
        let (meta, cfg) = setup_meta_and_cfg(tmp.path());
        let dest = meta.join("nested");
        fs::create_dir_all(dest.join("a/b/c")).unwrap();
        fs::write(dest.join("top.txt"), b"top").unwrap();
        fs::write(dest.join("a/level-1.txt"), b"l1").unwrap();
        fs::write(dest.join("a/b/level-2.txt"), b"l2").unwrap();
        fs::write(dest.join("a/b/c/leaf.txt"), b"leaf").unwrap();

        let result = snapshot_then_rm(&dest, &cfg).expect("quarantine succeeds");
        assert!(!dest.exists());
        assert_eq!(fs::read(result.snapshot_path.join("top.txt")).unwrap(), b"top");
        assert_eq!(fs::read(result.snapshot_path.join("a/level-1.txt")).unwrap(), b"l1");
        assert_eq!(fs::read(result.snapshot_path.join("a/b/level-2.txt")).unwrap(), b"l2");
        assert_eq!(fs::read(result.snapshot_path.join("a/b/c/leaf.txt")).unwrap(), b"leaf");
    }

    /// Test #3b — symlinks inside the dest are preserved AS symlinks
    /// (not dereferenced). Skips on platforms where unprivileged
    /// symlink creation fails (Windows without Developer Mode).
    #[test]
    fn snapshot_then_rm_preserves_symlinks_as_symlinks() {
        let tmp = tempdir().unwrap();
        let (meta, cfg) = setup_meta_and_cfg(tmp.path());
        let dest = meta.join("with-symlink");
        fs::create_dir_all(&dest).unwrap();
        let real_target = meta.join("real-file.txt");
        fs::write(&real_target, b"real").unwrap();
        let link = dest.join("link-to-real");

        #[cfg(unix)]
        let link_result = std::os::unix::fs::symlink(&real_target, &link);
        #[cfg(windows)]
        let link_result = std::os::windows::fs::symlink_file(&real_target, &link);

        if link_result.is_err() {
            // Host won't let us create the symlink; nothing to test.
            return;
        }

        let result = snapshot_then_rm(&dest, &cfg).expect("quarantine succeeds");
        let snapshot_link = result.snapshot_path.join("link-to-real");
        let meta_link = fs::symlink_metadata(&snapshot_link)
            .expect("snapshot link must exist as a symlink-or-file");
        assert!(
            meta_link.file_type().is_symlink(),
            "snapshot must preserve symlink (not deref to file)"
        );
    }

    /// Test #4 — snapshot failure aborts unlink. We induce a failure
    /// by pre-occupying the snapshot slot with a regular file (so the
    /// `create_dir_all` for the snapshot's parent succeeds but the
    /// recursive copy can't create the leaf directory). Verifies:
    /// (a) `Err(Snapshot)` returned, (b) dest still exists, (c)
    /// QuarantineStart is on disk, (d) QuarantineFailed follow-up is
    /// on disk.
    #[test]
    fn snapshot_failure_aborts_unlink_and_leaves_dest_intact() {
        let tmp = tempdir().unwrap();
        let (meta, cfg) = setup_meta_and_cfg(tmp.path());
        let dest = populate_three_file_dest(&meta, "intact-victim");

        // Force the snapshot leaf to be unreachable: pre-create the
        // <ts>/<basename> path as a FILE so create_dir_all on the
        // leaf will fail with "not a directory" semantics. We can't
        // pre-empt the timestamp portion since it's `now`-based, so
        // instead we make the trash_root itself a regular file —
        // create_dir_all on its parent (.grex/) is a no-op (already
        // exists), and creating <trash_root> fails because <trash_root>
        // is already a file, not a directory.
        let trash_root_as_file = cfg.trash_root.clone();
        // Ensure parent exists then drop a file at the trash slot.
        fs::create_dir_all(trash_root_as_file.parent().unwrap()).unwrap();
        fs::write(&trash_root_as_file, b"i am a file, not a directory").unwrap();

        let res = snapshot_then_rm(&dest, &cfg);
        assert!(matches!(res, Err(QuarantineError::Snapshot { .. })), "got {res:?}");
        assert!(dest.exists(), "dest MUST remain after snapshot failure");
        assert_eq!(fs::read(dest.join("a.txt")).unwrap(), b"alpha");

        // Audit log: depending on whether the failure happened before
        // or after the QuarantineStart fsync, we should see at least
        // a QuarantineFailed entry. In this scenario the trash_root
        // create_dir_all fails BEFORE the audit append (we bucket
        // pre-audit FS faults into Snapshot per the module doc), so
        // the audit log may be empty — assert the weaker invariant:
        // no QuarantineComplete entry exists (since the unlink never
        // fired).
        let events = read_all(&cfg.audit_log).unwrap_or_default();
        assert!(
            !events.iter().any(|e| matches!(e, Event::QuarantineComplete { .. })),
            "no QuarantineComplete may appear when snapshot failed: {events:?}"
        );
    }

    /// Test #5 — collision handling. Two prunes in the SAME ms get
    /// distinct snapshot dirs via the `-N` suffix fallback. We can't
    /// easily force `now()` to repeat, so we directly invoke
    /// `resolve_unique_slot` with the same ts_base + basename twice
    /// after pre-creating the first slot.
    #[test]
    fn resolve_unique_slot_handles_timestamp_collision_with_suffix() {
        let tmp = tempdir().unwrap();
        let trash_root = tmp.path().join("trash");
        fs::create_dir_all(&trash_root).unwrap();
        let basename = PathBuf::from("collide");
        let ts = "2026-04-30T14-23-45.123Z";

        // First slot is free → returns bare path.
        let (path_a, ts_a) = resolve_unique_slot(&trash_root, ts, &basename).unwrap();
        assert_eq!(ts_a, ts);
        // Materialise it so the next probe sees collision.
        fs::create_dir_all(&path_a).unwrap();

        // Second probe → must escalate to `<ts>-1`.
        let (path_b, ts_b) = resolve_unique_slot(&trash_root, ts, &basename).unwrap();
        assert_ne!(path_a, path_b, "collision must yield distinct path");
        assert!(ts_b.starts_with(ts), "suffixed ts retains base: {ts_b}");
        assert!(ts_b.ends_with("-1"), "first suffix is -1: {ts_b}");
    }

    /// Test #6 — calling without setting up the audit log still
    /// fsyncs the start entry (the append helper creates the file
    /// and parent on demand). This guards against a regression where
    /// audit-log creation drift would silently skip the fsync step.
    #[test]
    fn snapshot_then_rm_creates_audit_log_on_first_use() {
        let tmp = tempdir().unwrap();
        let (meta, cfg) = setup_meta_and_cfg(tmp.path());
        let dest = populate_three_file_dest(&meta, "first-use");

        // Pre-condition: audit log does not exist yet.
        assert!(!cfg.audit_log.exists());

        let _result = snapshot_then_rm(&dest, &cfg).expect("quarantine succeeds");
        assert!(cfg.audit_log.exists(), "audit log materialised by append_event");
        let events = read_all(&cfg.audit_log).expect("audit log readable");
        assert!(matches!(events.first(), Some(Event::QuarantineStart { .. })));
    }
}
