//! v1.1.0 post-review B2 — auto-migration of legacy `.grex/workspace/`
//! layout left over from v1.0.x.
//!
//! Constructs a workspace built on the legacy default (3 children
//! materialised under `<pack_root>/.grex/workspace/<name>/`), runs
//! `grex sync`, and asserts:
//!
//! 1. Every legacy child relocates to its flat-sibling slot
//!    `<pack_root>/<name>/`.
//! 2. The orphan `.grex/workspace/.grex.sync.lock` is removed.
//! 3. The empty `.grex/workspace/` directory is rmdir'd.
//! 4. The sync proceeds end-to-end (no halts, all children walked).
//! 5. The migration is idempotent — a re-run sees no legacy directory
//!    and emits no `[migrated]` lines.
//!
//! Mirrors `import_then_sync.rs` for the fresh-flat-sibling case;
//! together they cover both upgrade modes.

mod common;

use common::grex;
use grex_core::git::gix_backend::file_url_from_path;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tempfile::TempDir;

fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
    });
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(cwd).output().expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Seed a bare repo whose initial commit ships a `.grex/pack.yaml`
/// declaring a single declarative pack with one mkdir action.
fn seed_bare(tmp: &Path, name: &str, sink: &Path) -> PathBuf {
    init_git_identity();
    let mkdir_path = sink.join(format!("made-{name}")).to_string_lossy().replace('\\', "/");
    let pack_yaml = format!(
        "schema_version: \"1\"\nname: {name}\ntype: declarative\nactions:\n  - mkdir:\n      path: {mkdir_path}\n",
    );
    let work = tmp.join(format!("seed-{name}-work"));
    fs::create_dir_all(work.join(".grex")).unwrap();
    fs::write(work.join(".grex/pack.yaml"), &pack_yaml).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    run_git(&work, &["add", "-A"]);
    run_git(&work, &["commit", "-q", "-m", "seed"]);

    let bare = tmp.join(format!("{name}.git"));
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

/// Build a v1.0.x-shaped workspace: parent meta pack at
/// `<root>/.grex/pack.yaml`; children pre-cloned under the legacy
/// `<root>/.grex/workspace/<name>/` slot. Also writes an orphan
/// workspace lock at `<root>/.grex/workspace/.grex.sync.lock`.
struct LegacyLayout {
    _tmp: TempDir,
    root: PathBuf,
    child_names: [&'static str; 3],
}

fn build_legacy_layout() -> LegacyLayout {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let names: [&'static str; 3] = ["alpha", "beta", "gamma"];
    let sink = tmp_path.join("sink");
    fs::create_dir_all(&sink).unwrap();

    let root = tmp_path.join("root");
    fs::create_dir_all(&root).unwrap();
    let legacy = root.join(".grex").join("workspace");
    fs::create_dir_all(&legacy).unwrap();

    let mut clone_urls: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let bare = seed_bare(&tmp_path, name, &sink);
        let url = file_url_from_path(&bare);
        // Clone INTO the legacy slot, mirroring v1.0.x layout.
        run_git(&legacy, &["clone", "-q", url.as_str(), legacy.join(name).to_str().unwrap()]);
        clone_urls.push(url);
    }

    // Parent meta pack — mirrors what import + a hand-edit would
    // produce. The new walker resolves children at flat-sibling slots
    // (post-migration the migrated dirs land there).
    let mut parent_yaml = String::from("schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n");
    for (name, url) in names.iter().zip(clone_urls.iter()) {
        parent_yaml.push_str(&format!("  - url: {url}\n    path: {name}\n"));
    }
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    // Stale lock left over from a v1.0.x sync that didn't get a chance
    // to clean up before the user upgraded.
    fs::write(legacy.join(".grex.sync.lock"), b"").unwrap();

    LegacyLayout { _tmp: tmp, root, child_names: names }
}

#[test]
fn auto_migrates_legacy_workspace_layout_on_first_sync() {
    let layout = build_legacy_layout();
    let legacy_root = layout.root.join(".grex").join("workspace");

    // Sanity — pre-sync we are in legacy shape.
    for name in layout.child_names {
        assert!(legacy_root.join(name).join(".git").is_dir(), "fixture must seed legacy `{name}`");
        assert!(
            !layout.root.join(name).exists(),
            "fixture must NOT pre-create flat-sibling `{name}`"
        );
    }
    assert!(legacy_root.join(".grex.sync.lock").is_file());

    // Run sync. This both auto-migrates AND walks the now-flat tree.
    let assertion = grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();

    // Migration log lines surface to stderr (text mode).
    for name in layout.child_names {
        assert!(
            stderr.contains("[migrated]")
                && stderr.contains(name),
            "stderr must announce migration for `{name}`; got:\n{stderr}",
        );
    }

    // Children landed at flat-sibling slots; legacy slots are gone.
    for name in layout.child_names {
        assert!(
            layout.root.join(name).join(".git").is_dir(),
            "child `{name}` must be at flat-sibling slot post-migration",
        );
        assert!(
            !legacy_root.join(name).exists(),
            "legacy slot `{name}` must be removed after rename",
        );
    }

    // Orphan lock removed; legacy workspace dir rmdir'd.
    assert!(
        !legacy_root.join(".grex.sync.lock").exists(),
        "orphan lock at legacy location must be removed by migration",
    );
    assert!(
        !legacy_root.exists(),
        "empty `.grex/workspace/` must be rmdir'd by migration cleanup",
    );

    // Sync proceeded — every child name appears in the step log.
    for name in layout.child_names {
        assert!(
            stdout.contains(name),
            "sync stdout must mention child `{name}`; got:\n{stdout}\n--- stderr ---\n{stderr}"
        );
    }

    // Re-run is idempotent: no legacy dir to migrate, no `[migrated]`
    // lines. Sync still completes successfully.
    let assertion2 =
        grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    let stderr2 = String::from_utf8(assertion2.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr2.contains("[migrated]"),
        "second sync must observe no legacy layout to migrate; got:\n{stderr2}",
    );
}

#[test]
fn migration_refuses_to_clobber_pre_existing_destination() {
    // Same legacy fixture, but pre-create a file at one of the
    // flat-sibling slots so migration is forced into the
    // SkippedDestOccupied branch. Both legacy and the placeholder must
    // remain on disk after sync; the user resolves manually.
    let layout = build_legacy_layout();
    let legacy_root = layout.root.join(".grex").join("workspace");

    // Plant a placeholder file at `<root>/alpha`. The migration MUST
    // refuse to rename onto it.
    let alpha_dest = layout.root.join(layout.child_names[0]);
    fs::write(&alpha_dest, b"user-data; do not clobber\n").unwrap();

    // Sync may fail because the parent manifest expects `alpha` to be
    // a directory with `.grex/pack.yaml`. We do not assert success
    // here — only that the migration left both sides untouched.
    let _ = grex().current_dir(&layout.root).args(["sync", "."]).assert();

    // Placeholder file is intact.
    let body = fs::read_to_string(&alpha_dest).unwrap();
    assert_eq!(body, "user-data; do not clobber\n", "user data must NOT be clobbered");

    // Legacy slot for `alpha` is preserved (not deleted).
    assert!(
        legacy_root.join(layout.child_names[0]).join(".git").is_dir(),
        "legacy `alpha` must remain on disk when destination is occupied",
    );
}
