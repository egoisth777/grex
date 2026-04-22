//! M7-4c: verify workspace adopts `MIT OR Apache-2.0` dual license.
//!
//! Asserts:
//! - Every workspace crate declares `license = "MIT OR Apache-2.0"` in cargo metadata.
//! - `LICENSE`, `LICENSE-MIT`, `LICENSE-APACHE` files exist at repo root.
//! - The pointer `LICENSE` file references both MIT and Apache.
//! - `README.md` contains a `## License` section.
//!
//! The `cargo deny check licenses` check is exercised via CI; this test
//! deliberately does not shell out to it (would add a non-hermetic dep).

use std::path::{Path, PathBuf};
use std::process::Command;

/// Walk from CARGO_MANIFEST_DIR (crates/grex) up to the workspace root.
fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    // crates/grex -> crates -> <root>
    manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("workspace root two levels above crates/grex")
        .to_path_buf()
}

#[test]
fn license_files_exist_at_repo_root() {
    let root = workspace_root();
    for name in ["LICENSE", "LICENSE-MIT", "LICENSE-APACHE"] {
        let p = root.join(name);
        assert!(p.is_file(), "expected {} at repo root ({})", name, p.display());
    }
}

#[test]
fn pointer_license_mentions_both_licenses() {
    let root = workspace_root();
    let txt = std::fs::read_to_string(root.join("LICENSE")).expect("read LICENSE");
    assert!(txt.contains("MIT"), "LICENSE must mention MIT");
    assert!(txt.contains("Apache"), "LICENSE must mention Apache");
}

#[test]
fn apache_license_is_standard_text() {
    let root = workspace_root();
    let txt = std::fs::read_to_string(root.join("LICENSE-APACHE")).expect("read LICENSE-APACHE");
    // Canonical Apache-2.0 markers.
    assert!(
        txt.contains("Apache License"),
        "LICENSE-APACHE must contain the literal 'Apache License' title"
    );
    assert!(txt.contains("Version 2.0, January 2004"), "LICENSE-APACHE must declare Version 2.0");
    assert!(
        txt.contains("http://www.apache.org/licenses/"),
        "LICENSE-APACHE must reference the canonical URL"
    );
    assert!(
        txt.contains("TERMS AND CONDITIONS FOR USE, REPRODUCTION, AND DISTRIBUTION"),
        "LICENSE-APACHE must contain the terms-and-conditions header"
    );
}

#[test]
fn mit_license_is_standard_text() {
    let root = workspace_root();
    let txt = std::fs::read_to_string(root.join("LICENSE-MIT")).expect("read LICENSE-MIT");
    assert!(txt.contains("MIT License"), "LICENSE-MIT must contain 'MIT License' title");
    assert!(
        txt.contains("Permission is hereby granted"),
        "LICENSE-MIT must contain standard MIT grant clause"
    );
    assert!(
        txt.contains("THE SOFTWARE IS PROVIDED \"AS IS\""),
        "LICENSE-MIT must contain standard warranty disclaimer"
    );
}

#[test]
fn readme_has_license_section() {
    let root = workspace_root();
    let txt = std::fs::read_to_string(root.join("README.md")).expect("read README.md");
    assert!(txt.contains("## License"), "README.md must contain a `## License` section");
    assert!(
        txt.contains("MIT") && txt.contains("Apache"),
        "README.md License section must reference both MIT and Apache"
    );
}

#[test]
fn every_workspace_crate_is_dual_licensed() {
    // Prefer the cargo that ran the test so toolchain overrides are respected.
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let out = Command::new(&cargo)
        .args(["metadata", "--format-version=1", "--no-deps"])
        .current_dir(workspace_root())
        .output()
        .expect("run `cargo metadata`");
    assert!(
        out.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let meta: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("parse cargo metadata JSON");

    let packages = meta["packages"].as_array().expect("packages array");
    assert!(!packages.is_empty(), "metadata must list workspace packages");

    let mut checked = 0_usize;
    for pkg in packages {
        // --no-deps ensures only workspace members are listed, but double-check
        // via `source == null` (workspace members have no registry source).
        if !pkg.get("source").map(|s| s.is_null()).unwrap_or(false) {
            continue;
        }
        let name = pkg["name"].as_str().unwrap_or("<unknown>");
        let license = pkg["license"].as_str().unwrap_or("");
        assert_eq!(
            license, "MIT OR Apache-2.0",
            "crate `{}` must declare license = \"MIT OR Apache-2.0\" (got {:?})",
            name, license
        );
        checked += 1;
    }
    assert!(checked >= 4, "expected at least 4 workspace crates, saw {}", checked);
}
