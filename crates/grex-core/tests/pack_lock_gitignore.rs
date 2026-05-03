//! feat-m6-2 — `.grex-lock` was historically added to every pack's
//! managed gitignore block on install so the sidecar file never
//! appeared in `git status`.
//!
//! v1.3.1 (B12) — auto-mutation REMOVED. `grex sync` (and the
//! per-lifecycle plugin install/update/teardown/sync) no longer write
//! to `<workspace>/.gitignore`. The advisory finding for "parent git
//! index tracks pack content" is now surfaced by `grex doctor`. The
//! tests below were inverted: where they previously asserted that the
//! managed block WAS written, they now assert that the workspace
//! `.gitignore` is left UNTOUCHED.

use std::path::Path;
use std::sync::Arc;

use grex_core::execute::ExecCtx;
use grex_core::pack;
use grex_core::plugin::pack_type::{
    default_managed_gitignore_patterns, DeclarativePlugin, PackTypePlugin,
};
use grex_core::plugin::Registry;
use grex_core::vars::VarEnv;
use tempfile::TempDir;

fn read_gitignore(root: &Path) -> Option<String> {
    std::fs::read_to_string(root.join(".gitignore")).ok()
}

/// The default-managed-pattern accessor is still public surface for
/// `grex doctor --fix` (which IS allowed to upsert managed blocks on
/// operator request). Keep the assertion that `.grex-lock` is in the
/// list so doctor's heal path stays correct.
#[test]
fn default_patterns_includes_grex_lock() {
    let patterns = default_managed_gitignore_patterns();
    assert!(
        patterns.contains(&".grex-lock"),
        "managed block defaults must include `.grex-lock`: {patterns:?}"
    );
}

/// B12 v1.3.1: `DeclarativePlugin::install` MUST NOT create the
/// workspace `.gitignore` file. Inverted from the v1.2.x assertion.
#[tokio::test]
async fn declarative_install_does_not_create_gitignore() {
    let tmp = TempDir::new().unwrap();
    let src = "schema_version: \"1\"\nname: gi-lock\ntype: declarative\n";
    let pack = pack::parse(src).unwrap();
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path()).with_registry(&action_reg);

    DeclarativePlugin.install(&ctx, &pack).await.expect("install ok");
    assert!(
        read_gitignore(tmp.path()).is_none(),
        "B12 v1.3.1: install MUST NOT create `.gitignore`. Found: {:?}",
        read_gitignore(tmp.path()),
    );
}

/// B12 v1.3.1: even when the pack manifest declares `x-gitignore:`
/// patterns, install MUST NOT write a managed block. Authors that
/// want their patterns in `.gitignore` add them by hand (operator
/// owns the file); doctor surfaces an advisory if the parent git
/// index tracks the pack.
#[tokio::test]
async fn declarative_install_with_x_gitignore_does_not_mutate_file() {
    let tmp = TempDir::new().unwrap();
    let src = "schema_version: \"1\"\nname: gi-merge\ntype: declarative\n\
               x-gitignore:\n  - target/\n  - \"*.log\"\n";
    let pack = pack::parse(src).unwrap();
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path()).with_registry(&action_reg);

    DeclarativePlugin.install(&ctx, &pack).await.expect("install ok");
    assert!(
        read_gitignore(tmp.path()).is_none(),
        "install with x-gitignore MUST NOT create `.gitignore`. Found: {:?}",
        read_gitignore(tmp.path()),
    );
}

/// B12 v1.3.1: a pre-existing user-authored `.gitignore` is preserved
/// byte-for-byte by install. No managed block is appended.
#[tokio::test]
async fn declarative_install_preserves_user_gitignore_byte_for_byte() {
    let tmp = TempDir::new().unwrap();
    let initial = b"# user content\nnode_modules/\n";
    std::fs::write(tmp.path().join(".gitignore"), initial).unwrap();

    let src = "schema_version: \"1\"\nname: gi-dedup\ntype: declarative\n\
               x-gitignore:\n  - \".grex-lock\"\n  - target/\n";
    let pack = pack::parse(src).unwrap();
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path()).with_registry(&action_reg);

    DeclarativePlugin.install(&ctx, &pack).await.expect("install ok");
    let post = std::fs::read(tmp.path().join(".gitignore")).expect(".gitignore preserved");
    assert_eq!(
        post,
        initial.to_vec(),
        "user `.gitignore` must be byte-equal pre-and-post install. PRE: {:?} POST: {:?}",
        String::from_utf8_lossy(initial),
        String::from_utf8_lossy(&post),
    );
    let post_str = String::from_utf8_lossy(&post);
    assert!(
        !post_str.contains("# >>> grex:"),
        "no managed-block markers may be introduced: {post_str}",
    );
}
