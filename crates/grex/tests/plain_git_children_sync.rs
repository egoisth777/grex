//! feat-v1.1.1 e2e — plain-git children walked as synthetic
//! scripted-no-hooks packs.
//!
//! Locks the design-doc invariant: a child whose dest carries a
//! `.git/` but no `.grex/pack.yaml` must be walked as a leaf scripted
//! pack with `synthetic: true` on its lockfile entry. The walker must
//! NOT abort with `TreeError::ManifestNotFound` for such children —
//! that hard-error path is reserved for the genuine "you pointed at
//! nothing" case.
//!
//! Mirrors the harness shape used in `import_then_sync.rs` and
//! `legacy_workspace_migration.rs`:
//!
//! * `init_git_identity()` isolates the test from the developer's
//!   global / system git config.
//! * `seed_bare()` materialises a bare repo from a working tree, then
//!   the parent meta pack's `children[].url` points at the bare.
//! * `grex` is spawned via `assert_cmd::Command::cargo_bin("grex")`,
//!   matching every other e2e test in this crate.
//!
//! See `openspec/changes/feat-v1.1.1-plain-git-children/{proposal,design,tasks}.md`.

mod common;

use common::grex;
use grex_core::git::gix_backend::file_url_from_path;
use grex_core::lockfile::LockEntry;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use tempfile::TempDir;

fn init_git_identity() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        std::env::set_var("GIT_AUTHOR_NAME", "grex-test");
        std::env::set_var("GIT_AUTHOR_EMAIL", "test@grex.local");
        std::env::set_var("GIT_COMMITTER_NAME", "grex-test");
        std::env::set_var("GIT_COMMITTER_EMAIL", "test@grex.local");
        let null_cfg = std::env::temp_dir().join("grex-test-empty-gitconfig");
        let _ = std::fs::write(&null_cfg, b"");
        std::env::set_var("GIT_CONFIG_GLOBAL", &null_cfg);
        std::env::set_var("GIT_CONFIG_SYSTEM", &null_cfg);
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
    });
}

fn run_git(cwd: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(cwd).output().expect("git on PATH");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
}

/// Seed a bare repo whose initial commit ships `payload` files (and
/// optionally a `.grex/pack.yaml`). Returns the bare-repo path the
/// parent will clone from.
///
/// `pack_yaml`:
/// * `Some(yaml)` → seed commit includes `.grex/pack.yaml` with the
///   given body (declarative-with-pack-yaml children).
/// * `None` → seed commit is plain (just `README.md`); the cloned
///   working tree carries `.git/` but NOT `.grex/pack.yaml`. This is
///   the v1.1.1 plain-git-child shape.
fn seed_bare(tmp: &Path, name: &str, pack_yaml: Option<&str>) -> PathBuf {
    init_git_identity();
    let work = tmp.join(format!("seed-{name}-work"));
    fs::create_dir_all(&work).unwrap();
    fs::write(work.join("README.md"), format!("# {name}\n")).unwrap();
    if let Some(yaml) = pack_yaml {
        fs::create_dir_all(work.join(".grex")).unwrap();
        fs::write(work.join(".grex/pack.yaml"), yaml).unwrap();
    }
    run_git(&work, &["init", "-q", "-b", "main"]);
    run_git(&work, &["config", "user.email", "grex-test@example.com"]);
    run_git(&work, &["config", "user.name", "grex-test"]);
    run_git(&work, &["add", "-A"]);
    run_git(&work, &["commit", "-q", "-m", "seed"]);

    let bare = tmp.join(format!("{name}.git"));
    run_git(tmp, &["clone", "-q", "--bare", work.to_str().unwrap(), bare.to_str().unwrap()]);
    bare
}

/// Read `<root>/.grex/grex.lock.jsonl` as a `HashMap<id, LockEntry>`.
/// Panics on parse error so the test surfaces corruption directly.
fn read_lockfile_entries(root: &Path) -> HashMap<String, LockEntry> {
    let path = root.join(".grex").join("grex.lock.jsonl");
    let body = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("lockfile must exist at {}: {e}", path.display()));
    let mut out = HashMap::new();
    for (idx, line) in body.lines().enumerate() {
        if line.is_empty() {
            continue;
        }
        let entry: LockEntry = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("lockfile line {} invalid: {e}\n{line}", idx + 1));
        out.insert(entry.id.clone(), entry);
    }
    out
}

/// Build a meta-pack root whose `children[]` lists `n` plain-git
/// children. Each child is a fresh bare repo (no `.grex/pack.yaml`).
/// The children are NOT pre-cloned into the root — the walker clones
/// them on first sync and then synthesises a leaf scripted manifest
/// in-memory.
struct PlainGitLayout {
    _tmp: TempDir,
    root: PathBuf,
    child_names: Vec<String>,
}

fn build_plain_git_layout(names: &[&str]) -> PlainGitLayout {
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let root = tmp_path.join("root");
    fs::create_dir_all(&root).unwrap();

    let mut clone_urls: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        // No pack.yaml in the seed — the cloned working tree will be a
        // plain git repo, not a grex-flavoured one.
        let bare = seed_bare(&tmp_path, name, None);
        clone_urls.push(file_url_from_path(&bare));
    }

    let mut parent_yaml =
        String::from("schema_version: \"1\"\nname: root-meta\ntype: meta\nchildren:\n");
    for (name, url) in names.iter().zip(clone_urls.iter()) {
        parent_yaml.push_str(&format!("  - url: {url}\n    path: {name}\n"));
    }
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    PlainGitLayout { _tmp: tmp, root, child_names: names.iter().map(|s| (*s).into()).collect() }
}

#[test]
#[allow(clippy::too_many_lines)] // E2E narrative — splitting hides the test intent.
fn plain_git_children_sync_walks_to_completion() {
    let layout = build_plain_git_layout(&["alpha", "beta", "gamma"]);

    let assertion = grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();

    // The walker must NOT raise ManifestNotFound on plain-git children.
    assert!(
        !stderr.contains("manifest not found"),
        "sync stderr must not mention 'manifest not found': {stderr}",
    );

    // Every child clones into its flat-sibling slot and shows up in
    // sync output (plain-git children are walked as leaf scripted
    // packs, so each surfaces a step transcript line).
    for name in &layout.child_names {
        assert!(
            layout.root.join(name).join(".git").is_dir(),
            "child `{name}` must clone into flat-sibling slot on first sync",
        );
        assert!(
            !layout.root.join(name).join(".grex/pack.yaml").exists(),
            "child `{name}` must NOT have a pack.yaml — synthesis is in-memory only",
        );
        let combined = format!("{stdout}\n{stderr}");
        assert!(
            combined.contains(name.as_str()),
            "sync output must mention child `{name}`; got stdout=\n{stdout}\n--- stderr ---\n{stderr}",
        );
    }

    // Lockfile records every child with `synthetic: true`.
    let entries = read_lockfile_entries(&layout.root);
    for name in &layout.child_names {
        let entry = entries
            .get(name.as_str())
            .unwrap_or_else(|| panic!("lockfile must carry entry for `{name}`; got: {entries:?}"));
        assert!(entry.synthetic, "plain-git child `{name}` must have synthetic=true: {entry:?}",);
    }

    // FIX-4 — invariant pin: synthetic plain-git children MUST NOT
    // surface as `Event::Add` rows in the manifest log. The current
    // design synthesises the manifest in-memory at walk time, never
    // appending an `add` event. If a future refactor wires synthesis
    // into the persisted event log, the choice of `pack_type` for
    // such an event MUST stay within the documented v1 taxonomy
    // (`scripted` — never invented variants like `"synthetic_scripted"`).
    let manifest_path = layout.root.join(".grex").join("events.jsonl");
    if manifest_path.is_file() {
        let body = fs::read_to_string(&manifest_path).expect("read .grex/events.jsonl");
        let mut adds_for_synthetic = 0_usize;
        for line in body.lines() {
            if line.is_empty() {
                continue;
            }
            let v: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("manifest line not JSON: {e}\n{line}"));
            if v.get("op").and_then(|x| x.as_str()) != Some("add") {
                continue;
            }
            let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("");
            if !layout.child_names.iter().any(|n| n == id) {
                continue;
            }
            adds_for_synthetic += 1;
            // If this branch ever fires, force the `pack_type` field
            // to stay within the v1 taxonomy. `scripted` is the only
            // legal value for a synthesised plain-git child.
            let pack_type = v.get("type").and_then(|x| x.as_str()).unwrap_or("");
            assert_eq!(
                pack_type, "scripted",
                "synthetic plain-git child `{id}` Event::Add carried unexpected pack_type \
                 `{pack_type}` — only documented v1 values are legal (no invented variants)",
            );
        }
        assert_eq!(
            adds_for_synthetic, 0,
            "current design: synthetic plain-git children must not produce Event::Add rows; \
             got {adds_for_synthetic} for {:?}",
            layout.child_names,
        );
    }
}

#[test]
fn plain_git_children_sync_idempotent() {
    let layout = build_plain_git_layout(&["alpha", "beta", "gamma"]);

    grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    let lock_path = layout.root.join(".grex").join("grex.lock.jsonl");
    let entries_a = read_lockfile_entries(&layout.root);

    grex().current_dir(&layout.root).args(["sync", "."]).assert().success();
    let entries_b = read_lockfile_entries(&layout.root);

    // Same set of pack ids on both runs.
    let mut ids_a: Vec<&String> = entries_a.keys().collect();
    let mut ids_b: Vec<&String> = entries_b.keys().collect();
    ids_a.sort();
    ids_b.sort();
    assert_eq!(ids_a, ids_b, "second sync must not add/drop lockfile ids");

    // Per-id, every field except `installed_at` must match. The
    // `installed_at` timestamp is allowed to advance (each sync stamps
    // a fresh `Utc::now()`); everything else (sha, branch, hash,
    // schema, synthetic) is content-derived and must be byte-stable.
    for id in &ids_a {
        let a = &entries_a[id.as_str()];
        let b = &entries_b[id.as_str()];
        assert_eq!(a.sha, b.sha, "{id}: sha drift across idempotent runs");
        assert_eq!(a.branch, b.branch, "{id}: branch drift across idempotent runs");
        assert_eq!(a.actions_hash, b.actions_hash, "{id}: actions_hash drift");
        assert_eq!(a.schema_version, b.schema_version, "{id}: schema_version drift");
        assert_eq!(a.synthetic, b.synthetic, "{id}: synthetic flag drift");
    }

    assert!(lock_path.exists(), "lockfile must persist across runs at {}", lock_path.display());
}

#[test]
fn mixed_tree_meta_with_declarative_and_plain_git_children() {
    // Layout:
    //   <root>/.grex/pack.yaml  (type=meta, two children)
    //   <root>/decl/             — pack.yaml-equipped declarative pack
    //   <root>/plain/            — plain-git child (no pack.yaml)
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let root = tmp_path.join("root");
    fs::create_dir_all(&root).unwrap();
    let sink = tmp_path.join("sink");
    fs::create_dir_all(&sink).unwrap();

    // Declarative child — single trivial mkdir action, mirrors the
    // shape used in `import_then_sync.rs::build_layout`.
    let mkdir_path = sink.join("made-decl").to_string_lossy().replace('\\', "/");
    let decl_yaml = format!(
        "schema_version: \"1\"\nname: decl\ntype: declarative\nactions:\n  - mkdir:\n      path: {mkdir_path}\n",
    );
    let decl_bare = seed_bare(&tmp_path, "decl", Some(&decl_yaml));
    let decl_url = file_url_from_path(&decl_bare);

    // Plain-git child — no pack.yaml in the seed commit.
    let plain_bare = seed_bare(&tmp_path, "plain", None);
    let plain_url = file_url_from_path(&plain_bare);

    let parent_yaml = format!(
        "schema_version: \"1\"\nname: root-meta\ntype: meta\nchildren:\n  - url: {decl_url}\n    path: decl\n  - url: {plain_url}\n    path: plain\n",
    );
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    let assertion = grex().current_dir(&root).args(["sync", "."]).assert().success();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();
    assert!(
        !stderr.contains("manifest not found"),
        "mixed-tree sync stderr must not mention 'manifest not found': {stderr}",
    );

    // Both children are present on disk; only `decl` carries a
    // pack.yaml in its working tree.
    assert!(root.join("decl/.git").is_dir(), "decl child must clone");
    assert!(root.join("decl/.grex/pack.yaml").is_file(), "decl child must carry its pack.yaml");
    assert!(root.join("plain/.git").is_dir(), "plain-git child must clone");
    assert!(
        !root.join("plain/.grex/pack.yaml").exists(),
        "plain-git child must remain pack.yaml-less",
    );

    // Lockfile distinguishes the two: declared pack-yaml children have
    // `synthetic: false`; synthesised plain-git children have
    // `synthetic: true`.
    let entries = read_lockfile_entries(&root);
    let decl_entry = entries.get("decl").expect("decl in lockfile");
    let plain_entry = entries.get("plain").expect("plain in lockfile");
    assert!(
        !decl_entry.synthetic,
        "declarative child `decl` must have synthetic=false: {decl_entry:?}",
    );
    assert!(
        plain_entry.synthetic,
        "plain-git child `plain` must have synthetic=true: {plain_entry:?}",
    );
}

/// FIX 1 regression — synthetic plain-git children must surface as
/// `OK (synthetic)` doctor findings (driven by the lockfile, not the
/// manifest events) AND must NOT trip the `unregistered directory on
/// disk` warning. Both failure modes were observed by the v1.1.1 fix
/// sweep before doctor was rewired to consult the lockfile.
#[test]
#[allow(clippy::too_many_lines)] // E2E narrative — splitting hides the test intent.
fn doctor_after_plain_git_sync_reports_ok_synthetic_and_no_unregistered_warning() {
    let layout = build_plain_git_layout(&["alpha", "beta"]);

    // Populate the lockfile with synthetic entries via a real sync.
    grex().current_dir(&layout.root).args(["sync", "."]).assert().success();

    // Sanity: lockfile carries `synthetic: true` for every plain-git child.
    let entries = read_lockfile_entries(&layout.root);
    for name in &layout.child_names {
        let entry = entries
            .get(name.as_str())
            .unwrap_or_else(|| panic!("lockfile must carry entry for `{name}`; got: {entries:?}"));
        assert!(entry.synthetic, "fixture invariant: `{name}` must be synthetic");
    }

    // Run doctor in JSON mode so we can assert structurally on the
    // findings list. We bypass the table renderer entirely.
    let assertion = grex()
        .current_dir(&layout.root)
        .args(["--json", "doctor"])
        .assert()
        .code(predicates::ord::eq(0));
    let stdout = String::from_utf8(assertion.get_output().stdout.clone()).unwrap();
    let envelope: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("doctor --json must produce valid JSON");
    // v1.3.0: doctor --json envelope is `{workspace, pack, report: {...}}`.
    let report = &envelope["report"];
    let findings = report["findings"].as_array().expect("findings array");

    // Acceptance #1: one `synthetic-pack` finding per plain-git child,
    // each `severity: ok` and detail "OK (synthetic)".
    let synth: Vec<&serde_json::Value> =
        findings.iter().filter(|f| f["check"].as_str() == Some("synthetic-pack")).collect();
    assert_eq!(
        synth.len(),
        layout.child_names.len(),
        "one synthetic-pack finding per plain-git child; got {synth:?}",
    );
    for f in &synth {
        assert_eq!(f["severity"].as_str(), Some("ok"), "synthetic-pack must be Ok: {f}");
        assert_eq!(
            f["detail"].as_str(),
            Some("OK (synthetic)"),
            "synthetic-pack detail must be `OK (synthetic)`: {f}",
        );
        assert_eq!(f["synthetic"].as_bool(), Some(true), "Finding.synthetic must be true: {f}");
    }
    let mut synth_ids: Vec<&str> = synth.iter().map(|f| f["pack"].as_str().unwrap()).collect();
    synth_ids.sort();
    let mut expected_ids: Vec<&str> = layout.child_names.iter().map(String::as_str).collect();
    expected_ids.sort();
    assert_eq!(synth_ids, expected_ids, "synthetic-pack findings must cover every child");

    // Acceptance #2: ZERO unregistered-directory warnings for the
    // plain-git child names. The lockfile-driven on-disk-drift skip
    // is what makes this hold.
    for name in &layout.child_names {
        let needle = format!("unregistered directory on disk: {name}");
        let hit = findings.iter().find(|f| {
            f["check"].as_str() == Some("on-disk-drift")
                && f["severity"].as_str() == Some("warning")
                && f["detail"].as_str() == Some(needle.as_str())
        });
        assert!(
            hit.is_none(),
            "doctor must NOT flag synthetic plain-git child `{name}` as unregistered; \
             findings={findings:?}",
        );
    }
}

#[test]
fn child_dir_missing_both_pack_yaml_and_git_errors() {
    // Guard the design-doc invariant: synthesis ONLY fires when the
    // dest carries a `.git/`. A child whose dest already exists as a
    // non-empty *non-git* directory must fail the walk — it is
    // neither a declared grex pack nor a plain-git working tree, so
    // there is no synthesis fallback the walker can take.
    //
    // The failure surfaces from the git-clone-into-non-empty-dir
    // codepath (`GitError::DestinationNotEmpty`) rather than
    // `TreeError::ManifestNotFound` because `resolve_destination`
    // attempts the clone before the loader.load step. Both signal
    // the same invariant: the walker refuses to invent state for a
    // dest that is neither a pack nor a git repo.
    init_git_identity();
    let tmp = TempDir::new().unwrap();
    let tmp_path = tmp.path().to_path_buf();
    let root = tmp_path.join("root");
    fs::create_dir_all(&root).unwrap();

    // Seed a real bare repo so the URL itself is valid; the test is
    // about the dest-side guard, not URL validity.
    let bare = seed_bare(&tmp_path, "orphan", None);
    let url = file_url_from_path(&bare);

    // Pre-create the dest as a non-empty, non-git directory. This
    // pushes the walker into the failure path: clone refuses to
    // populate a non-empty dest, and there is no `.git/` already
    // there to skip the clone.
    let dest = root.join("orphan");
    fs::create_dir_all(&dest).unwrap();
    fs::write(dest.join("hand-authored.txt"), b"not a grex pack, not a git repo\n").unwrap();
    assert!(!dest.join(".git").exists(), "fixture invariant: dest must NOT have .git/");
    assert!(
        !dest.join(".grex/pack.yaml").exists(),
        "fixture invariant: dest must NOT have pack.yaml",
    );

    let parent_yaml = format!(
        "schema_version: \"1\"\nname: root-meta\ntype: meta\nchildren:\n  - url: {url}\n    path: orphan\n",
    );
    fs::create_dir_all(root.join(".grex")).unwrap();
    fs::write(root.join(".grex/pack.yaml"), parent_yaml).unwrap();

    let assertion = grex().current_dir(&root).args(["sync", "."]).assert().failure();
    let stderr = String::from_utf8(assertion.get_output().stderr.clone()).unwrap();

    // Accept either taxonomy entry-point as evidence the walker
    // refused to synthesise: the git-backend's "destination not
    // empty" or the loader's "manifest not found". The surface
    // depends on which failure the walker reaches first; what
    // matters is that synthesis did NOT silently swallow the
    // malformed child.
    let signals_refusal = stderr.contains("manifest not found")
        || stderr.contains("not empty")
        || stderr.contains("clone");
    assert!(
        signals_refusal,
        "sync stderr must signal walk refusal (manifest-not-found or clone failure); got:\n{stderr}",
    );

    // The user's hand-authored sentinel must remain on disk — the
    // failure path is read-only on the orphan dir.
    let body = fs::read_to_string(dest.join("hand-authored.txt")).unwrap();
    assert_eq!(body, "not a grex pack, not a git repo\n", "user data must survive failed sync");
}
