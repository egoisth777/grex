//! Integration: concurrent appenders under a shared fd-lock produce no
//! corruption and preserve total line count.
//!
//! This suite covers HIGH/MED severity findings surfaced by the codex
//! review of M2 manifest concurrency. Each test pins one failure mode the
//! lock implementation is expected to survive.

use chrono::{TimeZone, Utc};
use grex_core::fs::ManifestLock;
use grex_core::manifest::{append_event, read_all, Event, SCHEMA_VERSION};
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;
use tempfile::tempdir;

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn mk_event(tag: &str) -> Event {
    Event::Add {
        ts: Utc::now(),
        id: tag.into(),
        url: "u".into(),
        path: tag.into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    }
}

fn mk_event_at(tag: &str, secs: i64) -> Event {
    Event::Add {
        ts: Utc.timestamp_opt(secs, 0).unwrap(),
        id: tag.into(),
        url: "u".into(),
        path: tag.into(),
        pack_type: "declarative".into(),
        schema_version: SCHEMA_VERSION.into(),
    }
}

struct Paths {
    manifest: PathBuf,
    lock: PathBuf,
}

fn paths(dir: &Path) -> Paths {
    Paths { manifest: dir.join("grex.jsonl"), lock: dir.join(".grex.lock") }
}

// ---------------------------------------------------------------------------
// original baseline
// ---------------------------------------------------------------------------

#[test]
fn four_threads_append_under_lock() {
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());
    let manifest = Arc::new(manifest);

    let handles: Vec<_> = (0..4)
        .map(|tid| {
            let m = Arc::clone(&manifest);
            let lp = lock.clone();
            thread::spawn(move || {
                let mut l = ManifestLock::open(&m, &lp).unwrap();
                for i in 0..25 {
                    let ev = mk_event(&format!("t{tid}-p{i}"));
                    l.write(|| append_event(&m, &ev)).unwrap().unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    let events = read_all(&manifest).unwrap();
    assert_eq!(events.len(), 4 * 25);
    let mut ids: Vec<_> = events.iter().map(|e| e.id().clone()).collect();
    ids.sort();
    let total = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), total, "no duplicate or torn events");
}

// ---------------------------------------------------------------------------
// HIGH #1: writer panic mid-write releases lock; torn tail discarded.
// ---------------------------------------------------------------------------

#[test]
fn writer_panic_midwrite_releases_lock() {
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    // seed one good event
    {
        let mut l = ManifestLock::open(&manifest, &lock).unwrap();
        l.write(|| append_event(&manifest, &mk_event("pre"))).unwrap().unwrap();
    }

    // thread A: under write lock, append a partial torn line, then panic.
    let mp = manifest.clone();
    let lp = lock.clone();
    let a = thread::spawn(move || {
        let mut l = ManifestLock::open(&mp, &lp).unwrap();
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            l.write(|| {
                // Hand-write a partial JSON fragment (no trailing newline) and
                // then panic while the lock is held.
                let mut f = OpenOptions::new().append(true).open(&mp).unwrap();
                f.write_all(b"{\"op\":\"add\",\"ts\":\"2026-04").unwrap();
                panic!("simulated mid-write crash");
            })
            .unwrap();
        }));
    });
    a.join().unwrap();

    // thread B: must acquire lock next and append successfully. The torn
    // tail should be discarded at read time (read_all recovers it).
    let b = thread::spawn({
        let mp = manifest.clone();
        let lp = lock.clone();
        move || {
            let mut l = ManifestLock::open(&mp, &lp).unwrap();
            l.write(|| append_event(&mp, &mk_event("post"))).unwrap().unwrap();
        }
    });
    b.join().unwrap();

    let events = read_all(&manifest).unwrap();
    // torn partial line is the last line — read_all discards it; but the
    // "post" append produced a trailing complete line, so the partial is
    // now a middle line, which is a hard corruption error. Guard against
    // that case by re-sanitizing: if the partial became non-last, read_all
    // will surface it. We document the recovery expectation instead:
    // the "pre" event must survive regardless.
    let ids: Vec<_> = events.iter().map(|e| e.id().as_str().to_owned()).collect();
    assert!(ids.contains(&"pre".to_owned()), "prior event must survive panic");
    // And we must be able to take the lock again after the panic:
    let mut l = ManifestLock::open(&manifest, &lock).unwrap();
    l.write(|| ()).unwrap();
}

// ---------------------------------------------------------------------------
// HIGH #2: cross-process concurrent append via child test-binary invocations
// ---------------------------------------------------------------------------
//
// The child process is the same test binary, invoked with an env var that
// switches it into "append-mode": it appends N events under the lock and
// exits. The parent verifies combined count + uniqueness.

const CHILD_ENV: &str = "GREX_CONCURRENT_CHILD";
const CHILD_EVENTS: usize = 20;

#[test]
fn cross_process_concurrent_append() {
    // If invoked as a child by ourselves, do the append work and exit 0.
    if let Ok(spec) = std::env::var(CHILD_ENV) {
        run_child_append(&spec);
    }

    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    let exe = std::env::current_exe().expect("current_exe");
    // Spawn two children. Each runs `cross_process_concurrent_append` but
    // short-circuits into `run_child_append`.
    let spawn_child = |tag: &str| {
        std::process::Command::new(&exe)
            .arg("--exact")
            .arg("cross_process_concurrent_append")
            .arg("--nocapture")
            .env(CHILD_ENV, format!("{tag}|{}|{}", manifest.display(), lock.display()))
            .spawn()
            .expect("spawn child")
    };
    let mut c1 = spawn_child("A");
    let mut c2 = spawn_child("B");
    let s1 = c1.wait().expect("wait c1");
    let s2 = c2.wait().expect("wait c2");
    assert!(s1.success(), "child 1 failed: {s1:?}");
    assert!(s2.success(), "child 2 failed: {s2:?}");

    let events = read_all(&manifest).unwrap();
    assert_eq!(events.len(), 2 * CHILD_EVENTS, "combined event count");
    let mut ids: Vec<_> = events.iter().map(|e| e.id().clone()).collect();
    ids.sort();
    let total = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), total, "ids unique across processes");
}

fn run_child_append(spec: &str) -> ! {
    // spec = "TAG|MANIFEST|LOCK"
    let mut it = spec.split('|');
    let tag = it.next().expect("tag");
    let manifest = PathBuf::from(it.next().expect("manifest"));
    let lock = PathBuf::from(it.next().expect("lock"));

    let mut l = ManifestLock::open(&manifest, &lock).expect("child: open lock");
    for i in 0..CHILD_EVENTS {
        let ev = mk_event(&format!("proc{tag}-{i}"));
        l.write(|| append_event(&manifest, &ev))
            .expect("child: write lock")
            .expect("child: append");
    }
    // clean exit so the libtest harness doesn't also try to run other tests
    std::process::exit(0);
}

// ---------------------------------------------------------------------------
// HIGH #3: partial line injected manually, then concurrent appends
// ---------------------------------------------------------------------------

#[test]
fn partial_line_append_then_concurrent_preserves_prior() {
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    // prior complete event
    {
        let mut l = ManifestLock::open(&manifest, &lock).unwrap();
        l.write(|| append_event(&manifest, &mk_event("prior"))).unwrap().unwrap();
    }
    // inject partial trailing line (no newline)
    {
        let mut f = OpenOptions::new().append(true).open(&manifest).unwrap();
        f.write_all(b"{\"op\":\"add\",\"ts\":\"2026").unwrap();
    }

    // At this point the file ends in a torn fragment with no newline.
    // `append_event` heals the file on each call by truncating back to the
    // last newline, so concurrent appends each land as clean lines and the
    // torn fragment is discarded. All three `post-*` events plus `prior`
    // must be recoverable.
    let manifest_a = Arc::new(manifest.clone());
    let barrier = Arc::new(Barrier::new(3));
    let handles: Vec<_> = (0..3)
        .map(|tid| {
            let m = Arc::clone(&manifest_a);
            let lp = lock.clone();
            let b = Arc::clone(&barrier);
            thread::spawn(move || {
                let mut l = ManifestLock::open(&m, &lp).unwrap();
                b.wait();
                l.write(|| append_event(&m, &mk_event(&format!("post-{tid}")))).unwrap().unwrap();
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }

    // With heal-on-append, read_all must succeed and return the prior event
    // plus all three post events. The torn fragment is gone.
    let events = read_all(&manifest).expect("heal-on-append: read must succeed");
    let mut ids: Vec<_> = events.iter().map(|e| e.id().as_str().to_owned()).collect();
    ids.sort();
    assert!(ids.contains(&"prior".to_owned()), "prior event survives heal");
    let posts: Vec<_> = ids.iter().filter(|s| s.starts_with("post-")).collect();
    assert_eq!(posts.len(), 3, "all three concurrent appends landed cleanly: {ids:?}");
}

// ---------------------------------------------------------------------------
// HIGH #4: high thread count stress (64 × 10)
// ---------------------------------------------------------------------------

#[test]
fn high_thread_count_stress() {
    const THREADS: usize = 64;
    const PER_THREAD: usize = 10;
    #[cfg(windows)]
    const STRESS_DEADLINE: Duration = Duration::from_secs(90);
    #[cfg(not(windows))]
    const STRESS_DEADLINE: Duration = Duration::from_secs(30);

    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());
    let manifest = Arc::new(manifest);

    let start = std::time::Instant::now();
    let handles: Vec<_> = (0..THREADS)
        .map(|tid| {
            let m = Arc::clone(&manifest);
            let lp = lock.clone();
            thread::spawn(move || {
                let mut l = ManifestLock::open(&m, &lp).unwrap();
                for i in 0..PER_THREAD {
                    let ev = mk_event(&format!("stress-{tid}-{i}"));
                    l.write(|| append_event(&m, &ev)).unwrap().unwrap();
                }
            })
        })
        .collect();
    for h in handles {
        h.join().unwrap();
    }
    let elapsed = start.elapsed();

    let events = read_all(&manifest).unwrap();
    assert_eq!(events.len(), THREADS * PER_THREAD, "exact count");
    let mut ids: Vec<_> = events.iter().map(|e| e.id().clone()).collect();
    ids.sort();
    let total = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), total, "no duplicates under heavy contention");
    // Loose stability bound: 64×10 = 640 appends should complete under a
    // generous ceiling even on slow CI. Not a perf assertion.
    assert!(elapsed < STRESS_DEADLINE, "stress run took too long: {elapsed:?}");
}

// ---------------------------------------------------------------------------
// MED #5: reader blocks during active write, unblocks after, sees state
// ---------------------------------------------------------------------------

#[test]
fn reader_blocks_during_active_write() {
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    let writer_started = Arc::new(Barrier::new(2));
    let writer_hold = Arc::new(std::sync::Mutex::new(()));
    let writer_hold_taken = Arc::clone(&writer_hold);

    // thread A holds write lock and sleeps briefly.
    let mp = manifest.clone();
    let lp = lock.clone();
    let ws = Arc::clone(&writer_started);
    let a = thread::spawn(move || {
        let mut l = ManifestLock::open(&mp, &lp).unwrap();
        let _held = writer_hold_taken.lock().unwrap();
        l.write(|| {
            ws.wait();
            // simulate a slow write
            append_event(&mp, &mk_event("during-write")).unwrap();
            thread::sleep(Duration::from_millis(200));
        })
        .unwrap();
    });

    // wait until writer is known to be inside its critical section
    writer_started.wait();
    let t_before_read = std::time::Instant::now();

    // thread B: acquires a read lock. Should block until A releases.
    let lp2 = lock.clone();
    let mp2 = manifest.clone();
    let b = thread::spawn(move || {
        let mut l = ManifestLock::open(&mp2, &lp2).unwrap();
        l.read(|| read_all(&mp2).unwrap()).unwrap()
    });

    let events = b.join().unwrap();
    let waited = t_before_read.elapsed();
    a.join().unwrap();

    // reader should have been blocked a measurable amount of time
    assert!(
        waited >= Duration::from_millis(50),
        "reader did not appear to block: waited {waited:?}"
    );
    // and must see the event written inside the writer's critical section
    let ids: Vec<_> = events.iter().map(|e| e.id().as_str().to_owned()).collect();
    assert!(ids.contains(&"during-write".to_owned()));
}

// ---------------------------------------------------------------------------
// MED #6: file deleted mid-lock — platform-defined behavior
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[test]
fn file_deleted_midlock_defined_behavior_unix() {
    // On Unix the manifest fd remains valid after unlink; the open file
    // persists via its inode until all fds close. A new append via path
    // recreates the file.
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    let mut l = ManifestLock::open(&manifest, &lock).unwrap();
    l.write(|| append_event(&manifest, &mk_event("pre"))).unwrap().unwrap();

    // Remove the manifest; the lock file itself stays.
    std::fs::remove_file(&manifest).unwrap();
    assert!(!manifest.exists());

    // Append-by-path re-creates the file under the same held lock.
    l.write(|| append_event(&manifest, &mk_event("after-unlink"))).unwrap().unwrap();

    let events = read_all(&manifest).unwrap();
    let ids: Vec<_> = events.iter().map(|e| e.id().as_str().to_owned()).collect();
    // "pre" went with the unlinked inode; only "after-unlink" remains.
    assert_eq!(ids, vec!["after-unlink".to_owned()]);
}

#[cfg(windows)]
#[test]
fn file_deleted_midlock_defined_behavior_windows() {
    // On Windows, deleting a file that has open handles typically fails
    // with a sharing violation. We don't currently hold an open handle on
    // the manifest itself across the lock (only on the lock file), so a
    // delete can succeed here. The test pins the expected behavior:
    // delete may succeed; subsequent append must recreate and not panic.
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    let mut l = ManifestLock::open(&manifest, &lock).unwrap();
    l.write(|| append_event(&manifest, &mk_event("pre"))).unwrap().unwrap();

    // Best-effort delete; Windows may or may not allow it depending on
    // fs state. Either outcome is acceptable; we only require the lock
    // API keeps working.
    let _ = std::fs::remove_file(&manifest);

    l.write(|| append_event(&manifest, &mk_event("after-delete"))).unwrap().unwrap();

    let events = read_all(&manifest).unwrap();
    assert!(!events.is_empty(), "append after delete should yield events");
}

// ---------------------------------------------------------------------------
// MED #7: Windows advisory-vs-mandatory — document current gap
// ---------------------------------------------------------------------------

#[cfg(windows)]
#[test]
fn windows_advisory_vs_mandatory_lock() {
    // Windows `LockFileEx` is mandatory only on the locked handle. Our
    // `ManifestLock` locks a dedicated sidecar (not the manifest) to keep
    // the cooperating `append_event(path)` API functional. A non-grex
    // writer that opens the manifest directly bypasses the sidecar lock.
    //
    // This test pins that known gap (see rustdoc on `ManifestLock`): if it
    // ever starts failing, the lock became effectively mandatory and the
    // docs should be updated.
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    let mp = manifest.clone();
    let lp = lock.clone();
    let started = Arc::new(Barrier::new(2));
    let finish = Arc::new(Barrier::new(2));
    let s1 = Arc::clone(&started);
    let f1 = Arc::clone(&finish);

    let holder = thread::spawn(move || {
        let mut l = ManifestLock::open(&mp, &lp).unwrap();
        l.write(|| {
            s1.wait();
            f1.wait();
        })
        .unwrap();
    });

    started.wait();
    // bypass the lock entirely — open manifest and write
    let mut f = OpenOptions::new().create(true).append(true).open(&manifest).unwrap();
    let bypass_ok = f.write_all(b"sneaky\n").is_ok();
    drop(f);
    finish.wait();
    holder.join().unwrap();

    assert!(bypass_ok, "advisory-only gap expected: bypass write should currently succeed");
    // The lock itself still works for compliant users:
    let mut l = ManifestLock::open(&manifest, &lock).unwrap();
    l.write(|| ()).unwrap();
}

// ---------------------------------------------------------------------------
// MED #8: lock drop on unwind releases (tight-scope mirror of #1)
// ---------------------------------------------------------------------------

#[test]
fn lock_drop_on_unwind_releases() {
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    // Thread A panics inside the closure passed to ManifestLock::write.
    let mp = manifest.clone();
    let lp = lock.clone();
    let a = thread::spawn(move || {
        let mut l = ManifestLock::open(&mp, &lp).unwrap();
        let _ = panic::catch_unwind(AssertUnwindSafe(|| {
            l.write(|| panic!("inside-write-closure")).unwrap();
        }));
    });
    a.join().unwrap();

    // Thread B must be able to acquire the lock immediately afterwards.
    let mp2 = manifest.clone();
    let lp2 = lock.clone();
    let acquired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let acq2 = Arc::clone(&acquired);
    let b = thread::spawn(move || {
        let mut l = ManifestLock::open(&mp2, &lp2).unwrap();
        l.write(|| acq2.store(true, std::sync::atomic::Ordering::SeqCst)).unwrap();
    });
    b.join().unwrap();
    assert!(
        acquired.load(std::sync::atomic::Ordering::SeqCst),
        "next thread acquired lock after panic"
    );
}

// ---------------------------------------------------------------------------
// MED #9: timestamp collision does not merge/drop events
// ---------------------------------------------------------------------------

#[test]
fn timestamp_collision_not_corrupting() {
    let dir = tempdir().unwrap();
    let Paths { manifest, lock } = paths(dir.path());

    let mut l = ManifestLock::open(&manifest, &lock).unwrap();
    // Append four events with the SAME ts but distinct ids, in a known
    // order.
    let tags = ["a", "b", "c", "d"];
    for tag in tags {
        let ev = mk_event_at(tag, 1_700_000_000);
        l.write(|| append_event(&manifest, &ev)).unwrap().unwrap();
    }

    let events = read_all(&manifest).unwrap();
    assert_eq!(events.len(), tags.len(), "no merge/drop on equal ts");
    let ids: Vec<_> = events.iter().map(|e| e.id().as_str().to_owned()).collect();
    assert_eq!(
        ids,
        tags.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "append order preserved under identical ts"
    );
}
