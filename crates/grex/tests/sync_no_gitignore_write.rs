//! v1.3.1 (B12) — `grex sync` does NOT mutate the parent meta-repo's
//! `.gitignore` byte-for-byte.
//!
//! Pre-v1.3.1, every PackTypePlugin lifecycle wrote a managed
//! `# >>> grex:<pack> >>>` block into `<workspace>/.gitignore` on
//! every sync. The maintainer decision (2026-05-02) was to REMOVE
//! that auto-mutation entirely: `grex sync` syncs sub-pack content
//! with remotes only and never writes to the parent meta-repo's
//! `.gitignore`. The advisory finding for "parent git index tracks
//! pack content" is surfaced by `grex doctor` instead.
//!
//! This test snapshots the parent `.gitignore` byte content pre-sync
//! and asserts byte equality post-sync against a fixture whose
//! manifest declares an `x-gitignore:` extension that previously
//! would have triggered the auto-write.

#![allow(clippy::too_many_lines)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use grex_core::sync::{self, SyncOptions};
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn run(
    pack_root: &Path,
    opts: &SyncOptions,
) -> Result<grex_core::sync::SyncReport, grex_core::sync::SyncError> {
    sync::run(pack_root, opts, &CancellationToken::new())
}

fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
    });
}

fn write_root(dir: &Path, yaml: &str) {
    fs::create_dir_all(dir.join(".grex")).unwrap();
    fs::write(dir.join(".grex/pack.yaml"), yaml).unwrap();
}

fn options(workspace: PathBuf) -> SyncOptions {
    SyncOptions::new().with_dry_run(false).with_validate(true).with_workspace(Some(workspace))
}

/// Build a fixture: a single declarative pack with `x-gitignore:` —
/// pre-v1.3.1 this would have caused `grex sync` to APPEND a managed
/// block to `<workspace>/.gitignore` containing `.grex-lock` plus the
/// authored patterns. Under v1.3.1 the file must be byte-equal
/// pre-and-post sync.
struct Fixture {
    _tmp: TempDir,
    root: PathBuf,
    workspace: PathBuf,
}

fn build_fixture(seed_gitignore: Option<&str>) -> Fixture {
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let yaml = concat!(
        "schema_version: \"1\"\n",
        "name: gi-immutable\n",
        "type: declarative\n",
        "x-gitignore:\n",
        "  - target/\n",
        "  - \"*.log\"\n",
    );
    write_root(&root, yaml);

    if let Some(initial) = seed_gitignore {
        fs::write(root.join(".gitignore"), initial).unwrap();
    }

    let workspace = root.clone();
    Fixture { _tmp: tmp, root, workspace }
}

fn read_bytes(p: &Path) -> Option<Vec<u8>> {
    fs::read(p).ok()
}

// --- The contract ---

/// `grex sync` against a fixture with `x-gitignore:` patterns must NOT
/// create a `.gitignore` file when none existed pre-sync.
#[test]
fn sync_does_not_create_gitignore_when_absent() {
    let f = build_fixture(None);
    let gi = f.workspace.join(".gitignore");
    assert!(!gi.exists(), "fixture invariant: no .gitignore pre-sync");

    let _report = run(&f.root, &options(f.workspace.clone())).expect("sync ok");

    assert!(
        !gi.exists(),
        "B12 v1.3.1: grex sync MUST NOT create a `.gitignore` in the workspace; \
         operator owns the file. Found: {:?}",
        read_bytes(&gi),
    );
}

/// `grex sync` against a fixture with a pre-existing user-authored
/// `.gitignore` must leave the file BYTE-EQUAL pre-and-post sync. This
/// is the strict regression guard against the v1.3.0 auto-mutation
/// bug where every sync appended (or refreshed) a managed block.
#[test]
fn sync_preserves_existing_gitignore_byte_for_byte() {
    let initial = "# user content\nnode_modules/\nthis-is-not-touched-by-grex\n";
    let f = build_fixture(Some(initial));
    let gi = f.workspace.join(".gitignore");
    let pre_bytes = read_bytes(&gi).expect("seed wrote .gitignore");
    assert_eq!(pre_bytes, initial.as_bytes(), "fixture invariant: seed bytes match",);

    let _report = run(&f.root, &options(f.workspace.clone())).expect("sync ok");

    let post_bytes = read_bytes(&gi).expect("file must still exist post-sync");
    assert_eq!(
        pre_bytes,
        post_bytes,
        "B12 v1.3.1: grex sync MUST NOT mutate the parent meta-repo's \
         `.gitignore`. Pre/post bytes diverged.\nPRE:  {:?}\nPOST: {:?}",
        String::from_utf8_lossy(&pre_bytes),
        String::from_utf8_lossy(&post_bytes),
    );
    // Belt-and-suspenders: the managed-block markers must be absent.
    let post_str = String::from_utf8_lossy(&post_bytes);
    assert!(
        !post_str.contains("# >>> grex:"),
        "managed-block markers must not appear in `.gitignore`: {post_str}",
    );
    assert!(
        !post_str.contains("# <<< grex:"),
        "managed-block markers must not appear in `.gitignore`: {post_str}",
    );
}

/// Running `grex sync` twice in a row must also leave the
/// `.gitignore` byte-equal across both runs (idempotency under the
/// v1.3.1 contract). This guards against subtle reintroductions of
/// per-lifecycle mutation that might be conditionally gated.
#[test]
fn repeat_sync_is_byte_idempotent_on_gitignore() {
    let initial = "user-line\n";
    let f = build_fixture(Some(initial));
    let gi = f.workspace.join(".gitignore");
    let pre_bytes = read_bytes(&gi).unwrap();

    let _ = run(&f.root, &options(f.workspace.clone())).expect("first sync ok");
    let mid_bytes = read_bytes(&gi).unwrap();
    assert_eq!(pre_bytes, mid_bytes, "first sync must be byte-equal");

    let _ = run(&f.root, &options(f.workspace.clone())).expect("second sync ok");
    let post_bytes = read_bytes(&gi).unwrap();
    assert_eq!(mid_bytes, post_bytes, "second sync must be byte-equal");
    assert_eq!(pre_bytes, post_bytes, "two-pass byte-equality holds");
}
