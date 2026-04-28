//! `grex ls` — read-only tree listing covering v1.1.1 synthetic packs.
//!
//! Three cases:
//! 1. Meta root + declarative child — both names render, no `~` marker,
//!    declarative pack-type suffix surfaces.
//! 2. Meta root + plain-git child (no `.grex/pack.yaml`, only `.git/`) —
//!    child line is prefixed with `~ ` and the suffix carries
//!    `(scripted, synthetic)`.
//! 3. Same as (2) under `--json` — JSON tree carries `synthetic: true`
//!    on the synthesised entry and `false` on the root.

mod common;

use common::grex;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tempfile::TempDir;

/// Same env-isolation block used by `import_then_sync.rs` so this test
/// stays deterministic across machines (no global git config leaks in).
fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
        let null_cfg = std::env::temp_dir().join("grex-test-empty-gitconfig");
        let _ = fs::write(&null_cfg, b"");
        std::env::set_var("GIT_CONFIG_GLOBAL", &null_cfg);
        std::env::set_var("GIT_CONFIG_SYSTEM", &null_cfg);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
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

/// Build a single workspace with a meta root and a configurable mix of
/// children. `declarative_kids` get a real `.grex/pack.yaml`;
/// `plain_git_kids` get a `.git/` only. Returns the workspace path
/// inside an owned `TempDir` so the caller controls the lifetime.
struct Layout {
    _tmp: TempDir,
    root: PathBuf,
}

fn build_layout(declarative_kids: &[&str], plain_git_kids: &[&str]) -> Layout {
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join(".grex")).unwrap();

    // Hand-write the meta root so the walker has a graph to traverse.
    // URLs are placeholders — `ls` never clones, so they need not
    // resolve.
    let mut parent_yaml =
        String::from("schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n");
    for name in declarative_kids.iter().chain(plain_git_kids.iter()) {
        parent_yaml
            .push_str(&format!("  - url: https://example.invalid/{name}.git\n    path: {name}\n"));
    }
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    // Declarative children: a real `.grex/pack.yaml`. No need to be a
    // git repo — `ls` does not depend on `.git/` for declared packs.
    for name in declarative_kids {
        let dir = root.join(name).join(".grex");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pack.yaml"),
            format!("schema_version: \"1\"\nname: {name}\ntype: declarative\n"),
        )
        .unwrap();
    }

    // Plain-git children: `.git/` present, NO `.grex/pack.yaml`. Use a
    // real `git init` so the on-disk shape is genuine (a stray `.git`
    // file would be a different code path).
    for name in plain_git_kids {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-q", "-b", "main"]);
    }

    Layout { _tmp: tmp, root }
}

#[test]
fn ls_meta_root_and_declarative_child_renders_plain_tree() {
    let layout = build_layout(&["warp-cfg"], &[]);
    let assertion = grex().current_dir(&layout.root).args(["ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("root (meta)"), "stdout must show root meta line; got:\n{stdout}");
    assert!(
        stdout.contains("warp-cfg (declarative)"),
        "stdout must show declarative child; got:\n{stdout}"
    );
    assert!(
        !stdout.contains("~ "),
        "no synthetic markers expected for declarative-only tree; got:\n{stdout}"
    );
    assert!(!stdout.contains("synthetic"), "no `synthetic` text expected; got:\n{stdout}");
}

#[test]
fn ls_plain_git_child_renders_synthetic_marker_in_tree_mode() {
    let layout = build_layout(&[], &["algo-leet"]);
    let assertion = grex().current_dir(&layout.root).args(["ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();

    assert!(stdout.contains("root (meta)"), "root meta line missing; got:\n{stdout}");
    assert!(
        stdout.contains("~ algo-leet (scripted, synthetic)"),
        "synthetic plain-git child must render with `~ ` prefix and `scripted, synthetic` suffix; got:\n{stdout}"
    );
}

#[test]
fn ls_plain_git_child_renders_synthetic_true_in_json_mode() {
    let layout = build_layout(&[], &["algo-leet"]);
    let assertion = grex().current_dir(&layout.root).args(["--json", "ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();

    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("ls --json invalid JSON: {e}\n{stdout}"));
    assert!(v.get("workspace").and_then(Value::as_str).is_some(), "workspace key missing");

    let tree = v.get("tree").and_then(Value::as_array).expect("tree must be an array");
    assert_eq!(tree.len(), 1, "expected single root entry");
    let root = &tree[0];
    assert_eq!(root.get("name").and_then(Value::as_str), Some("root"));
    assert_eq!(root.get("type").and_then(Value::as_str), Some("meta"));
    assert_eq!(root.get("synthetic").and_then(Value::as_bool), Some(false));

    let kids = root.get("children").and_then(Value::as_array).expect("children must be an array");
    assert_eq!(kids.len(), 1, "expected exactly one child");
    let child = &kids[0];
    assert_eq!(child.get("name").and_then(Value::as_str), Some("algo-leet"));
    assert_eq!(child.get("type").and_then(Value::as_str), Some("scripted"));
    assert_eq!(
        child.get("synthetic").and_then(Value::as_bool),
        Some(true),
        "plain-git child must carry synthetic=true"
    );
}

/// FIX-4 (round 2): a fresh meta-pack checkout where children are
/// declared but never synced MUST surface every declared child as a
/// placeholder rather than silently rendering an empty tree. Pre-fix
/// the loader's `ManifestNotFound` error was swallowed and the verb
/// reported `root (meta)` with no children.
#[test]
fn ls_unsynced_children_render_as_placeholders() {
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(
        root.join(".grex/pack.yaml"),
        "schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n  - url: https://example.invalid/alpha.git\n    path: alpha\n  - url: https://example.invalid/beta.git\n    path: beta\n",
    )
    .unwrap();
    // Note: NO `alpha/` or `beta/` directories. Fresh checkout.

    let assertion = grex().current_dir(&root).args(["ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("alpha (declared, unsynced)"),
        "alpha should render with unsynced suffix; got:\n{stdout}"
    );
    assert!(
        stdout.contains("beta (declared, unsynced)"),
        "beta should render with unsynced suffix; got:\n{stdout}"
    );

    // JSON mode: each child carries `unsynced: true`.
    let assertion = grex().current_dir(&root).args(["--json", "ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("ls --json invalid JSON: {e}\n{stdout}"));
    let kids = v.pointer("/tree/0/children").and_then(Value::as_array).expect("children array");
    assert_eq!(kids.len(), 2, "both declared children must appear; got:\n{stdout}");
    for kid in kids {
        assert_eq!(
            kid.get("unsynced").and_then(Value::as_bool),
            Some(true),
            "kid must carry unsynced=true; got:\n{kid}"
        );
    }
}

/// FIX-3 (round 2): a child whose on-disk `.grex/pack.yaml` is
/// corrupt YAML MUST render with an `[error: parse]` indicator
/// instead of being silently elided. The verb itself still exits 0
/// — `ls` is a read-only diagnostic surface.
#[test]
fn ls_corrupt_child_yaml_renders_parse_error_marker() {
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().to_path_buf();
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(
        root.join(".grex/pack.yaml"),
        "schema_version: \"1\"\nname: root\ntype: meta\nchildren:\n  - url: https://example.invalid/corrupt.git\n    path: corrupt\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("corrupt/.grex")).unwrap();
    // Garbage YAML: not a mapping.
    fs::write(root.join("corrupt/.grex/pack.yaml"), "::: not yaml ::: : :\n").unwrap();

    let assertion = grex().current_dir(&root).args(["ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    assert!(
        stdout.contains("corrupt") && stdout.contains("[error: parse]"),
        "corrupt child should render with `[error: parse]` indicator; got:\n{stdout}"
    );

    // JSON mode: child carries `error.kind == "parse"`.
    let assertion = grex().current_dir(&root).args(["--json", "ls"]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("ls --json invalid JSON: {e}\n{stdout}"));
    let kid = v.pointer("/tree/0/children/0").expect("child entry");
    let err = kid.get("error").expect("error envelope present");
    assert_eq!(err.get("kind").and_then(Value::as_str), Some("parse"));
    assert!(
        err.get("message").and_then(Value::as_str).is_some_and(|s| !s.is_empty()),
        "error.message must be non-empty"
    );
}
