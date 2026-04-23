//! M8-4 integration smoke — keep `examples/pack-template/` continuously
//! validated against the canonical `pack.yaml` schema *and* the grex-core
//! sync contract.
//!
//! The template is the user-facing reference pack; if grex-core's pack-parse
//! or sync contract ever drifts, this test breaks loudly in CI (on all 3 OS
//! — CI's `cargo test --workspace` matrix) rather than silently shipping a
//! broken example.
//!
//! Coverage:
//!
//! 1. **Parse + shape** — `pack.yaml` parses clean via `grex_core::pack::parse`
//!    and matches the README-advertised shape (name, type, schema_version,
//!    first action is a `require` gate, explicit `teardown` present).
//! 2. **File inventory** — every file the README promises is on disk.
//! 3. **End-to-end sync** — copy the template into a tempdir, point its
//!    `$HOME` at the tempdir, run `grex_core::sync::run` twice, and assert
//!    the second run is a pure no-op (no `PerformedChange` steps).
//!
//! We drive `sync::run` directly instead of shelling out to `grex-cli`:
//! the library path is faster, covers the same semantics, and matches the
//! pattern used by `crates/grex-core/tests/pack_type_dispatch.rs`.

#![allow(clippy::needless_pass_by_value)]

use std::fs;
use std::path::{Path, PathBuf};

use grex_core::pack::{parse, Action, PackType};
use grex_core::sync::{self, SyncOptions};
use grex_core::ExecResult;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

fn repo_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` for this test binary = `<repo>/crates/grex`.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/grex must have a grand-parent (repo root)")
        .to_path_buf()
}

fn template_root() -> PathBuf {
    repo_root().join("examples").join("pack-template")
}

#[test]
fn pack_template_manifest_parses_and_matches_reference_shape() {
    let manifest_path = template_root().join(".grex").join("pack.yaml");
    let yaml = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest_path.display()));

    let pack = parse(&yaml).expect("pack-template/.grex/pack.yaml must parse clean");

    assert_eq!(pack.schema_version.as_str(), "1", "schema_version must be \"1\"");
    assert_eq!(pack.name, "grex-pack-template", "name drift");
    assert_eq!(pack.r#type, PackType::Declarative, "type drift");
    assert_eq!(pack.version.as_deref(), Some("1.0.0"), "version drift");
    assert!(pack.children.is_empty(), "template does not use children");

    // The template ships a `require` gate followed by one or more concrete
    // actions. Keep the shape loose — we only assert the high-level contract
    // so minor tweaks to the manifest don't break CI.
    assert!(
        pack.actions.len() >= 2,
        "template should have at least a require + one concrete action, got {}",
        pack.actions.len()
    );
    assert!(matches!(pack.actions[0], Action::Require(_)), "first action must be a require gate");

    // Explicit teardown is present — the README advertises it.
    let teardown = pack.teardown.as_ref().expect("explicit teardown expected");
    assert_eq!(teardown.len(), 1, "teardown is a single rmdir");
    assert!(matches!(teardown[0], Action::Rmdir(_)), "teardown step must be rmdir");
}

#[test]
fn pack_template_ships_all_expected_files() {
    let root = template_root();

    // Manifest + payload + user-facing docs. No LICENSE in-tree: the README
    // points at the main grex repo's LICENSE-MIT / LICENSE-APACHE. No
    // `.grex/hooks/` dir: the template is `type: declarative`, which does
    // not use hooks.
    for rel in &[".grex/pack.yaml", "files/hello.txt", "README.md", ".gitignore"] {
        let p = root.join(rel);
        assert!(p.is_file(), "missing expected file: {}", p.display());
    }
}

#[test]
fn pack_template_payload_referenced_by_symlink_exists() {
    // Tight coupling check: `pack.yaml` references `files/hello.txt` as the
    // `symlink.src`. If the template ever renames the payload without
    // updating the manifest, this assertion catches it before users do.
    let payload = template_root().join("files").join("hello.txt");
    assert!(payload.is_file(), "symlink.src target missing: {}", payload.display());
}

/// Recursively copy `src` → `dst`. Small helper to keep the test dep-free
/// (we already pull `tempfile`; no need for a `fs_extra` just for this).
fn copy_dir(src: &Path, dst: &Path) {
    fs::create_dir_all(dst).expect("create dst");
    for entry in fs::read_dir(src).expect("read_dir") {
        let entry = entry.expect("dir entry");
        let ft = entry.file_type().expect("file_type");
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir(&from, &to);
        } else if ft.is_file() {
            fs::copy(&from, &to).expect("copy file");
        }
        // Skip symlinks: the template ships none in-tree; grex creates them
        // at sync time into the redirected $HOME.
    }
}

#[test]
fn pack_template_sync_runs_end_to_end_and_second_run_is_noop() {
    // Copy the template into a tempdir, redirect `$HOME` / `USERPROFILE`
    // there, and run `sync::run` twice. The second run must produce zero
    // `PerformedChange` steps — every action in the template is advertised
    // as idempotent in the README.
    let tmp = TempDir::new().expect("tempdir");
    let tmp_path = tmp.path();

    let pack_root = tmp_path.join("pack");
    copy_dir(&template_root(), &pack_root);

    let fake_home = tmp_path.join("home");
    fs::create_dir_all(&fake_home).expect("create fake home");

    // Scope env overrides to this test; remove the prior values on the way
    // out to keep the rest of the suite clean. `std::env` is process-global
    // so this test is inherently non-parallel with anything else that pokes
    // $HOME — acceptable for M8-4's scope.
    let prev_home = std::env::var("HOME").ok();
    let prev_userprofile = std::env::var("USERPROFILE").ok();
    std::env::set_var("HOME", &fake_home);
    std::env::set_var("USERPROFILE", &fake_home);

    let workspace = tmp_path.join("ws");
    fs::create_dir_all(&workspace).expect("create ws");

    let opts = SyncOptions::new().with_workspace(Some(workspace.clone()));
    let cancel = CancellationToken::new();

    let report1 = sync::run(&pack_root, &opts, &cancel).expect("first sync ok");
    assert!(report1.halted.is_none(), "first sync halted: {:?}", report1.halted);
    let performed1 = report1
        .steps
        .iter()
        .filter(|s| matches!(s.exec_step.result, ExecResult::PerformedChange))
        .count();
    assert!(
        performed1 >= 1,
        "first sync should perform at least one change (mkdir or symlink); steps: {:?}",
        report1.steps
    );

    let report2 = sync::run(&pack_root, &opts, &cancel).expect("second sync ok");
    assert!(report2.halted.is_none(), "second sync halted: {:?}", report2.halted);
    let performed2 = report2
        .steps
        .iter()
        .filter(|s| matches!(s.exec_step.result, ExecResult::PerformedChange))
        .count();
    assert_eq!(
        performed2, 0,
        "second sync must be an all-no-op (idempotency contract); steps: {:?}",
        report2.steps
    );

    // Restore env.
    match prev_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    match prev_userprofile {
        Some(v) => std::env::set_var("USERPROFILE", v),
        None => std::env::remove_var("USERPROFILE"),
    }
}
