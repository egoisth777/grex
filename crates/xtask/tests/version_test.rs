//! Workspace version regression test (feat-v1.0.1-doc-site).
//!
//! `xtask` inherits `version.workspace = true`, so its compile-time
//! `CARGO_PKG_VERSION` is a faithful proxy for the workspace
//! `[workspace.package].version`. If a future bump forgets to update
//! `Cargo.toml`'s `[workspace.package].version`, this test goes red.
//!
//! When you intentionally cut a new release, update the constant below.

const EXPECTED_WORKSPACE_VERSION: &str = "1.1.0";

#[test]
fn workspace_version_is_pinned() {
    assert_eq!(
        env!("CARGO_PKG_VERSION"),
        EXPECTED_WORKSPACE_VERSION,
        "xtask CARGO_PKG_VERSION drifted from the expected workspace version. \
         Either bump `[workspace.package].version` in root Cargo.toml or update \
         EXPECTED_WORKSPACE_VERSION in crates/xtask/tests/version_test.rs to match."
    );
}
