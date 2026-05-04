//! v1.1.1 → v1.2.0 lockfile migrator. **Default-OFF, isolated module.**
//!
//! # Stage 0 LOCKED decision #5 — module isolation
//!
//! This module is a *removable unit*. The walker, sync orchestrator,
//! `ls`, `doctor`, `add`, and `rm` MUST NOT call into it. Reachability
//! is restricted to:
//!
//! 1. The CLI's `--migrate-lockfile` flag dispatcher
//!    (`SyncOptions::migrate_lockfile == true` is a *signal* — the
//!    walker never invokes the migrator, the CLI does, *before*
//!    handing off to the walker).
//! 2. The dedicated `grex migrate-lockfile` subcommand.
//! 3. The unit tests below + the module-isolation lint test in
//!    `crates/grex-core/tests/lockfile_migrator_isolation.rs`.
//!
//! Every other call site is an isolation violation. The CI grep test
//! enforces the rule mechanically — if a future patch adds an inbound
//! caller from `walker.rs` / `sync.rs` / `ls.rs` / `doctor.rs` /
//! `add.rs` / `rm.rs`, the lint fails.
//!
//! # What it does
//!
//! v1.1.1 lockfiles wrote one flat JSONL file at
//! `<workspace>/.grex/grex.lock.jsonl`, carrying entries WITHOUT an
//! explicit `path` field. v1.2.0 keeps the same on-disk filename but
//! adds the `path` field to every entry; per-meta distribution then
//! happens incrementally as users opt into nested children (a v1.1.1
//! lockfile already only references direct children of the root meta,
//! since v1.1.1's validator enforced bare-name `path:` — see
//! `.omne/var/migration.md` §"Forward-compat: v1.2.0 reads v1.1.1
//! lockfile").
//!
//! The migrator therefore has a small, well-defined job: rewrite the
//! root meta's lockfile so every entry carries an explicit `path` field
//! derived from the `id` field (the v1.1.1 1:1 id↔folder convention).
//! The output is a v1.2.0-shape lockfile that the walker will accept
//! without legacy-detection erroring.
//!
//! # Idempotency
//!
//! Running the migrator twice is a no-op on the second pass: the second
//! read finds every entry already carrying `path`, the
//! `detect_legacy_lockfile` short-circuit returns `false`, and the
//! migrator exits with `migrated_entries == 0`.

use std::path::{Path, PathBuf};

use super::distributed::{
    detect_legacy_lockfile, meta_lockfile_path, read_meta_lockfile, write_meta_lockfile,
};
use super::entry::LockfileError;

/// Outcome of a single [`migrate_v1_1_1_lockfile`] invocation.
///
/// Marked `#[non_exhaustive]` so future audit fields (per-entry
/// before/after diff, dry-run preview) can land without breaking
/// out-of-crate callers (the CLI subcommand) that destructure the
/// struct.
#[non_exhaustive]
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Path of the lockfile that was migrated.
    pub lockfile: PathBuf,
    /// Number of entries rewritten in place (each gained an explicit
    /// `path` field). Zero for a no-op run on an already-migrated
    /// lockfile or a meta with no lockfile.
    pub migrated_entries: usize,
    /// `true` when the input was already in v1.2.0 shape (no rewrite
    /// happened). Distinct from `migrated_entries == 0` because a
    /// missing file also yields zero entries but is not "already
    /// migrated" — see [`MigrationReport::no_lockfile`].
    pub already_migrated: bool,
    /// `true` when the meta carried no lockfile at all. Mutually
    /// exclusive with [`MigrationReport::already_migrated`].
    pub no_lockfile: bool,
}

/// Migrate the root meta's v1.1.1 lockfile to v1.2.0 in place.
///
/// **Surface contract.** This is the *only* public entry point of the
/// migrator module. The walker / sync / ls / doctor must not import or
/// call it — see the module-level docs for the isolation rule and the
/// `lockfile_migrator_isolation` integration test for the lint.
///
/// # Behavior
///
/// 1. If `<root_meta>/.grex/grex.lock.jsonl` does not exist → no-op,
///    `MigrationReport::no_lockfile == true`.
/// 2. Otherwise, read the existing lockfile via the v1.1.1-tolerant
///    deserializer ([`super::entry::LockEntry`]'s
///    `From<LockEntryRepr>` fills missing `path` fields with `id`).
/// 3. If `detect_legacy_lockfile` returns `false`, the on-disk file is
///    already v1.2.0 — exit with `already_migrated == true`. Idempotent
///    second-run path.
/// 4. Otherwise, rewrite the lockfile atomically via
///    [`write_meta_lockfile`]. Every entry is now serialized with the
///    explicit `path` field (since the deserializer derived it from
///    `id` already, the wire form gains `path` automatically on the
///    next serialize).
///
/// # Errors
///
/// Returns [`LockfileError::Io`] for IO failures, or
/// [`LockfileError::Corruption`] when the legacy lockfile is malformed
/// (a partial line is real corruption — atomic-write contract).
pub fn migrate_v1_1_1_lockfile(root_meta: &Path) -> Result<MigrationReport, LockfileError> {
    let lockfile = meta_lockfile_path(root_meta);

    // 1. Missing-file fast path — no migration needed.
    if !lockfile.exists() {
        return Ok(MigrationReport {
            lockfile,
            migrated_entries: 0,
            already_migrated: false,
            no_lockfile: true,
        });
    }

    // 2. Already-v1.2.0 short-circuit — idempotent second-run path.
    if !detect_legacy_lockfile(root_meta)? {
        return Ok(MigrationReport {
            lockfile,
            migrated_entries: 0,
            already_migrated: true,
            no_lockfile: false,
        });
    }

    // 3. Legacy → rewrite. The reader applies the v1.1.1 fallback
    //    (path = id) automatically; we just need to serialize the
    //    entries back out so the on-disk form gains the explicit
    //    `path` field.
    let entries = read_meta_lockfile(root_meta)?;
    let migrated_entries = entries.len();
    write_meta_lockfile(root_meta, &entries)?;

    Ok(MigrationReport { lockfile, migrated_entries, already_migrated: false, no_lockfile: false })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    /// AC: a v1.1.1-shape lockfile is rewritten to v1.2.0 shape and the
    /// number of migrated entries matches the input.
    #[test]
    fn test_migrator_v1_1_1_to_v1_2_0() {
        let dir = tempdir().unwrap();
        let lock_dir = dir.path().join(".grex");
        std::fs::create_dir_all(&lock_dir).unwrap();

        // Write a v1.1.1-shape lockfile (two entries, no `path` field).
        let v1_1_1 = "\
{\"id\":\"alpha\",\"sha\":\"abc\",\"branch\":\"main\",\"installed_at\":\"2026-04-27T10:00:00Z\",\"actions_hash\":\"h\",\"schema_version\":\"1\"}
{\"id\":\"beta\",\"sha\":\"def\",\"branch\":\"main\",\"installed_at\":\"2026-04-27T10:00:00Z\",\"actions_hash\":\"h\",\"schema_version\":\"1\"}
";
        std::fs::write(lock_dir.join("grex.lock.jsonl"), v1_1_1).unwrap();

        let report = migrate_v1_1_1_lockfile(dir.path()).expect("migrate");
        assert_eq!(report.migrated_entries, 2);
        assert!(!report.already_migrated);
        assert!(!report.no_lockfile);

        // After migration the on-disk form must carry `"path":`.
        let raw = std::fs::read_to_string(lock_dir.join("grex.lock.jsonl")).unwrap();
        assert!(raw.contains("\"path\":\"alpha\""), "got: {raw}");
        assert!(raw.contains("\"path\":\"beta\""), "got: {raw}");
        // And the legacy detector must now clear it.
        assert!(!detect_legacy_lockfile(dir.path()).unwrap());
    }

    /// AC: running the migrator twice is a no-op on the second pass.
    #[test]
    fn test_migrator_idempotent() {
        let dir = tempdir().unwrap();
        let lock_dir = dir.path().join(".grex");
        std::fs::create_dir_all(&lock_dir).unwrap();
        let v1_1_1 = "{\"id\":\"alpha\",\"sha\":\"abc\",\"branch\":\"main\",\"installed_at\":\"2026-04-27T10:00:00Z\",\"actions_hash\":\"h\",\"schema_version\":\"1\"}\n";
        std::fs::write(lock_dir.join("grex.lock.jsonl"), v1_1_1).unwrap();

        let first = migrate_v1_1_1_lockfile(dir.path()).unwrap();
        assert_eq!(first.migrated_entries, 1);
        assert!(!first.already_migrated);

        let bytes_after_first = std::fs::read(lock_dir.join("grex.lock.jsonl")).unwrap();

        let second = migrate_v1_1_1_lockfile(dir.path()).unwrap();
        assert_eq!(second.migrated_entries, 0);
        assert!(second.already_migrated);

        let bytes_after_second = std::fs::read(lock_dir.join("grex.lock.jsonl")).unwrap();
        assert_eq!(bytes_after_first, bytes_after_second, "second run must not rewrite the file",);
    }

    /// AC: a meta with no lockfile reports `no_lockfile` and does not
    /// touch the disk.
    #[test]
    fn test_migrator_no_lockfile_is_noop() {
        let dir = tempdir().unwrap();
        let report = migrate_v1_1_1_lockfile(dir.path()).unwrap();
        assert_eq!(report.migrated_entries, 0);
        assert!(report.no_lockfile);
        assert!(!report.already_migrated);
        assert!(!meta_lockfile_path(dir.path()).exists());
    }
}
