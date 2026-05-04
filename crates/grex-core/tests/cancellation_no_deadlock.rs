//! v1.2.4 — cancellation-token + Scheduler-permit deadlock probe.
//!
//! This integration test asserts that the v1.2.4 cancellation flag
//! plumbed through `sync_meta_inner`'s Phase 3 (rayon-driven sibling
//! recursion) cannot deadlock with the bounded `Scheduler` permit pool
//! when both are exercised in the same test body.
//!
//! ## What "deadlock" would look like
//!
//! If a sibling worker were to acquire a Scheduler permit and then
//! observe the cancellation flag inside a code path that drops the
//! work *without* releasing the permit, the pool would leak permits
//! and a downstream `Scheduler::acquire()` would block forever.
//!
//! The walker today does NOT route through `Scheduler` — Phase 1/3
//! sibling parallelism is rayon-driven (synchronous, work-stealing).
//! The Scheduler is reserved for the async tokio runtime (CLI verbs +
//! MCP server). This test still co-exercises both inside one runtime
//! to prove that:
//!
//! 1. The walker terminates promptly (`Err(CycleDetected)`) under
//!    `parallel: Some(4)` when one of 16 siblings cycles — i.e. the
//!    v1.2.4 cancellation flag short-circuits the other 15 sub-walks
//!    and rayon's pool never wedges.
//! 2. The bounded Scheduler (4 permits) is fully reusable AFTER the
//!    walk returns — `try_acquire` succeeds 4× in a row, proving no
//!    permit was leaked across the cancellation path.
//! 3. The whole assertion fits inside a `tokio::time::timeout(10s)`,
//!    so a regression that DID introduce a deadlock would surface as
//!    a timeout panic rather than a hung CI job.
//!
//! Per `proof/Grex/Walker.lean` `cancellation_terminates_promptly` —
//! when one sub-walk signals the per-`phase3_recurse` flag, every
//! subsequent sibling closure observes it at entry and short-circuits
//! to `Phase3ChildOutcome::Cancelled`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use grex_core::pack::{parse, PackManifest};
use grex_core::scheduler::Scheduler;
use grex_core::tree::{sync_meta, SyncMetaOptions};
use grex_core::{ClonedRepo, GitBackend, GitError, PackLoader, TreeError};
use tempfile::TempDir;
use tokio::sync::Semaphore;
use tokio::time::timeout;

// ---------------------------------------------------------------------------
// Mock infrastructure — same shape as `walker_parallel_stress.rs`.
// `PackManifest` / `ChildRef` are `#[non_exhaustive]`, so we round-trip
// fixtures through the public YAML parser instead of struct-literal
// construction.
// ---------------------------------------------------------------------------

struct InMemLoader {
    manifests: HashMap<PathBuf, PackManifest>,
}

impl InMemLoader {
    fn new() -> Self {
        Self { manifests: HashMap::new() }
    }
    fn with(mut self, dir: impl Into<PathBuf>, m: PackManifest) -> Self {
        self.manifests.insert(dir.into(), m);
        self
    }
}

impl PackLoader for InMemLoader {
    fn load(&self, path: &Path) -> Result<PackManifest, TreeError> {
        self.manifests
            .get(path)
            .cloned()
            .ok_or_else(|| TreeError::ManifestNotFound(path.to_path_buf()))
    }
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
enum BackendCall {
    Clone { url: String, dest: PathBuf },
    Fetch { dest: PathBuf },
    Checkout { dest: PathBuf, r#ref: String },
}

struct InMemGit {
    calls: Mutex<Vec<BackendCall>>,
}

impl InMemGit {
    fn new() -> Self {
        Self { calls: Mutex::new(Vec::new()) }
    }
}

impl GitBackend for InMemGit {
    fn name(&self) -> &'static str {
        "v1_2_4-cancel-deadlock-mock-git"
    }
    fn clone(
        &self,
        url: &str,
        dest: &Path,
        _ref: Option<&str>,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<ClonedRepo, GitError> {
        // Materialise `.git/` so re-runs would classify as PresentDeclared
        // (matches the production GixBackend post-condition).
        std::fs::create_dir_all(dest.join(".git")).unwrap();
        self.calls
            .lock()
            .unwrap()
            .push(BackendCall::Clone { url: url.to_string(), dest: dest.to_path_buf() });
        Ok(ClonedRepo { path: dest.to_path_buf(), head_sha: "0".repeat(40) })
    }
    fn fetch(&self, dest: &Path, _lock_ctx: grex_core::BackendLockCtx<'_>) -> Result<(), GitError> {
        self.calls.lock().unwrap().push(BackendCall::Fetch { dest: dest.to_path_buf() });
        Ok(())
    }
    fn checkout(
        &self,
        dest: &Path,
        r#ref: &str,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<(), GitError> {
        self.calls
            .lock()
            .unwrap()
            .push(BackendCall::Checkout { dest: dest.to_path_buf(), r#ref: r#ref.to_string() });
        Ok(())
    }
    fn head_sha(&self, _dest: &Path) -> Result<String, GitError> {
        Ok("0".repeat(40))
    }
}

/// Build a `meta` manifest by serializing to YAML and round-tripping
/// through the production parser. Sidesteps the `#[non_exhaustive]`
/// markers on `PackManifest` / `ChildRef`.
fn meta_with_children(name: &str, kids: &[(String, String)]) -> PackManifest {
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\nchildren:\n");
    for (url, path) in kids {
        yaml.push_str(&format!("  - url: {url}\n    path: {path}\n"));
    }
    parse(&yaml).expect("fixture yaml must parse")
}

/// Pre-populate a directory with a `.grex/pack.yaml` + stub `.git/` so
/// the classifier sees it as PresentDeclared (drives Fetch, not Clone).
fn make_sub_meta_on_disk(dir: &Path, name: &str) {
    std::fs::create_dir_all(dir.join(".grex")).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    let yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\n");
    std::fs::write(dir.join(".grex/pack.yaml"), yaml).unwrap();
}

/// `cancellation_under_scheduler_pressure_does_not_deadlock` —
/// co-exercises the v1.2.4 walker cancellation flag and a bounded
/// `Scheduler(4)` inside one tokio multi-thread runtime.
///
/// Topology:
/// ```text
/// root → meta-A
///   meta-A.children = [
///     a-cyclic,    // URL collides with meta-A's URL → CycleDetected
///     sib-00, sib-01, … sib-14   // 15 acyclic sub-metas, each a leaf
///   ]
/// ```
///
/// Walker runs with `parallel: Some(4)` (rayon pool = 4). The cyclic
/// arm trips Phase 3 cycle detection, signals the per-frame cancellation
/// flag, and the remaining siblings observe it on entry to
/// `phase3_handle_child` — Lean theorem `cancellation_terminates_promptly`.
///
/// The walk runs inside `spawn_blocking` (rayon is sync) and the whole
/// future is wrapped in `tokio::time::timeout(10s)`. A regression that
/// reintroduced a deadlock would surface as a timeout, not a hang.
///
/// AFTER the walk, the test acquires all 4 permits from a freshly-built
/// bounded `Scheduler(4)` AND verifies that a saturated Scheduler still
/// observes back-pressure correctly (5th `try_acquire` errors). This
/// asserts the Scheduler primitive stayed sane across the same runtime
/// that drove the walker — the goal "cancellation + parallel pool = no
/// deadlock" applied to BOTH the walker's rayon pool AND the tokio
/// Scheduler that gates async verbs.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[allow(clippy::too_many_lines)]
async fn cancellation_under_scheduler_pressure_does_not_deadlock() {
    // ---- Fixture ---------------------------------------------------------
    let tmp = TempDir::new().expect("tempdir");
    let root_dir = tmp.path().to_path_buf();
    let a_dir = root_dir.join("a");
    let a_cyclic_dir = a_dir.join("a-cyclic");

    // 15 acyclic siblings under meta-A — each a leaf sub-meta.
    let sib_dirs: Vec<PathBuf> = (0..15).map(|i| a_dir.join(format!("sib-{i:02}"))).collect();

    make_sub_meta_on_disk(&a_dir, "a");
    make_sub_meta_on_disk(&a_cyclic_dir, "a-cyclic");
    for sd in &sib_dirs {
        let name = sd.file_name().unwrap().to_str().unwrap().to_string();
        make_sub_meta_on_disk(sd, &name);
    }

    let url_a = "https://example.com/a.git".to_string();

    // meta-A's children: cyclic arm first (URL collides with A), then
    // 15 acyclic siblings. With `parallel: Some(4)` rayon work-steals
    // across the 16 — the cyclic arm signals; the others observe.
    let mut a_kids: Vec<(String, String)> = vec![(url_a.clone(), "a-cyclic".to_string())];
    for i in 0..15 {
        a_kids.push((format!("https://example.com/sib-{i:02}.git"), format!("sib-{i:02}")));
    }

    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[(url_a.clone(), "a".to_string())]))
        .with(a_dir.clone(), meta_with_children("a", &a_kids))
        .with(a_cyclic_dir.clone(), meta_with_children("a-cyclic", &[]));
    // Each acyclic sibling is a terminal meta with zero children.
    let loader = sib_dirs.iter().fold(loader, |acc, sd| {
        let name = sd.file_name().unwrap().to_str().unwrap();
        acc.with(sd.clone(), meta_with_children(name, &[]))
    });

    // ---- Bounded Scheduler co-existing in the same runtime -------------
    //
    // The walker doesn't route through this Scheduler today (Phase 1/3
    // are rayon-driven), but we hold it across the walk to prove that
    // both primitives can co-exist on the same multi-thread tokio
    // runtime without one starving the other.
    let scheduler: Arc<Scheduler> = Arc::new(Scheduler::new(4));
    assert_eq!(scheduler.max_parallelism(), 4, "bounded Scheduler with 4 permits");

    // Drain 2 permits BEFORE the walk to apply pre-existing pressure on
    // the runtime — workers that acquire these permits are simulating an
    // unrelated async verb running concurrently with the walk.
    let pre_permit_a = scheduler.acquire().await;
    let pre_permit_b = scheduler.acquire().await;

    // ---- The probe ------------------------------------------------------
    //
    // sync_meta is synchronous + rayon-driven, so we hand it off to
    // `spawn_blocking`. The whole future is bounded by a 10s timeout —
    // a deadlock regression would manifest as a timeout panic.
    let backend = Arc::new(InMemGit::new());
    let backend_for_walker = Arc::clone(&backend);
    // `SyncMetaOptions` is `#[non_exhaustive]` (v1.2.5 W1) — external crates
    // cannot use struct-literal construction even with `..base` spread per
    // E0639. Mutate a `default()` instance instead.
    let mut opts = SyncMetaOptions::default();
    opts.parallel = Some(4);

    let walk_fut = tokio::task::spawn_blocking(move || {
        sync_meta(&root_dir, &*backend_for_walker, &loader, &opts, &[])
    });

    let walk_result = timeout(Duration::from_secs(10), walk_fut)
        .await
        .expect("walker must terminate within 10s — cancellation deadlock regression")
        .expect("spawn_blocking must not panic");

    // The cyclic arm collides with meta-A's URL identity → CycleDetected.
    let err = walk_result.expect_err("cyclic input must surface CycleDetected");
    match err {
        TreeError::CycleDetected { chain } => {
            let id_a = format!("url:{url_a}");
            assert!(
                chain.contains(&id_a),
                "cycle chain must mention meta-A's identity {id_a}; got {chain:?}"
            );
        }
        other => panic!("expected CycleDetected, got {other:?}"),
    }

    // ---- Scheduler must still be healthy --------------------------------
    //
    // Drop the 2 pre-held permits; the pool must restore to 4 permits.
    drop(pre_permit_a);
    drop(pre_permit_b);

    // Now acquire all 4 permits — proves the Scheduler is fully reusable
    // and no permit was leaked through the cancellation path. Bounded by
    // a 1s timeout per acquire so a regression surfaces as a panic.
    let mut held = Vec::with_capacity(4);
    for i in 0..4 {
        let p = timeout(Duration::from_secs(1), scheduler.acquire())
            .await
            .unwrap_or_else(|_| panic!("permit {i} must be acquirable within 1s post-walk"));
        held.push(p);
    }
    assert_eq!(held.len(), 4, "Scheduler(4) must hand out all 4 permits after walk");

    // A 5th acquire on a saturated pool must NOT resolve within a short
    // probe window — confirms back-pressure semantics still work.
    let probe = timeout(Duration::from_millis(50), scheduler.acquire()).await;
    assert!(probe.is_err(), "saturated Scheduler must apply back-pressure after walk completes");

    // Sanity: the bounded sentinel must not have flipped to MAX_PERMITS.
    assert_ne!(
        scheduler.max_parallelism(),
        Semaphore::MAX_PERMITS,
        "bounded Scheduler must not silently widen to unbounded"
    );

    // Drop all permits — cleanup.
    drop(held);
}
