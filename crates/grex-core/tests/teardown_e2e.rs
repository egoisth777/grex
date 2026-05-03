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
use grex_core::sync::{self, SyncError, SyncOptions};
use tokio_util::sync::CancellationToken;

/// feat-m7-1 stage 2: never-cancelled sentinel wrappers (`run` +
/// `teardown`) so the existing test bodies — which call them directly —
/// stay untouched. See `crates/grex-core/tests/concurrency.rs` for the
/// rationale; teardown gets the same treatment for parity.
fn run(
    pack_root: &std::path::Path,
    opts: &SyncOptions,
) -> Result<grex_core::sync::SyncReport, SyncError> {
    sync::run(pack_root, opts, &CancellationToken::new())
}

fn teardown(
    pack_root: &std::path::Path,
    opts: &SyncOptions,
) -> Result<grex_core::sync::SyncReport, SyncError> {
    sync::teardown(pack_root, opts, &CancellationToken::new())
}
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
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();

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

/// Scripted pack with real `sync.sh` + `teardown.sh` hooks. The public
/// [`run`] entrypoint dispatches scripted packs through
/// [`crate::plugin::pack_type::ScriptedPlugin::sync`], which invokes
/// `sync.sh` (the `setup.sh` hook is only reached via
/// [`crate::plugin::pack_type::ScriptedPlugin::install`], which no M5-2
/// public verb calls yet — the CLI's `install` verb wires in M5-3).
/// Teardown runs `teardown.sh` which removes the sentinel. Unix-only:
/// Windows CI may lack `pwsh`; the missing-hook Windows branch is
/// already covered in `pack_type_dispatch.rs`.
#[cfg(unix)]
#[test]
fn scripted_pack_install_then_teardown_runs_both_hooks() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root = tmp_path.join("root");
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();
    let hooks = root.join(".grex").join("hooks");
    fs::create_dir_all(&hooks).unwrap();
    let sentinel = workspace.join("scripted.sentinel");

    let sync_hook = hooks.join("sync.sh");
    fs::write(&sync_hook, format!("#!/bin/sh\ntouch '{}'\n", sentinel.display())).unwrap();
    fs::set_permissions(&sync_hook, fs::Permissions::from_mode(0o755)).unwrap();
    let td = hooks.join("teardown.sh");
    fs::write(&td, format!("#!/bin/sh\nrm -f '{}'\n", sentinel.display())).unwrap();
    fs::set_permissions(&td, fs::Permissions::from_mode(0o755)).unwrap();

    write_pack(&root, "schema_version: \"1\"\nname: s1\ntype: scripted\n");

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(sentinel.exists(), "sync.sh must have materialised sentinel");

    teardown(&root, &options(workspace)).expect("teardown ok");
    assert!(!sentinel.exists(), "teardown.sh must have removed sentinel");
}

// ------------------------------------------------------------ 4. gitignore non-mutation
//
// B12 v1.3.1: per-lifecycle `.gitignore` mutation REMOVED. The tests
// below were inverted from their v1.2.x counterparts: where they
// previously asserted that install / teardown wrote and removed a
// managed block, they now assert that the workspace `.gitignore` is
// left untouched by both lifecycles. The advisory finding is now
// surfaced by `grex doctor` (see `tests/doctor_advisory.rs`).

/// B12 v1.3.1: Install with `x-gitignore:` MUST NOT create the
/// workspace `.gitignore`. Inverted from the v1.2.x assertion.
#[test]
fn gitignore_install_does_not_create_file() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        "schema_version: \"1\"\nname: gipack\ntype: declarative\nx-gitignore:\n  - target/\n  - \"*.log\"\n",
    );
    let workspace = root.clone();

    run(&root, &options(workspace.clone())).expect("install ok");
    assert!(
        !workspace.join(".gitignore").exists(),
        "B12 v1.3.1: install MUST NOT create `.gitignore`. Found: {:?}",
        fs::read_to_string(workspace.join(".gitignore")),
    );
}

/// B12 v1.3.1: repeated install does not re-emit a managed block —
/// because no managed block is ever emitted now. Counts markers as a
/// belt-and-suspenders check.
#[test]
fn gitignore_no_markers_after_repeated_install() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        "schema_version: \"1\"\nname: once\ntype: declarative\nx-gitignore:\n  - once/\n",
    );
    let workspace = root.clone();

    run(&root, &options(workspace.clone())).expect("install ok");
    run(&root, &options(workspace.clone())).expect("install ok again");
    let gi = fs::read_to_string(workspace.join(".gitignore")).unwrap_or_default();
    assert_eq!(
        gi.matches("# >>> grex:once >>>").count(),
        0,
        "no managed-block open markers must be written: {gi}",
    );
    assert_eq!(
        gi.matches("# <<< grex:once <<<").count(),
        0,
        "no managed-block close markers must be written: {gi}",
    );
}

// ------------------------------------------------------------ 5. gitignore teardown

/// B12 v1.3.1: Teardown does not touch the workspace `.gitignore` —
/// no managed block was written by install, and teardown does not
/// retire one either. User-authored content is preserved byte-for-byte.
#[test]
fn gitignore_teardown_preserves_user_content_byte_for_byte() {
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path();
    let root = tmp_path.join("root");
    write_pack(
        &root,
        "schema_version: \"1\"\nname: gip2\ntype: declarative\nx-gitignore:\n  - managed/\n",
    );
    let workspace = root.clone();
    let user_line = "user-authored-pattern/\n";
    fs::write(workspace.join(".gitignore"), user_line).unwrap();
    let pre_bytes = fs::read(workspace.join(".gitignore")).unwrap();

    run(&root, &options(workspace.clone())).expect("install ok");
    let mid_bytes = fs::read(workspace.join(".gitignore")).unwrap();
    assert_eq!(pre_bytes, mid_bytes, "install must not mutate user `.gitignore`");
    assert!(
        !String::from_utf8_lossy(&mid_bytes).contains("grex:gip2"),
        "no managed block expected after install"
    );

    teardown(&root, &options(workspace.clone())).expect("teardown ok");
    let post_bytes = fs::read(workspace.join(".gitignore")).unwrap();
    assert_eq!(pre_bytes, post_bytes, "teardown must not mutate user `.gitignore`");
}

// ------------------------------------------------------------ 6. multi-pack coexistence

/// Two packs with separate `x-gitignore` extensions yield two managed
/// blocks. Tearing down one preserves the other's block verbatim.
///
/// v1.2.1 path (iii) NOTE: this test asserted that two distinct packs
/// (`root-a` and `root-b`) could share the SAME `--workspace` and produce
/// per-pack managed `.gitignore` blocks under one workspace `.gitignore`.
/// Under the new resolution model the workspace IS the meta_dir, so two
/// distinct packs cannot share one workspace by construction (each has
/// its own `.grex/pack.yaml`). The legacy multi-pack-per-workspace
/// coexistence semantic was retired with the prod `Walker::walk` removal.
/// Per-pack gitignore upsert is still covered by the single-pack tests
/// above; the cross-pack interaction in one shared workspace is no longer
/// expressible.
#[ignore = "v1.2.1 path (iii): workspace IS the meta_dir; multi-pack-per-workspace coexistence is retired"]
#[test]
fn gitignore_multi_pack_coexistence_and_selective_teardown() {
    // Body kept as historical reference. The assertions below would now
    // fail because run(root_b, --workspace=ws) reads `ws/.grex/pack.yaml`
    // (which is `packa`, not `packb`).
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
    let root = tmp_path.join("root");
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();
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
    let root = tmp_path.join("root");
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();
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

    let root = tmp_path.join("root");
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();
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
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink(), "install must create link");

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
    let os_tok = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };

    let root = tmp_path.join("root");
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();
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
    // Pre-create `sentinel` so `rmdir` on it has something to remove.
    fs::create_dir_all(&sentinel).unwrap();

    let root = tmp_path.join("root");
    // v1.2.1 path (iii): workspace IS the meta_dir.
    let workspace = root.clone();
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
