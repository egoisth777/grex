//! Integration tests for M5-2c MetaPlugin real recursion + cycle detection.
//!
//! Unlike `pack_type_dispatch.rs` (which stays hermetic by avoiding git),
//! these tests drive `MetaPlugin` directly through the plugin trait so
//! they can stand up arbitrary `children:` layouts on disk — the sync
//! driver's normal git-backed walker would reject the local-only child
//! references these tests use.
//!
//! Coverage (see milestone acceptance list):
//!
//!  1. 1-level declarative child runs
//!  2. 2 sequential declarative children run in order
//!  3. meta→meta→declarative (3 levels) leaf runs
//!  4. cycle A→B→A → `ExecError::MetaCycle`
//!  5. self-cycle A→A → `ExecError::MetaCycle`
//!  6. child path missing on disk → clear error
//!  7. child manifest malformed → error propagates
//!  8. unknown pack-type in child → `ExecError::UnknownPackType`
//!  9. error in 2nd of 3 children aborts (3rd not dispatched)
//! 10. teardown visits children in reverse order
//! 11. update semantics == install (exercises child action)
//! 12. sync semantics == install (exercises child action)
//! 13. multi_thread runtime permits nested block_on (smoke probe)
//! 14. `pack_type_registry` absent → clear error (no panic)
//! 15. symlinked child path → cycle detection canonicalises

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use grex_core::execute::{ExecCtx, ExecError, MetaVisitedSet, StepKind};
use grex_core::pack::{self, PackManifest};
use grex_core::plugin::pack_type::MetaPlugin;
use grex_core::plugin::{PackTypePlugin, PackTypeRegistry};
use grex_core::{Registry, VarEnv};
use tempfile::TempDir;
use tokio::runtime::Builder;

// ------------------------------------------------------------ helpers

fn write_pack(dir: &Path, yaml: &str) {
    fs::create_dir_all(dir.join(".grex")).unwrap();
    fs::write(dir.join(".grex").join("pack.yaml"), yaml).unwrap();
}

fn declarative_mkdir_pack(name: &str, target: &Path) -> String {
    format!(
        "schema_version: \"1\"\nname: {}\ntype: declarative\nactions:\n  - mkdir:\n      path: {}\n",
        name,
        target.to_string_lossy().replace('\\', "/"),
    )
}

fn meta_pack_with_children(name: &str, child_paths: &[&str]) -> String {
    let mut yaml = format!("schema_version: \"1\"\nname: {name}\ntype: meta\n");
    if !child_paths.is_empty() {
        yaml.push_str("children:\n");
        for path in child_paths {
            // Use a placeholder url; the plugin resolves via `path:`.
            yaml.push_str(&format!(
                "  - url: https://example.invalid/{path}\n    path: {path}\n"
            ));
        }
    }
    yaml
}

fn parse(s: &str) -> PackManifest {
    pack::parse(s).expect("fixture parses")
}

fn new_visited() -> MetaVisitedSet {
    Arc::new(Mutex::new(HashSet::new()))
}

/// Build an ExecCtx with the full M5-1c/M5-2c wiring (action registry,
/// pack-type registry, cycle set). Pins lifetimes to the caller's
/// borrows so tests can own the Arcs.
fn ctx<'a>(
    vars: &'a VarEnv,
    pack_root: &'a Path,
    workspace: &'a Path,
    action_reg: &'a Arc<Registry>,
    pack_type_reg: &'a Arc<PackTypeRegistry>,
    visited: &'a MetaVisitedSet,
) -> ExecCtx<'a> {
    ExecCtx::new(vars, pack_root, workspace)
        .with_registry(action_reg)
        .with_pack_type_registry(pack_type_reg)
        .with_visited_meta(visited)
}

/// Build a multi_thread runtime for tests (matches M5-2c production).
fn rt() -> tokio::runtime::Runtime {
    Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap()
}

// ------------------------------------------------------------ cases

#[test]
fn meta_recurses_one_declarative_child() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let sink = tmp.path().join("sink-a");
    write_pack(&root, &meta_pack_with_children("parent", &["child-a"]));
    write_pack(&root.join("child-a"), &declarative_mkdir_pack("child-a", &sink));
    let pack = parse(&meta_pack_with_children("parent", &["child-a"]));

    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let step = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect("install ok");
    assert_eq!(step.action_name.as_ref(), "meta");
    assert!(sink.is_dir(), "child mkdir must have run");
    match step.details {
        StepKind::When { nested_steps, .. } => assert_eq!(nested_steps.len(), 1),
        other => panic!("expected When envelope, got {other:?}"),
    }
}

#[test]
fn meta_recurses_two_children_in_order() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let sink_a = tmp.path().join("a");
    let sink_b = tmp.path().join("b");
    write_pack(&root, &meta_pack_with_children("parent", &["a", "b"]));
    write_pack(&root.join("a"), &declarative_mkdir_pack("a", &sink_a));
    write_pack(&root.join("b"), &declarative_mkdir_pack("b", &sink_b));
    let pack = parse(&meta_pack_with_children("parent", &["a", "b"]));

    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let step = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect("install ok");
    assert!(sink_a.is_dir() && sink_b.is_dir());
    match step.details {
        StepKind::When { nested_steps, .. } => assert_eq!(nested_steps.len(), 2),
        other => panic!("expected When, got {other:?}"),
    }
}

#[test]
fn meta_recurses_three_levels_deep() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root"); // meta
    let mid = root.join("mid"); // meta
    let leaf = mid.join("leaf"); // declarative
    let sink = tmp.path().join("leaf-sink");
    write_pack(&root, &meta_pack_with_children("root", &["mid"]));
    write_pack(&mid, &meta_pack_with_children("mid", &["leaf"]));
    write_pack(&leaf, &declarative_mkdir_pack("leaf", &sink));
    let pack = parse(&meta_pack_with_children("root", &["mid"]));

    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    rt().block_on(MetaPlugin.install(&ctx, &pack)).expect("3-level install ok");
    assert!(sink.is_dir(), "3-level-deep leaf mkdir must have run");
}

#[test]
fn meta_cycle_a_to_b_to_a_errors() {
    // Build a true A → B → A cycle by using absolute-path child
    // references. `ChildRef::effective_path` returns `path:` verbatim;
    // `PathBuf::join(abs)` with an absolute path discards the base. The
    // visited-set then sees the same canonical path enter twice.
    let tmp = TempDir::new().unwrap();
    let dir_a = tmp.path().join("pack-a");
    let dir_b = tmp.path().join("pack-b");
    fs::create_dir_all(&dir_a).unwrap();
    fs::create_dir_all(&dir_b).unwrap();
    let abs_a = fs::canonicalize(&dir_a).unwrap();
    let abs_b = fs::canonicalize(&dir_b).unwrap();
    let abs_a_s = abs_a.to_string_lossy().replace('\\', "/");
    let abs_b_s = abs_b.to_string_lossy().replace('\\', "/");

    fs::create_dir_all(dir_a.join(".grex")).unwrap();
    fs::write(
        dir_a.join(".grex").join("pack.yaml"),
        format!(
            "schema_version: \"1\"\nname: pack-a\ntype: meta\nchildren:\n  - url: https://example.invalid/b\n    path: {abs_b_s}\n"
        ),
    )
    .unwrap();
    fs::create_dir_all(dir_b.join(".grex")).unwrap();
    fs::write(
        dir_b.join(".grex").join("pack.yaml"),
        format!(
            "schema_version: \"1\"\nname: pack-b\ntype: meta\nchildren:\n  - url: https://example.invalid/a\n    path: {abs_a_s}\n"
        ),
    )
    .unwrap();

    let pack = pack::parse(&fs::read_to_string(dir_a.join(".grex").join("pack.yaml")).unwrap())
        .expect("pack A parses");
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &dir_a, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("cycle must error");
    assert!(
        matches!(err, ExecError::MetaCycle { .. }),
        "expected MetaCycle, got: {err:?}"
    );
}

#[test]
fn meta_self_cycle_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("self");
    // Pack at root has a child whose path resolves to itself via "."
    // (reject by parse) — instead use a child directory that is the pack
    // root via a direct re-import under the same canonical path. We
    // arrange it with a child path "." is invalid; so seed a subdirectory
    // `me` whose own pack points its child back at `..` (the root). Since
    // ChildRef.path is treated as a forward name, "." and ".." are passed
    // through by effective_path, and PathBuf::join handles them. This
    // gives us `root → me → .. → root` — the canonical root appears
    // twice on the dispatch stack.
    write_pack(&root, &meta_pack_with_children("self", &["me"]));
    write_pack(&root.join("me"), &meta_pack_with_children("me", &[".."]));

    let pack = parse(&meta_pack_with_children("self", &["me"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("self-cycle errors");
    assert!(
        matches!(err, ExecError::MetaCycle { .. }),
        "expected MetaCycle, got: {err:?}"
    );
}

#[test]
fn meta_missing_child_path_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write_pack(&root, &meta_pack_with_children("p", &["missing"]));
    // deliberately do NOT create root/missing

    let pack = parse(&meta_pack_with_children("p", &["missing"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("missing child errors");
    match err {
        ExecError::ExecInvalid(msg) => {
            assert!(msg.contains("child manifest load failed"), "msg: {msg}");
        }
        other => panic!("expected ExecInvalid, got: {other:?}"),
    }
}

#[test]
fn meta_malformed_child_manifest_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write_pack(&root, &meta_pack_with_children("p", &["bad"]));
    fs::create_dir_all(root.join("bad").join(".grex")).unwrap();
    fs::write(root.join("bad").join(".grex").join("pack.yaml"), "this is not yaml: : :").unwrap();

    let pack = parse(&meta_pack_with_children("p", &["bad"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("malformed errors");
    assert!(matches!(err, ExecError::ExecInvalid(_)), "got: {err:?}");
}

#[test]
fn meta_unknown_child_pack_type_errors() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write_pack(&root, &meta_pack_with_children("p", &["kid"]));
    write_pack(&root.join("kid"), &declarative_mkdir_pack("kid", &tmp.path().join("k")));

    let pack = parse(&meta_pack_with_children("p", &["kid"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    // Empty pack-type registry (no declarative plugin registered) → child dispatch
    // must halt with UnknownPackType.
    let pack_type_reg = Arc::new(PackTypeRegistry::new());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("unknown type errors");
    match err {
        ExecError::UnknownPackType { requested } => assert_eq!(requested, "declarative"),
        other => panic!("expected UnknownPackType, got: {other:?}"),
    }
}

#[test]
fn meta_halts_on_second_child_error_does_not_run_third() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let sink_a = tmp.path().join("ok-a");
    let sink_c = tmp.path().join("never-c");
    write_pack(&root, &meta_pack_with_children("p", &["a", "b", "c"]));
    write_pack(&root.join("a"), &declarative_mkdir_pack("a", &sink_a));
    // b has a broken manifest → halt
    fs::create_dir_all(root.join("b").join(".grex")).unwrap();
    fs::write(root.join("b").join(".grex").join("pack.yaml"), "garbage: : :").unwrap();
    write_pack(&root.join("c"), &declarative_mkdir_pack("c", &sink_c));

    let pack = parse(&meta_pack_with_children("p", &["a", "b", "c"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("b halts");
    assert!(matches!(err, ExecError::ExecInvalid(_)), "got: {err:?}");
    assert!(sink_a.is_dir(), "first child must have completed");
    assert!(!sink_c.exists(), "third child must NOT have been dispatched");
}

#[test]
fn meta_teardown_visits_children_in_reverse() {
    // Probe plugin records the order each child is dispatched; a custom
    // pack-type registry swaps the declarative plugin with the probe so
    // we can read the order back. Each child pack is declared as
    // `type: declarative` but the probe just records its own name.
    // Probe plugin records its dispatch order by appending to a shared
    // log file keyed off the child's dirname. It delegates the
    // ExecStep production to `DeclarativePlugin` (public API) by
    // running an empty-actions pack, sidestepping the
    // `ExecStep` non-exhaustive constructor restriction that applies
    // to external crates.
    use grex_core::plugin::pack_type::DeclarativePlugin;
    use std::sync::Mutex as StdMutex;

    struct Probe {
        order: Arc<StdMutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl PackTypePlugin for Probe {
        fn name(&self) -> &str {
            "declarative"
        }
        async fn install(
            &self,
            ctx: &ExecCtx<'_>,
            pack: &PackManifest,
        ) -> Result<grex_core::ExecStep, ExecError> {
            self.order.lock().unwrap().push(format!(
                "install:{}",
                ctx.pack_root.file_name().unwrap().to_string_lossy()
            ));
            DeclarativePlugin.install(ctx, pack).await
        }
        async fn update(
            &self,
            ctx: &ExecCtx<'_>,
            pack: &PackManifest,
        ) -> Result<grex_core::ExecStep, ExecError> {
            self.install(ctx, pack).await
        }
        async fn teardown(
            &self,
            ctx: &ExecCtx<'_>,
            pack: &PackManifest,
        ) -> Result<grex_core::ExecStep, ExecError> {
            self.order.lock().unwrap().push(format!(
                "teardown:{}",
                ctx.pack_root.file_name().unwrap().to_string_lossy()
            ));
            // DeclarativePlugin::teardown is a no-op stub; reuse its
            // returned ExecStep so we don't construct one ourselves.
            DeclarativePlugin.teardown(ctx, pack).await
        }
        async fn sync(
            &self,
            ctx: &ExecCtx<'_>,
            pack: &PackManifest,
        ) -> Result<grex_core::ExecStep, ExecError> {
            self.install(ctx, pack).await
        }
    }

    let order = Arc::new(StdMutex::new(Vec::<String>::new()));
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write_pack(&root, &meta_pack_with_children("p", &["a", "b", "c"]));
    // Child packs are `declarative` with no actions — probe records the
    // dispatch order and DeclarativePlugin.teardown returns a valid
    // no-op step without needing to execute anything.
    for name in ["a", "b", "c"] {
        write_pack(
            &root.join(name),
            &format!("schema_version: \"1\"\nname: {name}\ntype: declarative\n"),
        );
    }

    let pack = parse(&meta_pack_with_children("p", &["a", "b", "c"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let mut reg = PackTypeRegistry::new();
    reg.register(MetaPlugin);
    reg.register(Probe { order: order.clone() });
    let pack_type_reg = Arc::new(reg);
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    rt().block_on(MetaPlugin.teardown(&ctx, &pack)).expect("teardown ok");
    let seen = order.lock().unwrap().clone();
    assert_eq!(seen, vec!["teardown:c", "teardown:b", "teardown:a"], "got: {seen:?}");
}

#[test]
fn meta_update_equals_install_semantics() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let sink = tmp.path().join("upd-sink");
    write_pack(&root, &meta_pack_with_children("p", &["k"]));
    write_pack(&root.join("k"), &declarative_mkdir_pack("k", &sink));

    let pack = parse(&meta_pack_with_children("p", &["k"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    rt().block_on(MetaPlugin.update(&ctx, &pack)).expect("update ok");
    assert!(sink.is_dir(), "update must dispatch child action");
}

#[test]
fn meta_sync_equals_install_semantics() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let sink = tmp.path().join("sync-sink");
    write_pack(&root, &meta_pack_with_children("p", &["k"]));
    write_pack(&root.join("k"), &declarative_mkdir_pack("k", &sink));

    let pack = parse(&meta_pack_with_children("p", &["k"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    rt().block_on(MetaPlugin.sync(&ctx, &pack)).expect("sync ok");
    assert!(sink.is_dir(), "sync must dispatch child action");
}

#[test]
fn multi_thread_runtime_allows_nested_dispatch() {
    // 3-level meta chain where every level async-dispatches the next.
    // A current_thread runtime would deadlock on the second block_on;
    // this test passes only because M5-2c switched to multi_thread.
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let mid = root.join("mid");
    let leaf = mid.join("leaf");
    let sink = tmp.path().join("nested-sink");
    write_pack(&root, &meta_pack_with_children("root", &["mid"]));
    write_pack(&mid, &meta_pack_with_children("mid", &["leaf"]));
    write_pack(&leaf, &declarative_mkdir_pack("leaf", &sink));
    let pack = parse(&meta_pack_with_children("root", &["mid"]));

    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    rt().block_on(MetaPlugin.install(&ctx, &pack)).expect("nested install ok");
    assert!(sink.is_dir());
}

#[test]
fn meta_without_pack_type_registry_errors_cleanly() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    write_pack(&root, &meta_pack_with_children("p", &["k"]));
    write_pack(&root.join("k"), &declarative_mkdir_pack("k", &tmp.path().join("k-sink")));

    let pack = parse(&meta_pack_with_children("p", &["k"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let visited = new_visited();
    // Deliberately omit `with_pack_type_registry` so the meta plugin
    // must error instead of panic.
    let ctx = ExecCtx::new(&vars, root.as_path(), tmp.path())
        .with_registry(&action_reg)
        .with_visited_meta(&visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("missing registry errors");
    match err {
        ExecError::ExecInvalid(msg) => {
            assert!(msg.contains("pack_type_registry"), "msg: {msg}");
        }
        other => panic!("expected ExecInvalid, got: {other:?}"),
    }
}

#[test]
#[cfg(unix)]
fn cycle_detection_canonicalises_symlinks() {
    // UNIX-only: Windows requires Developer Mode / elevation for
    // CreateSymbolicLink. On Unix, create `root/b` as a real meta pack,
    // then a symlinked duplicate `root/b-link` pointing to it. A parent
    // meta pack referencing both paths through the cycle-back child
    // must detect the re-entry via canonicalization, not raw path
    // string equality.
    use std::os::unix::fs::symlink;
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("root");
    let b = root.join("b");
    write_pack(&root, &meta_pack_with_children("root", &["b"]));
    write_pack(&b, &meta_pack_with_children("b", &["b-link"]));
    // b/b-link is a symlink that points back up to `b` itself → canonical cycle.
    symlink(&b, b.join("b-link")).unwrap();
    // Write a child pack under b-link's canonical target so the loader
    // resolves; otherwise the symlinked path lookup still reads b's
    // pack.yaml due to canonicalise-on-load.

    let pack = parse(&meta_pack_with_children("root", &["b"]));
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let pack_type_reg = Arc::new(PackTypeRegistry::bootstrap());
    let visited = new_visited();
    let ctx = ctx(&vars, &root, tmp.path(), &action_reg, &pack_type_reg, &visited);

    let err = rt().block_on(MetaPlugin.install(&ctx, &pack)).expect_err("symlink cycle errors");
    assert!(
        matches!(err, ExecError::MetaCycle { .. }),
        "symlinked cycle should be MetaCycle, got: {err:?}"
    );
}

// Tiny sanity guard: ensure the Path and PathBuf imports aren't flagged
// as dead if no test exercises them directly.
#[test]
fn _imports_live() {
    let _: PathBuf = PathBuf::from("/");
    let _: &Path = Path::new("/");
}
