//! Module-isolation lint for the v1.1.1 → v1.2.0 lockfile migrator.
//!
//! Stage 0 LOCKED decision #5 declares the migrator a *removable
//! module*: walker / sync / ls / doctor / add / rm MUST NOT reference
//! it. Reachability is restricted to the CLI's `--migrate-lockfile`
//! flag dispatcher, the dedicated `grex migrate-lockfile` subcommand,
//! and the migrator's own tests / docs.
//!
//! This test enforces the rule mechanically so a future patch that
//! sneaks an inbound caller into the steady-state code paths fails CI
//! before it lands. The check is a content grep — coarser than a real
//! call-graph analysis but cheap enough to run on every test invocation
//! and catch the failure mode (an unintended `use ... migrate_v1_1_1`
//! import) the rule is meant to prevent.

use std::fs;
use std::path::{Path, PathBuf};

/// Files under `crates/grex-core/src/` that MUST NOT mention the
/// migrator module name. The walker / sync / ls / doctor / add / rm
/// boundaries are the steady-state code paths — every Stage 0 LOCKED
/// rationale points to "the migrator can be deleted in a future minor
/// release without touching other units".
const FORBIDDEN_FILES: &[&str] = &["tree/walker.rs", "sync.rs", "tree/ls.rs", "doctor.rs"];

/// Token the lint searches for. A bare `migrate_v1_1_1` substring is
/// the simplest possible signal: any `use`, `super::migrate_v1_1_1`,
/// `crate::lockfile::migrate_v1_1_1`, or fully-qualified call site
/// trips it.
const FORBIDDEN_TOKEN: &str = "migrate_v1_1_1";

fn crate_root() -> PathBuf {
    // CARGO_MANIFEST_DIR resolves to `crates/grex-core/` at test time.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_or_skip(path: &Path) -> Option<String> {
    match fs::read_to_string(path) {
        Ok(s) => Some(s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => panic!("failed to read {}: {e}", path.display()),
    }
}

#[test]
fn migrator_module_has_no_inbound_callers_from_steady_state_code() {
    let src_dir = crate_root().join("src");
    let mut violations: Vec<String> = Vec::new();

    for rel in FORBIDDEN_FILES {
        let path = src_dir.join(rel);
        let Some(content) = read_or_skip(&path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if line.contains(FORBIDDEN_TOKEN) {
                violations.push(format!("{}:{}: {}", rel, idx + 1, line.trim()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "migrator module must NOT be referenced from walker / sync / ls / \
         doctor (Stage 0 LOCKED decision #5 — removable-module isolation). \
         Offending references:\n  {}",
        violations.join("\n  "),
    );
}

#[test]
fn migrator_module_is_not_re_exported_from_lockfile_mod() {
    // The migrator module is intentionally `pub mod migrate_v1_1_1` so
    // out-of-crate callers (the CLI) can reach it via the long path,
    // but it MUST NOT be re-exported via `pub use migrate_v1_1_1::*`
    // — that would put the public symbols on the same level as
    // `read_lockfile` and tempt the walker to call them. The lint
    // checks for the re-export shape specifically.
    let mod_rs = crate_root().join("src").join("lockfile").join("mod.rs");
    let content = fs::read_to_string(&mod_rs).expect("read lockfile/mod.rs");
    assert!(
        !content.contains("pub use migrate_v1_1_1"),
        "lockfile/mod.rs must NOT re-export migrate_v1_1_1::* — long-path \
         access only (Stage 0 LOCKED decision #5)",
    );
}
