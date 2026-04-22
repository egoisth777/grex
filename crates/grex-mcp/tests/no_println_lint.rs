//! Stage 5 test #5.T5 — `println!` / `print!` are forbidden anywhere under
//! `crates/grex-mcp/src/`.
//!
//! Choice: implemented as a Rust integration test (rather than a Make/xtask
//! target) so it runs on every `cargo test` invocation, on Windows + Linux,
//! without external tooling. Walks the crate's `src/` tree at test time and
//! grep-by-line — fast (handful of files) and self-contained.

use std::{fs, path::Path};

#[test]
fn src_tree_has_zero_println_or_print_macros() {
    let here = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut hits = Vec::<String>::new();
    walk(&here, &mut hits);
    assert!(
        hits.is_empty(),
        "stdout-only-JSON-RPC discipline broken — found println!/print! in:\n{}",
        hits.join("\n")
    );
}

fn walk(dir: &Path, hits: &mut Vec<String>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, hits);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("rs") {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        for (lineno, line) in text.lines().enumerate() {
            // Skip our own comment that talks about the lint.
            if line.trim_start().starts_with("//") || line.trim_start().starts_with("///") {
                continue;
            }
            if line.contains("println!") || line.contains("print!") {
                hits.push(format!("{}:{}: {}", path.display(), lineno + 1, line));
            }
        }
    }
}
