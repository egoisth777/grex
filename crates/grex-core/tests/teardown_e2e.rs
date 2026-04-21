//! End-to-end integration tests for M5-2 teardown + gitignore.
//!
//! Covers the full install → teardown round-trip at three layers:
//!
//! 1. Single-pack declarative / scripted packs dispatched through the
//!    public `grex_core::sync::{run, teardown}` driver.
//! 2. Meta packs with multiple declarative children driven through
//!    `MetaPlugin` directly (the sync driver's git-backed walker rejects
//!    local-only child references — same constraint as
//!    `meta_recursion.rs`).
//! 3. `x-gitignore` managed-block lifecycle at the workspace
//!    `.gitignore` (upsert on install, retire on teardown, multi-pack
//!    coexistence).
//!
//! Fixture notes:
//!
//! * Tier-1 actions do not include `copy` or `write`; `mkdir` is used as
//!   the materialising action throughout — its side-effect (a directory)
//!   is observable by `Path::is_dir()` and reversible via the
//!   `mkdir→rmdir` auto-reverse mapping exercised in unit tests.
//! * The scripted teardown test is Unix-only: seeding a functional
//!   `.ps1` hook requires `pwsh` on PATH, which Windows CI does not
//!   guarantee. The Windows branch is covered by
//!   `pack_type_dispatch::scripted_pack_missing_hook_is_a_noop`.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use grex_core::execute::{ExecCtx, MetaVisitedSet};
use grex_core::pack::{self, PackManifest};
use grex_core::plugin::pack_type::MetaPlugin;
use grex_core::plugin::{PackTypePlugin, PackTypeRegistry};
use grex_core::sync::{run, teardown, SyncOptions};
use grex_core::{Registry, VarEnv};
use tempfile::TempDir;
use tokio::runtime::Builder;

// ------------------------------------------------------------ helpers

fn write_pack(dir: &Path, yaml: &str) -> PathBuf {
    fs::create_dir_all(dir.join(".grex")).unwrap();
    let p = dir.join(".grex").join("pack.yaml");
    fs::write(&p, yaml).unwrap();
    p
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

fn options(workspace: PathBuf) -> SyncOptions {
    SyncOptions::new().with_validate(true).with_workspace(Some(workspace))
}

fn parse(s: &str) -> PackManifest {
    pack::parse(s).expect("fixture must parse")
}

fn new_visited() -> MetaVisitedSet {
    Arc::new(Mutex::new(HashSet::new()))
}

fn rt() -> tokio::runtime::Runtime {
    Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

// ------------------------------------------------------------ 1. declarative

/// Install materialises a directory; teardown removes it via the
/// `mkdir→rmdir` auto-reverse mapping (R-M5-09).
#[test]
fn install_then_teardown_declarative_pack_removes_materialised_dir() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let target = tmp_path.join("materialised");
    let yaml = format!(
        "schema_version: \"1\"\nname: d1\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
        fwd(&target)
    );
    let root = tmp_path.join("root");
    write_pack(&root, &yaml);
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let rep = run(&root, &options(workspace.clone())).expect("install ok");
    assert!(rep.halted.is_none(), "halted: {:?}", rep.halted);
    assert!(target.is_dir(), "install must materialise dir");

    let rep = teardown(&root, &options(workspace)).expect("teardown ok");
    assert!(rep.halted.is_none(), "halted: {:?}", rep.halted);
    assert!(!target.exists(), "teardown must remove materialised dir");
}

// ------------------------------------------------------------ 2. meta + children

/// Meta pack with two declarative children: install materialises both
/// dirs; teardown (via `MetaPlugin::teardown`) reverses children in LIFO
/// order and auto-reverses each child's actions.
#[test]
fn install_then_teardown_meta_pack_with_two_declarative_children() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let sink_a = tmp.path().join("sink-a");
    let sink_b = tmp.path().join("sink-b");
    let meta_yaml = "schema_version: \"1\"\nname: p\ntype: meta\nchildren:\n  - url: https://example.invalid/a\n    path: a\n  - url: https://example.invalid/b\n    path: b\n";
    write_pack(&root, meta_yaml);
    write_pack(
        &root.join("a"),
        &format!(
            "schema_version: \"1\"\nname: a\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
            fwd(&sink_a)
        ),
    );
    write_pack(
        &root.join("b"),
        &format!(
            "schema_version: \"1\"\nname: b\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
            fwd(&sink_b)
        ),
    );

    let pack = parse(meta_yaml);
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ExecCtx::new(&vars, &root, tmp.path())
        .with_registry(&action_reg)
        .with_pack_type_registry(&pack_type_reg)
        .with_visited_meta(&visited);

    rt().block_on(MetaPlugin.install(&ctx, &pack)).expect("install ok");
    assert!(sink_a.is_dir() && sink_b.is_dir(), "both children must install");

    // Fresh visited set for the teardown pass (cycle guard is per-run).
    let visited2 = new_visited();
    let ctx2 = ExecCtx::new(&vars, &root, tmp.path())
        .with_registry(&action_reg)
        .with_pack_type_registry(&pack_type_reg)
        .with_visited_meta(&visited2);
    rt().block_on(MetaPlugin.teardown(&ctx2, &pack)).expect("teardown ok");
    assert!(!sink_a.exists(), "child a must be torn down");
    assert!(!sink_b.exists(), "child b must be torn down");
}

// ------------------------------------------------------------ 3. scripted

/// Scripted pack with real `setup.sh` + `teardown.sh` hooks. Install
/// runs `setup.sh` which writes a sentinel file; teardown runs
/// `teardown.sh` which removes it. Unix-only: Windows CI may lack
/// `pwsh`; the missing-hook Windows branch is already covered in
/// `pack_type_dispatch.rs`.
#[cfg(unix)]
#[test]
fn scripted_pack_install_then_teardown_runs_both_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root = tmp_path.join("root");
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let hooks = root.join(".grex").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let sentinel = workspace.join("scripted.sentinel");

    let setup = hooks.join("setup.sh");
    fs::write(&setup, format!("#!/bin/sh\ntouch '{}'\n", sentinel.display())).unwrap();
    fs::set_permissions(&setup, fs::Permissions::from_mode(0o755)).unwrap();
    let td = hooks.join("teardown.sh");
    fs::write(&td, format!("#!/bin/sh\nrm -f '{}'\n", sentinel.display())).unwrap();
    fs::set_permissions(&td, fs::Permissions::from_mode(0o755)).unwrap();

    write_pack(&root, "schema_version: \"1\"\nname: s1\ntype: scripted\n");

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(sentinel.exists(), "setup.sh must have materialised sentinel");

    teardown(&root, &options(workspace)).expect("teardown ok");
    assert!(!sentinel.exists(), "teardown.sh must have removed sentinel");
}

// ------------------------------------------------------------ 4. gitignore upsert

/// Install with `x-gitignore` writes a workspace `.gitignore` whose
/// managed block contains the declared patterns and is marked with the
/// pack name (R-M5-08).
#[test]
fn gitignore_upsert_on_install_writes_managed_block() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        "schema_version: \"1\"\nname: gipack\ntype: declarative\nx-gitignore:\n  - target/\n  - \"*.log\"\n",
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    let gi = fs::read_to_string(workspace.join(".gitignore")).expect(".gitignore must exist");
    assert!(gi.contains("# >>> grex:gipack >>>"), "open marker missing: {gi}");
    assert!(gi.contains("# <<< grex:gipack <<<"), "close marker missing: {gi}");
    assert!(gi.contains("target/"), "pattern missing: {gi}");
    assert!(gi.contains("*.log"), "pattern missing: {gi}");
}

/// `apply_gitignore` is called exactly once per install — no duplicate
/// block (which would indicate both the declarative driver in
/// `run_declarative_actions` AND the plugin `install` ran apply).
/// Counting open markers is the cheapest observable proof; the
/// managed-block upserter is idempotent so multiple apply calls
/// would still leave one block, but a second apply site on the wrong
/// ordering could re-open the block elsewhere in the file.
#[test]
fn gitignore_applied_once_per_install() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        "schema_version: \"1\"\nname: once\ntype: declarative\nx-gitignore:\n  - once/\n",
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    let gi = fs::read_to_string(workspace.join(".gitignore")).unwrap();
    let opens = gi.matches("# >>> grex:once >>>").count();
    let closes = gi.matches("# <<< grex:once <<<").count();
    assert_eq!(opens, 1, "expected exactly one open marker, got {opens}: {gi}");
    assert_eq!(closes, 1, "expected exactly one close marker, got {closes}: {gi}");
}

// ------------------------------------------------------------ 5. gitignore teardown

/// Teardown removes the managed block and preserves any user-authored
/// content outside the block verbatim.
#[test]
fn gitignore_teardown_removes_block_preserves_user_content() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    // Seed user-authored content.
    let user_line = "user-authored-pattern/\n";
    fs::write(workspace.join(".gitignore"), user_line).unwrap();

    let root = tmp_path.join("root");
    write_pack(
        &root,
        "schema_version: \"1\"\nname: gip2\ntype: declarative\nx-gitignore:\n  - managed/\n",
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    let after_install = fs::read_to_string(workspace.join(".gitignore")).unwrap();
    assert!(after_install.contains("user-authored-pattern/"));
    assert!(after_install.contains("grex:gip2"));

    teardown(&root, &options(workspace.clone())).expect("teardown ok");
    let after = fs::read_to_string(workspace.join(".gitignore")).unwrap();
    assert!(!after.contains("grex:gip2"), "block must be gone: {after}");
    assert!(after.contains("user-authored-pattern/"), "user content preserved: {after}");
}

// ------------------------------------------------------------ 6. multi-pack coexistence

/// Two packs with separate `x-gitignore` extensions yield two managed
/// blocks. Tearing down one preserves the other's block verbatim.
#[test]
fn gitignore_multi_pack_coexistence_and_selective_teardown() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let root_a = tmp_path.join("root-a");
    let root_b = tmp_path.join("root-b");
    write_pack(
        &root_a,
        "schema_version: \"1\"\nname: packa\ntype: declarative\nx-gitignore:\n  - a-only/\n",
    );
    write_pack(
        &root_b,
        "schema_version: \"1\"\nname: packb\ntype: declarative\nx-gitignore:\n  - b-only/\n",
    );

    run(&root_a, &options(workspace.clone())).expect("install a ok");
    run(&root_b, &options(workspace.clone())).expect("install b ok");
    let both = fs::read_to_string(workspace.join(".gitignore")).unwrap();
    assert!(both.contains("grex:packa"), "A block missing: {both}");
    assert!(both.contains("grex:packb"), "B block missing: {both}");
    assert!(both.contains("a-only/"));
    assert!(both.contains("b-only/"));

    teardown(&root_b, &options(workspace.clone())).expect("teardown b ok");
    let after = fs::read_to_string(workspace.join(".gitignore")).unwrap();
    assert!(after.contains("grex:packa"), "A block must survive: {after}");
    assert!(after.contains("a-only/"));
    assert!(!after.contains("grex:packb"), "B block must be gone: {after}");
    assert!(!after.contains("b-only/"));
}

// ------------------------------------------------------------ 7. idempotent teardown

/// Running teardown twice is Ok: the second pass is a no-op because
/// the mkdir→rmdir auto-reverse already removed the dir, and
/// `remove_managed_block` on a file without the block is documented
/// no-op. The second run must not error.
#[test]
fn teardown_is_idempotent() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let target = tmp_path.join("idem-target");
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        &format!(
            "schema_version: \"1\"\nname: idem\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\nx-gitignore:\n  - idem/\n",
            fwd(&target)
        ),
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(target.is_dir());

    let first = teardown(&root, &options(workspace.clone())).expect("first teardown ok");
    assert!(first.halted.is_none(), "first halted: {:?}", first.halted);
    assert!(!target.exists(), "first teardown removes dir");

    let second = teardown(&root, &options(workspace)).expect("second teardown ok");
    assert!(second.halted.is_none(), "second halted: {:?}", second.halted);
}

// ------------------------------------------------------------ 8. auto-reverse order

/// mkdir [outer, inner] → teardown must auto-reverse as [rmdir inner,
/// rmdir outer]. Removing `outer` first would fail (non-empty dir); a
/// successful teardown with both gone proves reverse order.
#[test]
fn auto_reverse_deletes_in_reverse_order() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let outer = tmp_path.join("outer");
    let inner = outer.join("inner");
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        &format!(
            "schema_version: \"1\"\nname: ord\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n  - mkdir:\n      path: {}\n",
            fwd(&outer),
            fwd(&inner)
        ),
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(inner.is_dir());

    teardown(&root, &options(workspace)).expect("teardown ok");
    assert!(!inner.exists(), "inner must be gone");
    assert!(!outer.exists(), "outer must be gone — reverse order required");
}

// ------------------------------------------------------------ 9a. auto-reverse symlink

/// mkdir + symlink auto-reverse (R-M5-09): teardown must invert
/// symlink via `unlink` (remove the link), not leave it behind.
/// Unix-only — Windows symlink creation requires elevation / Developer
/// Mode, and the auto-reverse is fs-layer symmetric to
/// [`grex_core::SymlinkPlugin`] which is already platform-gated in its
/// own tests.
#[cfg(unix)]
#[test]
fn declarative_autoreverse_inverts_symlink() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let src_dir = tmp_path.join("real");
    let link = tmp_path.join("link");
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();

    let root = tmp_path.join("root");
    write_pack(
        &root,
        &format!(
            "schema_version: \"1\"\nname: ars\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n  - symlink:\n      src: {}\n      dst: {}\n",
            fwd(&src_dir),
            fwd(&src_dir),
            fwd(&link)
        ),
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(
        link.symlink_metadata().unwrap().file_type().is_symlink(),
        "install must create link"
    );

    teardown(&root, &options(workspace)).expect("teardown ok");
    assert!(link.symlink_metadata().is_err(), "auto-reverse must unlink");
}

// ------------------------------------------------------------ 9b. auto-reverse when

/// when-gated mkdir auto-reverse: teardown must recurse into
/// `when.actions` and invert each, preserving the gate. The `when`
/// gates on the current OS (always-true branch) so the inner mkdir
/// runs at install AND the inner rmdir runs at teardown.
#[test]
fn declarative_autoreverse_recurses_into_when() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let inner = tmp_path.join("gated-dir");
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    let os_tok = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    let root = tmp_path.join("root");
    write_pack(
        &root,
        &format!(
            "schema_version: \"1\"\nname: arw\ntype: declarative\nactions:\n  - when:\n      os: {}\n      actions:\n        - mkdir:\n            path: {}\n",
            os_tok,
            fwd(&inner)
        ),
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(inner.is_dir(), "when-gated install must materialise");

    teardown(&root, &options(workspace)).expect("teardown ok");
    assert!(!inner.exists(), "auto-reverse must recurse into when.actions");
}

// ------------------------------------------------------------ 10. explicit teardown

/// A pack with both `actions:` and an explicit `teardown:` block must
/// run the explicit block and NOT auto-reverse. Authoring the teardown
/// to remove a sentinel directory distinct from the install targets
/// makes the distinction observable: after teardown the install
/// targets remain (auto-reverse did not run), and the sentinel is
/// gone (explicit block did run).
#[test]
fn explicit_teardown_overrides_auto_reverse() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let keep = tmp_path.join("keep");
    let sentinel = tmp_path.join("sentinel");
    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).unwrap();
    // Pre-create `sentinel` so `rmdir` on it has something to remove.
    fs::create_dir_all(&sentinel).unwrap();

    let root = tmp_path.join("root");
    write_pack(
        &root,
        &format!(
            "schema_version: \"1\"\nname: exp\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\nteardown:\n  - rmdir:\n      path: {}\n",
            fwd(&keep),
            fwd(&sentinel)
        ),
    );

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(keep.is_dir(), "install materialised keep/");
    assert!(sentinel.is_dir(), "sentinel was pre-created");

    teardown(&root, &options(workspace)).expect("teardown ok");
    // Auto-reverse would have removed `keep/`; explicit block did not.
    assert!(keep.is_dir(), "explicit teardown must NOT auto-reverse mkdir keep/");
    assert!(!sentinel.exists(), "explicit teardown must remove sentinel/");
}
