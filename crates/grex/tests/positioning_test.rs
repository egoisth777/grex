//! Positioning regression test (feat-positioning-rewrite).
//!
//! Asserts the locked v1 tagline ("nested meta-repo manager") and key
//! positioning phrases appear on the three surfaces that adopters
//! actually read first:
//!
//! * `clap::Command::about` — what `grex --help` prints.
//! * `man/grex.1` — what `man grex` prints.
//! * `README.md` first 30 lines — what crates.io / GitHub render
//!   above the fold.
//!
//! Designed to FAIL on the pre-edit `main` (where the tagline still
//! says "Cross-platform dev-environment orchestrator" / "Pack-based
//! dev-env orchestrator") and PASS once the locked copy lands.

use clap::CommandFactory;
use std::{fs, path::PathBuf};

use grex_cli::cli::args::Cli;

/// The locked tagline phrase. Substring-matched (not equality) so
/// surrounding punctuation (`.`, ` —`) doesn't have to be byte-identical
/// across surfaces.
const TAGLINE_PHRASE: &str = "nested meta-repo manager";

/// Resolve the workspace root from this test's `CARGO_MANIFEST_DIR`
/// (`<workspace>/crates/grex`). Two `pop`s land on `<workspace>`.
fn workspace_root() -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // -> crates/
    p.pop(); // -> workspace root
    p
}

#[test]
fn clap_about_contains_nested_meta_repo_phrase() {
    let cmd = Cli::command();
    let about = cmd
        .get_about()
        .map(|s| s.to_string())
        .expect("clap `about` must be set on the root Cli command");
    assert!(
        about.contains(TAGLINE_PHRASE),
        "clap `about` must contain `{TAGLINE_PHRASE}`; got: {about:?}"
    );
}

#[test]
fn man_page_grex_1_contains_nested_meta_repo_phrase() {
    // `clap_mangen` escapes ASCII hyphens as `\-` per troff convention,
    // so the rendered NAME line reads `nested meta\-repo manager`. We
    // strip those backslash-hyphens before substring-matching so the
    // test asserts the *user-visible* text rather than the troff source.
    let path = workspace_root().join("man").join("grex.1");
    let raw = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let unescaped = raw.replace("\\-", "-");
    assert!(
        unescaped.contains("nested meta-repo"),
        "man/grex.1 must mention `nested meta-repo` (after unescaping `\\-`); first 200 chars: {:?}",
        &raw.chars().take(200).collect::<String>()
    );
}

#[test]
fn readme_first_30_lines_contain_tagline_and_phrase() {
    let path = workspace_root().join("README.md");
    let body = fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    let head: String = body.lines().take(30).collect::<Vec<_>>().join("\n");
    assert!(
        head.contains(TAGLINE_PHRASE),
        "README.md first 30 lines must contain `{TAGLINE_PHRASE}`; got:\n{head}"
    );
    // Pack-based / agent-native / Rust-fast triplet is part of the
    // locked tagline; assert the most distinctive token to guard
    // against accidental rewrites that drop the rhythm.
    assert!(
        head.contains("Pack-based, agent-native, Rust-fast"),
        "README.md first 30 lines must contain the locked positioning triplet; got:\n{head}"
    );
}
