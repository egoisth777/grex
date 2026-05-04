//! Regression gates for dogfood bugs B1-B15.
//!
//! Source post-mortem: `.omne/var/dogfood-findings-v1.3.0.md` (SSOT, separate
//! `grex-inst` repo per Rule 7). Each test below maps 1:1 to a bug ID and is
//! designed to FAIL against a v1.3.0 binary (= the regression direction is
//! locked). v1.3.x patch series will flip them green as the underlying defects
//! are fixed.
//!
//! All tests are `#[ignore]` by default — they require:
//!   1. A pre-built `grex` binary (located via `GREX_BIN` env var or
//!      `target/{debug,release}/grex`).
//!   2. Network access + an SSH key registered with `egoisth777` for cloning
//!      the six pre-provisioned GitHub fixture repos.
//!   3. `git` on PATH for worktree management.
//!
//! Run via: `cargo test -p real-smoke --tests -- --ignored --nocapture`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use tempfile::TempDir;

use real_smoke::assertions::{
    self, assert_gitignore_unchanged, assert_lockfile_under_grex_dir, assert_no_warn_on_stdout,
    sha256_of_file,
};
use real_smoke::fixtures::{FIXTURE_BROKEN, FIXTURE_LEAF, FIXTURE_META_FLAT, FIXTURE_META_NESTED};
use real_smoke::grex_cli::{self, CliResult};
use real_smoke::worktree::WorktreeGuard;

// ---------------------------------------------------------------------------
// Test helpers (local — not part of the harness public API).
// ---------------------------------------------------------------------------

/// Default branch every fixture is seeded on.
const DEFAULT_BRANCH: &str = "main";

/// Wraps a freshly minted `tempfile::TempDir` (used as `base_dir` for the
/// `WorktreeGuard`) together with the guard itself, so the temp dir survives
/// for the lifetime of the test.
struct WtFixture {
    _base: TempDir,
    wt: WorktreeGuard,
}

impl WtFixture {
    fn new(repo_url: &str) -> Result<Self> {
        let base = tempfile::Builder::new()
            .prefix("real-smoke-")
            .tempdir()
            .context("creating tempdir for worktree base")?;
        let wt = WorktreeGuard::new(repo_url, base.path(), DEFAULT_BRANCH)
            .with_context(|| format!("WorktreeGuard::new for {repo_url}"))?;
        Ok(Self { _base: base, wt })
    }

    fn path(&self) -> &Path {
        self.wt.path()
    }
}

/// Reads a YAML or JSON lockfile from the worktree's `.grex/` dir, trying the
/// known candidate names in order. Returns the lockfile path + parsed value
/// (`serde_yaml::Value` is a JSON superset for our purposes).
fn read_lockfile(worktree: &Path) -> Result<(PathBuf, serde_yaml::Value)> {
    let grex_dir = worktree.join(".grex");
    let candidates = ["grex.lock", "grex.lock.yaml", "grex.lock.json", ".grex.sync.lock"];
    for name in candidates {
        let p = grex_dir.join(name);
        if p.exists() {
            let text = std::fs::read_to_string(&p)
                .with_context(|| format!("read lockfile {}", p.display()))?;
            let v: serde_yaml::Value = serde_yaml::from_str(&text)
                .with_context(|| format!("parse lockfile {}", p.display()))?;
            return Ok((p, v));
        }
    }
    Err(anyhow!("no lockfile found under {}/.grex/ (tried {:?})", worktree.display(), candidates))
}

/// Walks every node of a `serde_yaml::Value`, collecting every string key
/// appearing in any mapping. Used to assert `synthetic` does not appear (B6).
fn collect_yaml_keys(v: &serde_yaml::Value, sink: &mut BTreeSet<String>) {
    match v {
        serde_yaml::Value::Mapping(m) => {
            for (k, val) in m {
                if let serde_yaml::Value::String(s) = k {
                    sink.insert(s.clone());
                }
                collect_yaml_keys(val, sink);
            }
        }
        serde_yaml::Value::Sequence(s) => {
            for item in s {
                collect_yaml_keys(item, sink);
            }
        }
        _ => {}
    }
}

/// Reads `events.jsonl` and returns one parsed JSON value per non-empty line.
fn read_event_log(worktree: &Path) -> Result<Vec<serde_json::Value>> {
    let p = worktree.join(".grex").join("events.jsonl");
    let text =
        std::fs::read_to_string(&p).with_context(|| format!("read event log {}", p.display()))?;
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).with_context(|| format!("parse jsonl line: {l}")))
        .collect()
}

/// Asserts exit-code 0 with a message that includes captured stdout/stderr.
fn assert_success(result: &CliResult, ctx: &str) {
    assert!(
        result.is_success(),
        "{ctx}: expected exit 0, got {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        result.exit_code,
        result.stdout,
        result.stderr
    );
}

// ---------------------------------------------------------------------------
// B1 — `grex ls` label/path shape on nested children.
// ---------------------------------------------------------------------------

/// B1 (ls label/path shape on nested children).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
///
/// `grex ls` against a meta-pack with nested children must NOT carry the
/// `(scripted, synthetic)` substring; nested children declared in pack.yaml
/// should render as `(declared, unsynced)` (or similar declared-state label)
/// before `grex sync` runs.
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b01_ls_label_path_shape() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_NESTED)?;
    let result = grex_cli::run(&["ls"], f.path())?;
    assert_success(&result, "grex ls on meta-nested fixture");
    assert!(
        !result.stdout.contains("(scripted, synthetic)"),
        "B1 regressed: nested child labelled `(scripted, synthetic)`\n--- stdout ---\n{}",
        result.stdout
    );
    assert!(
        result.stdout.contains("(declared") || result.stdout.contains("declared, unsynced"),
        "B1: expected `declared` label for nested child\n--- stdout ---\n{}",
        result.stdout
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B2 — `grex sync` defaults pack root to cwd.
// ---------------------------------------------------------------------------

/// B2 (sync defaults pack root to cwd).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
///
/// `cd <worktree>; grex sync` (no positional, no `--pack` flag) must succeed
/// with the pack root resolved to cwd. Regressed in v1.3.0 by requiring an
/// explicit `--pack .` flag.
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b02_sync_default_pack_root_cwd() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_NESTED)?;
    let result = grex_cli::run(&["sync", "--dry-run"], f.path())?;
    assert_success(&result, "grex sync (cwd-default pack root)");
    assert!(
        !result.stderr.contains("missing pack root") && !result.stderr.contains("--pack required"),
        "B2 regressed: sync demanded explicit pack root\n--- stderr ---\n{}",
        result.stderr
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B3 — `grex sync --pack .` and `--workspace .` both succeed.
// ---------------------------------------------------------------------------

/// B3 (--pack . and --workspace . both succeed).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
///
/// Both forms must parse and resolve to the worktree. `--workspace` is the
/// legacy alias and SHOULD emit a deprecation warning on stderr but still
/// succeed.
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b03_pack_dot_and_workspace_dot() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_NESTED)?;

    let pack_dot = grex_cli::run(&["sync", "--pack", ".", "--dry-run"], f.path())?;
    assert_success(&pack_dot, "grex sync --pack .");

    let workspace_dot = grex_cli::run(&["sync", "--workspace", ".", "--dry-run"], f.path())?;
    assert_success(&workspace_dot, "grex sync --workspace .");
    assert!(
        workspace_dot.stderr.to_lowercase().contains("deprecate")
            || workspace_dot.stderr.to_lowercase().contains("workspace"),
        "B3: expected deprecation warning for --workspace on stderr\n--- stderr ---\n{}",
        workspace_dot.stderr
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B4 — `--dry-run` does NOT touch the network.
// ---------------------------------------------------------------------------

/// B4 (dry-run no network).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
///
/// `grex sync --dry-run` with `GIT_SSH_COMMAND` rigged to fail must still
/// exit 0 and must NOT materialise any `.git/` directory under expected child
/// paths.
///
/// HARNESS GAP (W2b): `grex_cli::run` does not currently accept env overrides.
/// We rig `GIT_SSH_COMMAND` on the harness process for the duration of the
/// call. Real-smoke runs serially by design, so this is acceptable today, but
/// it would be cleaner if `grex_cli::run` grew an `env` parameter (or a
/// builder variant `run_with_env`).
#[test]
#[ignore = "requires provisioned GH fixtures (network is intentionally blocked)"]
fn t_b04_dry_run_no_network() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;

    let prev_ssh = std::env::var("GIT_SSH_COMMAND").ok();
    // SAFETY: real-smoke tests are sequential by contract; mutating harness
    // process env is acceptable here.
    unsafe {
        std::env::set_var(
            "GIT_SSH_COMMAND",
            "ssh -o BatchMode=yes -o ConnectTimeout=2 -i /dev/null",
        );
    }

    let result = grex_cli::run(&["sync", "--dry-run"], f.path());

    // Restore env before any assertion can panic.
    match prev_ssh {
        Some(v) => unsafe { std::env::set_var("GIT_SSH_COMMAND", v) },
        None => unsafe { std::env::remove_var("GIT_SSH_COMMAND") },
    }

    let result = result?;
    assert_success(&result, "grex sync --dry-run with broken SSH");
    for child in ["leaf-1", "leaf-2", "leaf-3"] {
        let dot_git = f.path().join(child).join(".git");
        assert!(!dot_git.exists(), "B4 regressed: dry-run materialised {}", dot_git.display());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B5 — `grex doctor` consults `.gitignore`.
// ---------------------------------------------------------------------------

/// B5 (doctor consults .gitignore).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
///
/// A directory listed in `.gitignore` must NOT be reported as drift by
/// `grex doctor`.
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b05_doctor_consults_gitignore() -> Result<()> {
    let f = WtFixture::new(FIXTURE_BROKEN)?;

    // Append our marker dir to .gitignore and create it on disk.
    let gi = f.path().join(".gitignore");
    let mut existing = std::fs::read_to_string(&gi).unwrap_or_default();
    if !existing.is_empty() && !existing.ends_with('\n') {
        existing.push('\n');
    }
    existing.push_str("pseudo-ignored-dir/\n");
    std::fs::write(&gi, existing)?;
    std::fs::create_dir_all(f.path().join("pseudo-ignored-dir"))?;

    let result = grex_cli::run(&["doctor", "--json"], f.path())?;
    // doctor exits non-zero on broken manifests; we assert findings shape, not
    // exit code.
    assert!(
        !result.stdout.contains("pseudo-ignored-dir"),
        "B5 regressed: doctor flagged a .gitignore-listed directory\n--- stdout ---\n{}",
        result.stdout
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B6 — Lockfile carries no `synthetic: true` entries.
// ---------------------------------------------------------------------------

/// B6 (no synthetic field in lockfile).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b06_no_synthetic_field() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;
    let result = grex_cli::run(&["sync"], f.path())?;
    assert_success(&result, "grex sync on meta-flat");

    let (lock_path, lock) = read_lockfile(f.path())?;
    let mut keys = BTreeSet::new();
    collect_yaml_keys(&lock, &mut keys);
    assert!(
        !keys.contains("synthetic"),
        "B6 regressed: lockfile {} carries `synthetic` key (keys: {:?})",
        lock_path.display(),
        keys
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B7 — Warnings land on stderr; op names are Display-formatted.
// ---------------------------------------------------------------------------

/// B7 (warn lands on stderr, op name Display not Discriminant).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b07_warn_stderr_op_name() -> Result<()> {
    let f = WtFixture::new(FIXTURE_BROKEN)?;
    let result = grex_cli::run(&["doctor"], f.path())?;

    assert_no_warn_on_stdout(&result).context("B7: stdout/stderr split")?;

    let combined = format!("{}{}", result.stdout, result.stderr);
    assert!(
        !combined.contains("Discriminant("),
        "B7 regressed: op rendered as `Discriminant(N)` instead of named\n{}",
        combined
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B8 — Event log records carry `id` and `schema_version`.
// ---------------------------------------------------------------------------

/// B8 (event log id + schema_version).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
///
/// HARNESS GAP (W2b): `assertions::assert_eventlog_record_carries_field` keys
/// off the `op` field; the dogfood report names the relevant ops as
/// `action_started` / `action_completed`. We assert both `id` and
/// `schema_version` for the `action_started` op via the helper, then walk the
/// raw log to assert the LEGACY `pack` field is gone (the helper has no
/// "field MUST be absent" mode — consider adding `assert_eventlog_field_absent`).
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b08_event_log_id_and_schema_version() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;
    let result = grex_cli::run(&["sync"], f.path())?;
    assert_success(&result, "grex sync to populate event log");

    let log_path = f.path().join(".grex").join("events.jsonl");

    assertions::assert_eventlog_record_carries_field(&log_path, "action_started", "id")
        .context("B8: action_started missing `id`")?;
    assertions::assert_eventlog_record_carries_field(&log_path, "action_started", "schema_version")
        .context("B8: action_started missing `schema_version`")?;

    let events = read_event_log(f.path())?;
    for ev in &events {
        if ev.get("op").and_then(|v| v.as_str()).is_some_and(|s| s.starts_with("action_")) {
            assert!(
                ev.get("pack").is_none(),
                "B8 regressed: action event still carries legacy `pack` field: {ev}"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B9 — Stub verbs exit non-zero (or carry a stub marker).
// ---------------------------------------------------------------------------

/// B9 (stub verbs exit non-zero).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b09_stub_verbs_exit_nonzero() -> Result<()> {
    let f = WtFixture::new(FIXTURE_LEAF)?;

    for verb in ["status", "update"] {
        let result = grex_cli::run(&[verb], f.path())?;
        let stub_marker = result.stdout.contains("\"stub\":true")
            || result.stdout.contains("unimplemented")
            || result.stderr.contains("unimplemented");
        assert!(
            !result.is_success() || stub_marker,
            "B9 regressed: `grex {verb}` exited 0 with no stub marker\n--- stdout ---\n{}\n--- stderr ---\n{}",
            result.stdout,
            result.stderr
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B10 — `grex add` accepts `--ref`.
// ---------------------------------------------------------------------------

/// B10 (add --ref flag).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b10_add_ref_flag() -> Result<()> {
    let f = WtFixture::new(FIXTURE_LEAF)?;

    let help = grex_cli::run(&["add", "--help"], f.path())?;
    assert!(
        help.stdout.contains("--ref") || help.stderr.contains("--ref"),
        "B10 regressed: `grex add --help` does not advertise --ref\n--- stdout ---\n{}\n--- stderr ---\n{}",
        help.stdout,
        help.stderr
    );

    // Parse-only: a real `add` would attempt a clone. We use --dry-run so the
    // assertion is purely about flag acceptance.
    let parsed = grex_cli::run(
        &["add", "--url", FIXTURE_LEAF, "--ref", "main", "--dry-run", "leaf-pinned"],
        f.path(),
    )?;
    assert!(
        !parsed.stderr.contains("unexpected argument")
            && !parsed.stderr.contains("unrecognized")
            && !parsed.stderr.contains("error: unknown"),
        "B10 regressed: `--ref` rejected as unknown flag\n--- stderr ---\n{}",
        parsed.stderr
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B11 — Lockfile lives under `.grex/`, not workspace root.
// ---------------------------------------------------------------------------

/// B11 (lockfile location).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b11_lockfile_location_under_grex_dir() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;
    let result = grex_cli::run(&["sync"], f.path())?;
    assert_success(&result, "grex sync (lockfile location check)");

    // Helper covers the legacy `.grex-lock` case.
    assert_lockfile_under_grex_dir(f.path()).context("B11: legacy .grex-lock present")?;

    // Belt-and-braces: also block other lock-shaped names at workspace root.
    for orphan in ["grex.lock", ".grex.sync.lock", "grex.lock.yaml"] {
        let p = f.path().join(orphan);
        assert!(!p.exists(), "B11 regressed: lockfile present at workspace root: {}", p.display());
    }
    // And confirm something WAS written under .grex/.
    let (lock_path, _) = read_lockfile(f.path())?;
    assert!(
        lock_path.starts_with(f.path().join(".grex")),
        "B11: lockfile {} not under .grex/",
        lock_path.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B12 — `.gitignore` is not silently mutated.
// ---------------------------------------------------------------------------

/// B12 (.gitignore no silent mutation).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b12_gitignore_no_silent_mutation() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;
    let gi_path = f.path().join(".gitignore");

    // .gitignore may or may not exist on the fixture; if it does, snapshot it.
    let pre = if gi_path.exists() { Some(sha256_of_file(&gi_path)?) } else { None };

    let result = grex_cli::run(&["sync"], f.path())?;
    assert_success(&result, "grex sync (gitignore mutation check)");

    match pre {
        Some(expected) => {
            assert_gitignore_unchanged(f.path(), &expected)
                .context("B12: .gitignore mutated mid-flight")?;
        }
        None => {
            assert!(
                !gi_path.exists(),
                "B12 regressed: sync materialised a .gitignore where none existed pre-run"
            );
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// B13 — Slash-path children sync without "path separators not allowed".
// ---------------------------------------------------------------------------

/// B13 (nested slash-path supported).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b13_nested_slash_path_supported() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_NESTED)?;
    let result = grex_cli::run(&["sync"], f.path())?;
    assert_success(&result, "grex sync on meta-nested (slash paths)");
    assert!(
        !result.stderr.contains("path separators not allowed")
            && !result.stderr.contains("path must not contain"),
        "B13 regressed: slash-path child rejected\n--- stderr ---\n{}",
        result.stderr
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B14 — Lockfile entries carry `branch` from manifest `ref:`.
// ---------------------------------------------------------------------------

/// B14 (lockfile branch carries ref).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b14_lockfile_branch_carries_ref() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;
    let result = grex_cli::run(&["sync"], f.path())?;
    assert_success(&result, "grex sync (branch field check)");

    let (lock_path, lock) = read_lockfile(f.path())?;

    // Walk lockfile entries: any mapping with a `url` key represents a child.
    let mut child_count = 0usize;
    let mut empty_branches = Vec::<String>::new();
    fn walk(v: &serde_yaml::Value, children: &mut usize, empties: &mut Vec<String>) {
        match v {
            serde_yaml::Value::Mapping(m) => {
                if m.contains_key("url") {
                    *children += 1;
                    let branch = m
                        .get("branch")
                        .or_else(|| m.get("ref"))
                        .and_then(|x| x.as_str())
                        .unwrap_or("");
                    if branch.is_empty() {
                        let url =
                            m.get("url").and_then(|u| u.as_str()).unwrap_or("<no url>").to_string();
                        empties.push(url);
                    }
                }
                for (_, v) in m {
                    walk(v, children, empties);
                }
            }
            serde_yaml::Value::Sequence(s) => {
                for item in s {
                    walk(item, children, empties);
                }
            }
            _ => {}
        }
    }
    walk(&lock, &mut child_count, &mut empty_branches);

    assert!(child_count > 0, "B14: lockfile {} has no child entries", lock_path.display());
    assert!(
        empty_branches.is_empty(),
        "B14 regressed: {} child entries with empty `branch`/`ref`: {:?}",
        empty_branches.len(),
        empty_branches
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// B15 — Path collisions emit a warning.
// ---------------------------------------------------------------------------

/// B15 (path collision warning).
/// Source: .omne/var/dogfood-findings-v1.3.0.md
#[test]
#[ignore = "requires network + SSH key + provisioned GH fixtures"]
fn t_b15_path_collision_warn() -> Result<()> {
    let f = WtFixture::new(FIXTURE_META_FLAT)?;

    // Seed a colliding directory at the worktree (manifest declares
    // `path: leaf-1` for a child; we pre-create `leaf-1/` with a marker file
    // to provoke the collision detector).
    let collide = f.path().join("leaf-1");
    std::fs::create_dir_all(&collide)?;
    std::fs::write(collide.join("MARKER"), b"pre-existing")?;

    let result = grex_cli::run(&["sync", "--dry-run"], f.path())?;
    let combined = format!("{}\n{}", result.stdout, result.stderr);
    let lower = combined.to_lowercase();
    assert!(
        lower.contains("collid") || lower.contains("conflict") || lower.contains("already exists"),
        "B15 regressed: no collision warning emitted\n--- stdout ---\n{}\n--- stderr ---\n{}",
        result.stdout,
        result.stderr
    );
    Ok(())
}
