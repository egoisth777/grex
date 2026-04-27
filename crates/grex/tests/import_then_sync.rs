//! feat-v1.1.0 e2e — `grex import` + `grex sync` against the flat-sibling
//! child layout that the pack-spec has always advertised.
//!
//! This test reproduces the user's real-world failure case:
//!
//! ```text
//! tempdir/
//! ├── REPOS.json                  (legacy registry — 3 children)
//! ├── .grex/pack.yaml             (parent meta pack — hand-written)
//! ├── child-a/.grex/pack.yaml     (child pack — flat sibling, NOT under .grex/workspace/)
//! ├── child-b/.grex/pack.yaml
//! └── child-c/.grex/pack.yaml
//! ```
//!
//! Pre-v1.1.0 sync looked for children at `<root>/.grex/workspace/<name>/.grex/pack.yaml`
//! and failed with "manifest not found". v1.1.0 resolves at `<root>/<name>/.grex/pack.yaml`
//! — flat siblings — and walks every child without `--workspace` override.
//!
//! Note: `grex init` is still stubbed in v1.1.0, so the test hand-writes the parent
//! `pack.yaml` directly. Once `init` lands the test can switch to invoking it.

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

/// Run `git` in `cwd` and assert success. Mirrors the helper in
/// `crates/grex/tests/sync_e2e.rs`.
fn run_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(cwd).output().expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Seed an empty bare repo with an initial commit containing a
/// pre-built `.grex/pack.yaml`. Returns the bare-repo path the parent
/// will clone from.
fn seed_bare(tmp: &Path, name: &str, pack_yaml: &str) -> PathBuf {
    init_git_identity();
    let work = tmp.join(format!("seed-{name}-work"));
    fs::create_dir_all(work.join(".grex")).unwrap();
    fs::write(work.join(".grex/pack.yaml"), pack_yaml).unwrap();
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    run_git(&work, &["add", "-A"]);
    run_git(&work, &["commit", "-q", "-m", "seed"]);

    let bare = tmp.join(format!("{name}.git"));
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

/// Set of three children laid out as flat siblings of the parent root.
struct Layout {
    _tmp: TempDir,
    root: PathBuf,
    child_names: [&'static str; 3],
}

fn build_layout() -> Layout {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let names: [&'static str; 3] = ["alpha", "beta", "gamma"];

    // Sink directory each child mkdir's into so the walk emits a
    // visible action step per child (declarative pack with one mkdir).
    let sink = tmp_path.join("sink");
    fs::create_dir_all(&sink).unwrap();

    // Build a bare repo per child and clone it into its flat-sibling
    // slot under `root`.
    let root = tmp_path.join("root");
    fs::create_dir_all(&root).unwrap();

    let mut clone_urls: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let mkdir_path = sink.join(format!("made-{name}")).to_string_lossy().replace('\\', "/");
        let child_yaml = format!(
            "schema_version: \"1\"\nname: {name}\ntype: declarative\nactions:\n  - mkdir:\n      path: {mkdir_path}\n",
        );
        let bare = seed_bare(&tmp_path, name, &child_yaml);
        let url = file_url_from_path(&bare);
        // Clone child into root/<name> — exactly the layout grex sync expects
        // post-v1.1.0 (flat siblings of the parent pack root).
        run_git(&root, &["clone", "-q", url.as_str(), root.join(name).to_str().unwrap()]);
        clone_urls.push(url);
    }

    // Hand-write the parent meta pack.yaml. `grex init` is stubbed, so
    // the test cannot rely on it to produce this file.
    let mut parent_yaml =
        String::from("schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n");
    for (name, url) in names.iter().zip(clone_urls.iter()) {
        parent_yaml.push_str(&format!("  - url: {url}\n    path: {name}\n"));
    }
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    // Write a REPOS.json that mirrors the flat-sibling layout for the
    // import step. `grex import` writes `grex.jsonl` rows; the parent
    // `pack.yaml` is independent of that registry.
    let repos_json = format!(
        r#"[
  {{"url": "{}", "path": "{}"}},
  {{"url": "{}", "path": "{}"}},
  {{"url": "{}", "path": "{}"}}
]"#,
        clone_urls[0], names[0], clone_urls[1], names[1], clone_urls[2], names[2],
    );
    fs::write(root.join("REPOS.json"), repos_json).unwrap();

    Layout { _tmp: tmp, root, child_names: names }
}

/// Same shape as `build_layout` but does NOT pre-clone children. The
/// parent's `children[].url` points at the bare repos so the walker
/// must clone them itself on first sync. Returns the layout plus the
/// list of bare-repo URLs so callers can reuse them in assertions.
fn build_layout_no_preclones() -> Layout {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let names: [&'static str; 3] = ["alpha", "beta", "gamma"];
    let sink = tmp_path.join("sink");
    fs::create_dir_all(&sink).unwrap();

    let root = tmp_path.join("root");
    fs::create_dir_all(&root).unwrap();

    let mut clone_urls: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let mkdir_path = sink.join(format!("made-{name}")).to_string_lossy().replace('\\', "/");
        let child_yaml = format!(
            "schema_version: \"1\"\nname: {name}\ntype: declarative\nactions:\n  - mkdir:\n      path: {mkdir_path}\n",
        );
        let bare = seed_bare(&tmp_path, name, &child_yaml);
        clone_urls.push(file_url_from_path(&bare));
        // NB: deliberately do NOT clone into `root/<name>` — the
        // walker must do it on first sync.
    }

    // Parent meta pack listing the children.
    let mut parent_yaml =
        String::from("schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n");
    for (name, url) in names.iter().zip(clone_urls.iter()) {
        parent_yaml.push_str(&format!("  - url: {url}\n    path: {name}\n"));
    }
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    Layout { _tmp: tmp, root, child_names: names }
}

#[test]
fn sync_clones_children_into_flat_sibling_slots_on_first_run() {
    let layout = build_layout_no_preclones();

    // Sanity — children are NOT pre-cloned.
    for name in layout.child_names {
        assert!(
            !layout.root.join(name).exists(),
            "fixture must NOT pre-create flat-sibling `{name}`",
        );
    }

    // First sync: walker clones each child into its flat-sibling slot.
    grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    for name in layout.child_names {
        assert!(
            layout.root.join(name).join(".git").is_dir(),
            "child `{name}` must be cloned into flat-sibling slot on first sync",
        );
        assert!(
            layout.root.join(name).join(".grex/pack.yaml").is_file(),
            "child `{name}`'s pack.yaml must land at flat-sibling slot",
        );
    }
    // Legacy slot must NEVER be created.
    assert!(
        !layout.root.join(".grex").join("workspace").exists(),
        "v1.1.0 fresh sync must NOT create `.grex/workspace/`",
    );

    // Second sync: idempotent — children already exist, no halts.
    grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
}

#[test]
fn sync_with_workspace_override_routes_children_to_override_dir() {
    // `--workspace <override>` puts children under <override> instead
    // of the parent pack root. The flag still accepts an explicit
    // path post-v1.1.0; only the *default* changed.
    let layout = build_layout_no_preclones();
    let override_ws = layout.root.parent().unwrap().join("override-ws");
    fs::create_dir_all(&override_ws).unwrap();

    grex()
        .current_dir(&layout.root)
        .args(["sync", ".", "--workspace", override_ws.to_str().unwrap()])
        .assert()
        .success();

    // Children must land under the override workspace, NOT under the
    // pack root.
    for name in layout.child_names {
        assert!(
            override_ws.join(name).join(".git").is_dir(),
            "child `{name}` must be cloned into --workspace override `{}`",
            override_ws.display(),
        );
        assert!(
            !layout.root.join(name).exists(),
            "child `{name}` must NOT appear under pack root when --workspace overrides",
        );
    }
    // Workspace lock lives under the override, not the pack root.
    assert!(
        override_ws.join(".grex.sync.lock").exists(),
        "workspace lock must live under the --workspace override",
    );
}

#[test]
fn import_writes_manifest_and_sync_walks_flat_siblings() {
    let layout = build_layout();
    let manifest = layout.root.join("grex.jsonl");
    let repos_json = layout.root.join("REPOS.json");

    // Step 1: `grex import` — writes the manifest at the parent root.
    grex()
        .args([
            "import",
            "--from-repos-json",
            repos_json.to_str().unwrap(),
            "--manifest",
            manifest.to_str().unwrap(),
        ])
        .assert()
        .success();
    assert!(manifest.exists(), "import must produce grex.jsonl");
    let manifest_lines: Vec<String> =
        fs::read_to_string(&manifest).unwrap().lines().map(str::to_string).collect();
    assert_eq!(manifest_lines.len(), 3, "one row per child");

    // Step 2: `grex sync .` against the parent pack root. Without the
    // v1.1.0 fix this fails with `pack manifest not found at
    // .grex/workspace/<child>/.grex/pack.yaml`. Post-fix it walks every
    // child as a flat sibling.
    let assertion = grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("manifest not found"),
        "sync stderr must not mention 'manifest not found': {stderr}"
    );
    for name in layout.child_names {
        assert!(
            stdout.contains(name),
            "sync stdout must mention child `{name}`; got: {stdout}\n--- stderr ---\n{stderr}"
        );
    }

    // Step 3: re-run sync — must be idempotent (no errors, no halts).
    grex().current_dir(&layout.root).args(["sync", "."]).assert().success();

    // Step 4: lockfile lives at the pack root, NOT under
    // `.grex/workspace/`. The whole `.grex/workspace/` directory must
    // never have been created.
    let legacy_workspace = layout.root.join(".grex").join("workspace");
    assert!(
        !legacy_workspace.exists(),
        "v1.1.0 must NOT create .grex/workspace/ — found one at {}",
        legacy_workspace.display(),
    );
    let lockfile = layout.root.join(".grex.sync.lock");
    assert!(
        lockfile.exists(),
        "workspace lock must live at <pack_root>/.grex.sync.lock; expected: {}",
        lockfile.display(),
    );
}
