//! v1.3.2 W3 — slash-separated `child.path:` regression suite.
//!
//! Pins the v1.2.0 contract from `pack-spec.md` §"declarative nested
//! paths": `child.path:` MAY be a relative path with `/` separators
//! (e.g. `tools/foo`, `courses/cpp/cpp-grammar`); each segment must
//! individually satisfy the bare-name regex. Absolute paths, `..`
//! segments, empty segments, and Windows-style backslash separators
//! remain rejected.
//!
//! Three test classes:
//!
//! 1. **Acceptance** — slash paths resolve correctly; mixed bare/slash
//!    siblings co-exist; lockfile keying preserves the slash form;
//!    distinct paths sharing the same pack name do not collide.
//! 2. **Rejection** — `..`, absolute, `tools/..`, empty segment, and
//!    symlink-escape continue to fail (and are caught BEFORE any
//!    backend mutation fires).
//! 3. **Walker invariant** — a slash-path child whose intermediate
//!    folder lacks `.grex/pack.yaml` does NOT cause the walker to
//!    treat that intermediate as a meta. The maintainer-locked rule
//!    "walker never recurses into a folder lacking `.grex/`" survives
//!    the v1.2.0 path relaxation.
//!
//! Mock infrastructure mirrors `walker_dry_run.rs` so no real network
//! / git subprocess fires.

#![allow(clippy::too_many_lines)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use grex_core::pack::{parse, PackManifest, PackValidationError};
use grex_core::tree::{sync_meta, SyncMetaOptions};
use grex_core::{ClonedRepo, GitBackend, GitError, PackLoader, TreeError};
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Mock infrastructure
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
    fn calls(&self) -> Vec<BackendCall> {
        self.calls.lock().unwrap().clone()
    }
}

impl GitBackend for InMemGit {
    fn name(&self) -> &'static str {
        "v1_3_2-w3-slash-paths-mock-git"
    }
    fn clone(
        &self,
        url: &str,
        dest: &Path,
        _ref: Option<&str>,
        _lock_ctx: grex_core::BackendLockCtx<'_>,
    ) -> Result<ClonedRepo, GitError> {
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

fn meta_with_children(name: &str, kids: &[(&str, &str)]) -> PackManifest {
    // Single-quote the `path:` scalar so edge inputs (`..`, `/abs`,
    // `tools//foo`, trailing `/`, embedded `\`) round-trip verbatim
    // through the YAML parser. Plain-scalar YAML would otherwise
    // coerce some shapes (e.g. trailing `/` is permitted but a leading
    // `/` lands as a plain scalar — quoting normalises the path source
    // for the validator).
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\nchildren:\n");
    for (url, path) in kids {
        yaml.push_str(&format!("  - url: {url}\n    path: '{path}'\n"));
    }
    parse(&yaml).expect("fixture yaml must parse")
}

/// Pre-materialise a sub-meta on disk so Phase 1 classifies it as
/// `PresentDeclared` and Phase 3 recurses into it. `pack.yaml` must
/// declare a name matching the LAST segment of the parent's
/// `child.path:` per the v1.2.0 `verify_child_name` rule.
fn write_sub_meta(dir: &Path, pack_yaml: &str) {
    std::fs::create_dir_all(dir.join(".grex")).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    std::fs::write(dir.join(".grex").join("pack.yaml"), pack_yaml).unwrap();
}

// ---------------------------------------------------------------------------
// 1. Acceptance — slash paths resolve correctly.
// ---------------------------------------------------------------------------

/// Single slash-segment child: `path: tools/foo` resolves to
/// `<root>/tools/foo` and the intermediate `tools/` is created by
/// `create_dir_all(parent)` on Phase 1's clone.
#[test]
fn slash_path_two_segments_resolves_dest() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new().with(
        root_dir.clone(),
        meta_with_children("root", &[("https://example.com/foo.git", "tools/foo")]),
    );
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let report =
        sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("slash-path walk must succeed");

    // Phase 1 fired exactly one clone, dest under `tools/foo`.
    let calls = backend.calls();
    let clones: Vec<&BackendCall> =
        calls.iter().filter(|c| matches!(c, BackendCall::Clone { .. })).collect();
    assert_eq!(clones.len(), 1, "expected one clone, got {calls:?}");
    if let BackendCall::Clone { dest, .. } = clones[0] {
        assert_eq!(dest, &root_dir.join("tools").join("foo"));
        assert!(dest.join(".git").exists(), "child dest must carry .git after clone");
    }
    // The phase1_classifications dest is the resolved slash path.
    let classes = &report.phase1_classifications;
    assert_eq!(classes.len(), 1);
    assert_eq!(classes[0].1, root_dir.join("tools").join("foo"));
}

/// Three-segment slash path: `path: courses/cpp/cpp-grammar` resolves
/// 3 levels deep; intermediate `courses/` and `courses/cpp/` are
/// created on demand.
#[test]
fn slash_path_three_segments_resolves_deep_dest() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new().with(
        root_dir.clone(),
        meta_with_children(
            "root",
            &[("https://example.com/grammar.git", "courses/cpp/cpp-grammar")],
        ),
    );
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("3-deep slash walk must succeed");

    let expected = root_dir.join("courses").join("cpp").join("cpp-grammar");
    assert!(expected.exists(), "3-deep dest must materialise: {}", expected.display());
    assert!(expected.join(".git").exists(), "leaf dest must carry .git");
    // Intermediate dirs created by create_dir_all(parent).
    assert!(root_dir.join("courses").is_dir());
    assert!(root_dir.join("courses").join("cpp").is_dir());
    // Intermediate dirs are NOT packs (no `.grex/`).
    assert!(!root_dir.join("courses").join(".grex").exists());
    assert!(!root_dir.join("courses").join("cpp").join(".grex").exists());
}

/// Mixed siblings: a bare-name child and a slash-path child in the
/// same parent manifest both resolve correctly without
/// cross-contamination.
#[test]
fn mixed_bare_and_slash_siblings_coexist() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new().with(
        root_dir.clone(),
        meta_with_children(
            "root",
            &[("https://example.com/bar.git", "bar"), ("https://example.com/baz.git", "tools/baz")],
        ),
    );
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("mixed walk must succeed");

    assert!(root_dir.join("bar").join(".git").exists(), "bare-name child must clone");
    assert!(
        root_dir.join("tools").join("baz").join(".git").exists(),
        "slash-path child must clone",
    );
}

/// Two children sharing `name:` (manifest pack name) at distinct
/// slash paths. Per `manifest.md §v1.2.0 keying`, path-keyed
/// distributed lockfile makes this legal; cycle-detection's
/// url+ref-based identity scheme does NOT collide on shared name.
#[test]
fn distinct_slash_paths_share_pack_name_no_collision() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    // Two children at `tools/foo` and `vendor/foo` with DISTINCT URLs
    // (the cycle detector keys on url+ref, so two distinct urls are
    // already two distinct packs regardless of name).
    let loader = InMemLoader::new().with(
        root_dir.clone(),
        meta_with_children(
            "root",
            &[
                ("https://example.com/tools-foo.git", "tools/foo"),
                ("https://example.com/vendor-foo.git", "vendor/foo"),
            ],
        ),
    );
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let report =
        sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("shared-name walk must succeed");
    assert!(report.errors.is_empty(), "no error expected, got {:?}", report.errors);
    assert!(root_dir.join("tools").join("foo").exists());
    assert!(root_dir.join("vendor").join("foo").exists());
}

/// Lockfile keying for a slash-path child: `LockEntry.path` carries
/// the canonical POSIX `/` form. Exercises the pure
/// `lockfile::write_entry_from_child` transform — no walker, no IO.
/// `ChildRef` is `#[non_exhaustive]`, so we build the value via
/// `pack::parse` rather than struct-literal construction.
#[test]
fn lockfile_entry_keying_preserves_slash_path() {
    use chrono::{TimeZone, Utc};
    use grex_core::lockfile::write_entry_from_child;

    let yaml = "schema_version: \"1\"\nname: parent\ntype: meta\nchildren:\n  - url: https://example.com/foo.git\n    path: tools/foo\n";
    let manifest = parse(yaml).expect("fixture must parse");
    let child = manifest.children.into_iter().next().expect("one child");
    let ts = Utc.with_ymd_and_hms(2026, 5, 3, 0, 0, 0).unwrap();
    let entry = write_entry_from_child(&child, "deadbeef", ts, "h", "1");
    // Per manifest.md §v1.2.0 keying: path is meta-relative POSIX with
    // forward slash, normalised at write-time. The transform is
    // platform-agnostic (no backslash on Windows).
    assert_eq!(entry.path, "tools/foo");
    assert!(!entry.path.contains('\\'), "POSIX `/` form, no Windows backslash");
}

// ---------------------------------------------------------------------------
// 2. Rejection — hostile / malformed paths still fail.
// ---------------------------------------------------------------------------

/// `..` is rejected at the validator gate (and surfaced as
/// `ChildPathInvalid` by the walker before any clone fires).
#[test]
fn dotdot_rejected() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[("https://x/a.git", "..")]));
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
        .expect_err("`..` path must be rejected");
    assert!(
        matches!(&err, TreeError::ChildPathInvalid { .. }),
        "expected ChildPathInvalid, got {err:?}",
    );
    // Critical: no clone fired.
    assert!(backend.calls().iter().all(|c| !matches!(c, BackendCall::Clone { .. })));
}

/// Absolute paths are rejected.
#[test]
fn absolute_path_rejected() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[("https://x/a.git", "/abs")]));
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
        .expect_err("absolute path must be rejected");
    assert!(
        matches!(&err, TreeError::ChildPathInvalid { .. }),
        "expected ChildPathInvalid, got {err:?}",
    );
    assert!(backend.calls().iter().all(|c| !matches!(c, BackendCall::Clone { .. })));
}

/// `tools/..` (a `..` segment buried inside a slash path) is rejected.
#[test]
fn embedded_dotdot_segment_rejected() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[("https://x/a.git", "tools/..")]));
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
        .expect_err("embedded `..` segment must be rejected");
    assert!(
        matches!(&err, TreeError::ChildPathInvalid { .. }),
        "expected ChildPathInvalid, got {err:?}",
    );
    assert!(backend.calls().iter().all(|c| !matches!(c, BackendCall::Clone { .. })));
}

/// Empty segment (consecutive `/`) is rejected.
#[test]
fn empty_segment_rejected() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let loader = InMemLoader::new()
        .with(root_dir.clone(), meta_with_children("root", &[("https://x/a.git", "tools//foo")]));
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let err = sync_meta(&root_dir, &backend, &loader, &opts, &[])
        .expect_err("empty segment must be rejected");
    assert!(
        matches!(&err, TreeError::ChildPathInvalid { .. }),
        "expected ChildPathInvalid, got {err:?}",
    );
    assert!(backend.calls().iter().all(|c| !matches!(c, BackendCall::Clone { .. })));
}

/// Validator-level direct check: `validate_plan` rejects each
/// hostile slash-path form. Exercises the rejection table directly so
/// future refactors of the walker dispatch don't accidentally relax
/// the contract.
#[test]
fn validator_rejection_table_for_slash_paths() {
    fn check(path: &str) -> Vec<PackValidationError> {
        // Single-quote the YAML scalar so edge characters (`/`, `\`,
        // trailing slash, leading dots) round-trip verbatim through
        // the parser without scalar-style coercion.
        let yaml = format!(
            "schema_version: \"1\"\nname: p\ntype: meta\nchildren:\n  - url: u\n    path: '{path}'\n"
        );
        let m = parse(&yaml).expect("yaml must parse");
        m.validate_plan().err().unwrap_or_default()
    }
    for bad in &[
        "..",
        "/abs",
        "tools/..",
        "tools//foo",
        "tools/",    // trailing slash
        "foo\\bar",  // Windows separator
        "Tools/foo", // uppercase segment
    ] {
        let errs = check(bad);
        assert!(!errs.is_empty(), "input {bad:?} must be rejected by validate_plan, got {errs:?}");
    }
    for ok in &["tools/foo", "courses/cpp/cpp-grammar", "vendor/sub/lib", "bar"] {
        let errs = check(ok);
        assert!(errs.is_empty(), "input {ok:?} must pass validate_plan, got {errs:?}");
    }
}

/// Symlink-escape: a pre-existing symlinked child destination is
/// refused by `dest_has_git_repo`'s symlink-metadata probe — the
/// public helper the walker delegates to when classifying a slot
/// before any synthesis or recursion. Returning `false` here ensures
/// the walker never treats a symlink-mounted dest as a registered
/// pack (closing the synthesis-redirection attack
/// pack-spec.md §"Synthesis policy" was retired to avoid).
///
/// On hosts where `symlink_dir` is unavailable (no Developer Mode on
/// Windows) the test no-ops — the protection is still in place.
#[test]
fn symlink_dest_refused_by_dest_has_git_repo() {
    use grex_core::tree::dest_has_git_repo;

    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    let outside = TempDir::new().unwrap();
    // Pre-create the dest path AS a symlink pointing outside the
    // workspace, with the target carrying a `.git/` directory.
    std::fs::create_dir_all(outside.path().join(".git")).unwrap();
    let dest = root_dir.join("link");
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(outside.path(), &dest).is_ok();
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(outside.path(), &dest).is_ok();
    if !made {
        // Host won't allow symlink creation — nothing to exercise.
        return;
    }
    // Even though the symlink target carries `.git/`, the walker's
    // boundary probe MUST refuse to recognise it as a registered
    // pack. The probe reads `symlink_metadata` (does NOT follow the
    // link) so the target's `.git/` is invisible.
    assert!(
        !dest_has_git_repo(&dest),
        "symlinked dest must NOT be classified as a git repo (boundary escape hazard)"
    );
}

// ---------------------------------------------------------------------------
// 3. Walker invariant — never recurse into a folder lacking `.grex/`.
// ---------------------------------------------------------------------------

/// Maintainer-locked invariant: when the walker resolves a slash-path
/// child at `tools/foo`, the intermediate `tools/` (a regular dir
/// created via `create_dir_all`) MUST NOT be treated as a meta. Phase
/// 3 recursion checks `<dest>/.grex/pack.yaml` — for `tools/` that
/// file does not exist, so the walker must NOT descend.
#[test]
fn walker_skips_intermediate_dir_lacking_grex() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    // The slash-path leaf IS itself a sub-meta with its own children,
    // pre-materialised on disk. The intermediate `tools/` is just a
    // regular dir — it has no pack.yaml, no .git.
    let leaf_dir = root_dir.join("tools").join("foo");
    write_sub_meta(&leaf_dir, "schema_version: \"1\"\nname: foo\ntype: meta\n");

    let loader = InMemLoader::new()
        .with(
            root_dir.clone(),
            meta_with_children("root", &[("https://example.com/foo.git", "tools/foo")]),
        )
        .with(leaf_dir.clone(), parse("schema_version: \"1\"\nname: foo\ntype: meta\n").unwrap());
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let report =
        sync_meta(&root_dir, &backend, &loader, &opts, &[]).expect("invariant walk must succeed");

    // Two metas visited: root + leaf. The intermediate `tools/` was
    // NEVER visited (no .grex/pack.yaml there).
    assert_eq!(
        report.metas_visited, 2,
        "walker must visit only declared meta dirs (root + leaf), not the intermediate scaffolding"
    );

    // Spot-check: NO classification entry references `<root>/tools` as
    // a meta_dir. Only `<root>` (parent of `tools/foo`) appears as a
    // meta_dir in phase1_classifications.
    let intermediate = root_dir.join("tools");
    for (meta_dir, _, _) in &report.phase1_classifications {
        assert_ne!(
            meta_dir, &intermediate,
            "walker must NOT treat intermediate `tools/` as a meta_dir",
        );
    }
}

/// Even if the operator sneakily creates a `pack.yaml` inside the
/// intermediate folder OUT-OF-BAND (not declared by any parent), the
/// walker must not pick it up. The walker is purely manifest-graph-
/// driven; undeclared `.grex/pack.yaml` directories are invisible.
/// This pins `walker.md §"purely manifest-graph-driven"`.
#[test]
fn walker_ignores_undeclared_grex_under_intermediate() {
    let tmp = TempDir::new().unwrap();
    let root_dir = tmp.path().to_path_buf();
    // Intermediate dir gets a stray pack.yaml that NO parent declares.
    // The walker must ignore it — `tools/` is not in root's children.
    let intermediate = root_dir.join("tools");
    std::fs::create_dir_all(intermediate.join(".grex")).unwrap();
    std::fs::write(
        intermediate.join(".grex").join("pack.yaml"),
        "schema_version: \"1\"\nname: tools\ntype: meta\n",
    )
    .unwrap();

    let leaf_dir = intermediate.join("foo");
    write_sub_meta(&leaf_dir, "schema_version: \"1\"\nname: foo\ntype: meta\n");

    let loader = InMemLoader::new()
        .with(
            root_dir.clone(),
            meta_with_children("root", &[("https://example.com/foo.git", "tools/foo")]),
        )
        .with(leaf_dir.clone(), parse("schema_version: \"1\"\nname: foo\ntype: meta\n").unwrap());
    let backend = InMemGit::new();
    let opts = SyncMetaOptions::default();

    let report = sync_meta(&root_dir, &backend, &loader, &opts, &[])
        .expect("undeclared-meta-ignored walk must succeed");

    // Only root + leaf visited. Stray `tools/.grex/pack.yaml` was NOT
    // loaded — `metas_visited` reflects manifest-declared frames only.
    assert_eq!(
        report.metas_visited, 2,
        "walker must NOT recurse into an undeclared `.grex/pack.yaml` under an intermediate dir"
    );
}
