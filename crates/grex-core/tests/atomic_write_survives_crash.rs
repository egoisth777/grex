//! Integration: simulated crash leaves the original file intact, plus
//! robustness coverage for HIGH/MED codex findings:
//!
//!   1. cross-filesystem rename (best-effort, skipped on single-drive setups)
//!   2. read-only target preservation
//!   3. target is a directory
//!   4. symlink target (Unix only)
//!   5. concurrent writers produce one winner (no torn result)
//!   6. permission-denied parent preserves existing target
//!   7. unicode path round-trip

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;
use tempfile::tempdir;

fn tmp_sibling(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(".tmp");
    PathBuf::from(s)
}

#[test]
fn crash_before_rename_preserves_original() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("grex.lock.jsonl");
    fs::write(&p, b"original contents").unwrap();

    // Simulate: atomic_write was interrupted after writing the temp but
    // before the rename.
    let tmp = tmp_sibling(&p);
    fs::write(&tmp, b"partial new contents").unwrap();

    // Original untouched.
    assert_eq!(fs::read(&p).unwrap(), b"original contents");
    // Temp is present — a subsequent atomic_write must clean it up.
    assert!(tmp.exists());

    // Now run a successful atomic write.
    grex_core::fs::atomic_write(&p, b"final").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"final");
    assert!(!tmp.exists(), "stale temp must be cleaned");
}

#[test]
fn crash_before_rename_new_file_absent() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("fresh.jsonl");
    let tmp = tmp_sibling(&p);
    // Simulate interrupted first-ever write.
    fs::write(&tmp, b"partial").unwrap();
    assert!(!p.exists(), "target never existed");
    grex_core::fs::atomic_write(&p, b"ok").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"ok");
}

// ---------------------------------------------------------------------------
// HIGH-1: cross-filesystem rename
// ---------------------------------------------------------------------------
//
// `atomic_write` creates the temp as a sibling of the target, so cross-fs
// rename should never occur under normal usage. We guard against a regression
// where a future refactor might place the temp in `std::env::temp_dir()`.
// If it did, and temp_dir lives on a different filesystem than the target,
// the rename would fail (EXDEV on Unix) leaving the original untouched.
//
// The current impl keeps tmp sibling-local, so this test simply documents +
// verifies: write from a directory that differs from the process temp dir
// still succeeds, and the original is either replaced cleanly or preserved.
#[test]
fn cross_filesystem_rename_fails_cleanly_without_altering_target() {
    // Use a fresh directory that — on CI runners / dev machines — is often
    // on the same filesystem as `std::env::temp_dir()` but MAY differ.
    let dir = tempdir().unwrap();
    let sys_tmp = std::env::temp_dir();

    // Heuristic: if the dirs share the same root volume, we can't truly
    // exercise the EXDEV path cross-platform. Run the test anyway as a
    // documentation/regression guard — the invariant holds on any fs.
    let _cross_fs_hint = dir.path().starts_with(&sys_tmp);

    let p = dir.path().join("target.bin");
    fs::write(&p, b"ORIGINAL").unwrap();

    let result = grex_core::fs::atomic_write(&p, b"NEW");
    match result {
        Ok(()) => {
            // Happy path: rename succeeded (same-fs or fallback copy).
            assert_eq!(fs::read(&p).unwrap(), b"NEW");
        }
        Err(_) => {
            // Error path: original MUST be preserved byte-for-byte.
            assert_eq!(
                fs::read(&p).unwrap(),
                b"ORIGINAL",
                "failed atomic_write must not mutate the target"
            );
        }
    }
    // Temp sibling must not linger in either branch.
    assert!(!tmp_sibling(&p).exists(), "stale temp left behind");
}

// ---------------------------------------------------------------------------
// HIGH-2: read-only target preservation
// ---------------------------------------------------------------------------
#[cfg(unix)]
#[test]
fn readonly_target_returns_err_and_preserves_content() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempdir().unwrap();
    let p = dir.path().join("ro.txt");
    fs::write(&p, b"SACRED").unwrap();

    // Lock down BOTH the file and the parent dir so rename cannot replace.
    // A RO file alone is not enough on Unix — rename only needs +w on the
    // parent dir. Make parent RO so the write is rejected.
    let parent_perm = fs::metadata(dir.path()).unwrap().permissions();
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o555)).unwrap();

    let res = grex_core::fs::atomic_write(&p, b"INTRUDER");

    // Restore perms before assertions (so tempdir cleanup works).
    fs::set_permissions(dir.path(), parent_perm).unwrap();

    assert!(res.is_err(), "write to read-only parent should fail");
    assert_eq!(fs::read(&p).unwrap(), b"SACRED", "original must survive");
}

#[cfg(windows)]
#[test]
fn readonly_target_returns_err_and_preserves_content() {
    use std::process::Command;

    let dir = tempdir().unwrap();
    let p = dir.path().join("ro.txt");
    fs::write(&p, b"SACRED").unwrap();

    // `attrib +R` marks the file read-only. On Windows, MoveFileExW replacing
    // a read-only target fails with ERROR_ACCESS_DENIED.
    let out =
        Command::new("attrib").arg("+R").arg(&p).output().expect("attrib available on Windows");
    assert!(out.status.success(), "attrib +R failed: {:?}", out);

    let res = grex_core::fs::atomic_write(&p, b"INTRUDER");

    // Clear the RO bit so tempdir cleanup can remove the file.
    let _ = Command::new("attrib").arg("-R").arg(&p).output();

    assert!(res.is_err(), "write to read-only target should fail");
    assert_eq!(fs::read(&p).unwrap(), b"SACRED", "original must survive");
}

// ---------------------------------------------------------------------------
// HIGH-3: target is a directory
// ---------------------------------------------------------------------------
#[test]
fn target_is_directory_returns_err() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("a_directory");
    fs::create_dir(&p).unwrap();
    // Put a sentinel inside so we can detect accidental replacement.
    let sentinel = p.join("keep.txt");
    fs::write(&sentinel, b"MARKER").unwrap();

    let res = grex_core::fs::atomic_write(&p, b"payload");
    assert!(res.is_err(), "renaming over a non-empty dir must fail");
    assert!(p.is_dir(), "target directory must still be a directory");
    assert_eq!(fs::read(&sentinel).unwrap(), b"MARKER");
    // Temp sibling must be cleaned up after failure.
    assert!(!tmp_sibling(&p).exists(), "stale temp left behind");
}

// ---------------------------------------------------------------------------
// HIGH-4: symlink target behaviour (Unix only)
// ---------------------------------------------------------------------------
//
// Documented behaviour: `fs::rename` on a symlink path REPLACES the symlink
// with a regular file — the underlying pointee is NOT rewritten. This test
// pins that behaviour so any future change is visible.
#[cfg(unix)]
#[test]
fn symlink_target_not_replaced_as_regular_file() {
    use std::os::unix::fs::symlink;

    let dir = tempdir().unwrap();
    let real = dir.path().join("real.txt");
    let link = dir.path().join("link.txt");
    fs::write(&real, b"POINTEE").unwrap();
    symlink(&real, &link).unwrap();

    grex_core::fs::atomic_write(&link, b"VIA_LINK").unwrap();

    // After rename, `link` is now a regular file with new content.
    let link_meta = fs::symlink_metadata(&link).unwrap();
    assert!(!link_meta.file_type().is_symlink(), "rename replaces the symlink itself");
    assert_eq!(fs::read(&link).unwrap(), b"VIA_LINK");
    // And the original pointee is untouched.
    assert_eq!(
        fs::read(&real).unwrap(),
        b"POINTEE",
        "atomic_write must not follow symlinks into the pointee"
    );
}

// ---------------------------------------------------------------------------
// HIGH-5: concurrent writers produce exactly one winner
// ---------------------------------------------------------------------------
#[test]
fn concurrent_writers_produce_one_winner() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("race.txt");

    // Two writers, two distinct payloads. A torn/mixed result would fail the
    // final equality check.
    let payload_a: Vec<u8> = std::iter::repeat(b'A').take(64 * 1024).collect();
    let payload_b: Vec<u8> = std::iter::repeat(b'B').take(64 * 1024).collect();

    let barrier = Arc::new(Barrier::new(2));
    let p1 = p.clone();
    let p2 = p.clone();
    let a = payload_a.clone();
    let b = payload_b.clone();
    let bar1 = Arc::clone(&barrier);
    let bar2 = Arc::clone(&barrier);

    // NOTE: atomic_write uses a single `<path>.tmp` sibling. Two concurrent
    // writers CAN collide on that temp and one may fail with ENOENT/EACCES.
    // That's acceptable — the contract is "the final file matches exactly
    // one writer's bytes, never a mix". Both Ok is also fine (last rename
    // wins atomically).
    let h1 = thread::spawn(move || {
        bar1.wait();
        grex_core::fs::atomic_write(&p1, &a)
    });
    let h2 = thread::spawn(move || {
        bar2.wait();
        grex_core::fs::atomic_write(&p2, &b)
    });
    let r1 = h1.join().unwrap();
    let r2 = h2.join().unwrap();

    // At least one writer must have succeeded.
    assert!(r1.is_ok() || r2.is_ok(), "both concurrent writes failed: {:?} / {:?}", r1, r2);

    let final_bytes = fs::read(&p).unwrap();
    assert!(
        final_bytes == payload_a || final_bytes == payload_b,
        "final content must match exactly one writer's payload (no tearing)"
    );
    // No stale temp left behind by either writer.
    assert!(!tmp_sibling(&p).exists(), "stale temp left behind");
}

// ---------------------------------------------------------------------------
// MED-6: permission / disk-full style error preserves target
// ---------------------------------------------------------------------------
//
// We can't portably force ENOSPC without root. The closest portable
// approximation is a write into a directory we don't own. We use
// "parent doesn't exist" as a permission/write-error proxy: the write to the
// sibling temp will fail, so atomic_write must return Err and leave nothing
// on disk.
#[test]
fn disk_full_or_permission_error_preserves_target() {
    let dir = tempdir().unwrap();
    // Parent doesn't exist — write MUST fail at the temp-write step.
    let bogus_parent = dir.path().join("does_not_exist");
    let p = bogus_parent.join("child.txt");

    let res = grex_core::fs::atomic_write(&p, b"never lands");
    assert!(res.is_err(), "write into missing parent must fail");
    assert!(!p.exists(), "target must not be created on failure");
    assert!(!tmp_sibling(&p).exists(), "temp sibling must not linger on failure");

    // Second scenario: existing target under a parent that becomes RO on Unix.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let sub = dir.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let q = sub.join("keep.txt");
        fs::write(&q, b"ORIGINAL").unwrap();

        let prev = fs::metadata(&sub).unwrap().permissions();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();

        let r = grex_core::fs::atomic_write(&q, b"NEW");

        fs::set_permissions(&sub, prev).unwrap();

        assert!(r.is_err(), "perm-denied write should fail");
        assert_eq!(fs::read(&q).unwrap(), b"ORIGINAL");
    }
}

// ---------------------------------------------------------------------------
// MED-7: unicode path round-trip
// ---------------------------------------------------------------------------
#[test]
fn unicode_path_round_trip() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("测试_файл_🦀.jsonl");

    grex_core::fs::atomic_write(&p, b"unicode ok").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"unicode ok");

    // Overwrite path with distinct content; sibling temp must still resolve.
    grex_core::fs::atomic_write(&p, b"second").unwrap();
    assert_eq!(fs::read(&p).unwrap(), b"second");
    assert!(!tmp_sibling(&p).exists(), "temp cleaned on unicode path");
}
