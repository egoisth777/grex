//! feat-m6-2 — `.grex-lock` is added to every pack's managed gitignore
//! block by default so the sidecar file never appears in `git status`.

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

fn read_gitignore(root: &Path) -> String {
    std::fs::read_to_string(root.join(".gitignore")).unwrap_or_default()
}

#[test]
fn default_patterns_includes_grex_lock() {
    let patterns = default_managed_gitignore_patterns();
    assert!(
        patterns.contains(&".grex-lock"),
        "managed block defaults must include `.grex-lock`: {patterns:?}"
    );
}

#[tokio::test]
async fn declarative_install_writes_grex_lock_in_managed_block() {
    let tmp = TempDir::new().unwrap();
    let src = "schema_version: \"1\"\nname: gi-lock\ntype: declarative\n";
    let pack = pack::parse(src).unwrap();
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path()).with_registry(&action_reg);

    DeclarativePlugin.install(&ctx, &pack).await.expect("install ok");
    let gitignore = read_gitignore(tmp.path());
    assert!(gitignore.contains("# >>> grex:gi-lock >>>"), "header: {gitignore}");
    assert!(gitignore.contains("# <<< grex:gi-lock <<<"), "footer: {gitignore}");
    assert!(gitignore.contains(".grex-lock"), "default pattern: {gitignore}");
    // Exactly one occurrence (no duplication on successive re-runs).
    assert_eq!(gitignore.matches(".grex-lock").count(), 1);
}

#[tokio::test]
async fn declarative_install_merges_author_patterns_with_defaults() {
    let tmp = TempDir::new().unwrap();
    let src = "schema_version: \"1\"\nname: gi-merge\ntype: declarative\n\
               x-gitignore:\n  - target/\n  - \"*.log\"\n";
    let pack = pack::parse(src).unwrap();
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path()).with_registry(&action_reg);

    DeclarativePlugin.install(&ctx, &pack).await.expect("install ok");
    let gitignore = read_gitignore(tmp.path());
    assert!(gitignore.contains(".grex-lock"));
    assert!(gitignore.contains("target/"));
    assert!(gitignore.contains("*.log"));
}

#[tokio::test]
async fn declarative_install_dedup_default_if_author_listed_it() {
    let tmp = TempDir::new().unwrap();
    // Author explicitly listing `.grex-lock` must not double-emit.
    let src = "schema_version: \"1\"\nname: gi-dedup\ntype: declarative\n\
               x-gitignore:\n  - \".grex-lock\"\n  - target/\n";
    let pack = pack::parse(src).unwrap();
    let vars = VarEnv::default();
    let action_reg = Arc::new(Registry::bootstrap());
    let ctx = ExecCtx::new(&vars, tmp.path(), tmp.path()).with_registry(&action_reg);

    DeclarativePlugin.install(&ctx, &pack).await.expect("install ok");
    let gitignore = read_gitignore(tmp.path());
    assert_eq!(
        gitignore.matches(".grex-lock").count(),
        1,
        "duplicate author entry must be de-duped: {gitignore}"
    );
}
