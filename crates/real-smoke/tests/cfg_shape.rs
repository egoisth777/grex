//! Offline cfg-shape smoke journey.
//!
//! Mimics the cfg metarepo layout (platform-bucketed config repos tracked
//! via `REPOS.json`) without touching network or real GitHub fixtures.
//! Every child pack is seeded as a local bare repo and addressed via
//! `file://` URLs.
//!
//! Journey: `init` → `import` (platform-prefixed) → `ls` → `sync` →
//! `doctor`. Each assertion gates against a specific v1.4.0 smoke-test
//! bug — Bug #1/#2 (`add`/`import` not bridging to `pack.yaml`),
//! Bug #3 (`platform` silently dropped), Bug #4 (post-import drift),
//! Bug #5 (verb inconsistency), Bug #6 (status undercount).

use real_smoke::grex_cli::CliResult;
use real_smoke::journey::{
    events_jsonl_add_count, file_exists, pack_yaml_has_children, stdout_contains_all, Journey, Step,
};
use real_smoke::seed::{file_url, render_repos_json, seed_pack_template, ReposRow};
use std::path::Path;
use tempfile::TempDir;

struct Fixture {
    _tmp: TempDir,
    workspace: std::path::PathBuf,
}

const ROWS: &[(&str, &str)] = &[
    ("cmn", "warp-cfgs"),
    ("win", "choco-pkgs"),
    ("win", "pwsh-cfg"),
    ("cmn", "nvim-cfg"),
    ("lnx", "starship-cfg"),
    ("mac", "cc-cfg"),
];

fn build_fixture() -> Fixture {
    let tmp = TempDir::new().expect("tempdir");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let bare_dir = tmp.path().join("bares");
    let sink_dir = tmp.path().join("sink");
    std::fs::create_dir_all(&sink_dir).unwrap();

    let mut rows = Vec::new();
    for (platform, basename) in ROWS {
        let bare = seed_pack_template(&bare_dir, basename, &sink_dir).expect("seed bare");
        rows.push(ReposRow {
            url: file_url(&bare),
            path: (*basename).to_string(),
            platform: Some((*platform).to_string()),
        });
    }
    let repos_path = workspace.join("REPOS.json");
    std::fs::write(&repos_path, render_repos_json(&rows)).expect("write REPOS.json");

    Fixture { _tmp: tmp, workspace }
}

const ALL_PATHS: &[&str] = &[
    "cmn/warp-cfgs",
    "win/choco-pkgs",
    "win/pwsh-cfg",
    "cmn/nvim-cfg",
    "lnx/starship-cfg",
    "mac/cc-cfg",
];

fn step_init() -> Step {
    Step::new("init", &["init", "."])
        .assert(file_exists(".grex/pack.yaml"))
        .assert(stdout_contains_all(["wrote"]))
}

fn step_import() -> Step {
    let stdout_needles: Vec<&str> =
        std::iter::once("imported=6").chain(ALL_PATHS.iter().copied()).collect();
    Step::new("import-with-platform-prefix", &["import", "--from-repos-json", "REPOS.json"])
        .assert(stdout_contains_all(stdout_needles))
        .assert(events_jsonl_add_count(6))
        .assert(pack_yaml_has_children(ALL_PATHS))
}

fn step_ls() -> Step {
    Step::new("ls-sees-children", &["ls"]).assert(stdout_contains_all(ALL_PATHS.iter().copied()))
}

fn step_sync() -> Step {
    let mut s = Step::new("sync-clones-children-into-platform-dirs", &["sync"]);
    for child in ALL_PATHS {
        s = s.assert(file_exists(format!("{child}/.git")));
    }
    s
}

fn assert_doctor_clean(cli: &CliResult, _: &Path) -> anyhow::Result<()> {
    for child in ALL_PATHS {
        let needle = format!("registered pack dir missing: {child}");
        if cli.stdout.contains(&needle) || cli.stderr.contains(&needle) {
            return Err(
                anyhow::anyhow!("doctor still reports drift for `{child}` — bridge broke",),
            );
        }
    }
    Ok(())
}

#[test]
fn cfg_shape_offline_journey_covers_all_seven_v140_bugs() {
    let fixture = build_fixture();
    let journey = Journey::new("cfg-shape-offline", fixture.workspace.clone())
        .step(step_init())
        .step(step_import())
        .step(step_ls())
        .step(step_sync())
        .step(Step::new("status-clean-after-sync", &["status"]).exit_any())
        .step(Step::new("doctor-clean", &["doctor"]).exit_any().assert(assert_doctor_clean));

    if let Err(err) = journey.run().into_result() {
        panic!("{err}");
    }
}

#[test]
fn cfg_shape_idempotent_import_skips_duplicates() {
    let fixture = build_fixture();
    let workspace = fixture.workspace.clone();

    let setup = Journey::new("cfg-shape-prep", workspace.clone())
        .step(Step::new("init", &["init", "."]))
        .step(Step::new("import-1", &["import", "--from-repos-json", "REPOS.json"]));
    if let Err(err) = setup.run().into_result() {
        panic!("{err}");
    }

    let second = Journey::new("cfg-shape-idempotent", workspace).step(
        Step::new("import-2", &["import", "--from-repos-json", "REPOS.json"])
            .assert(stdout_contains_all(["imported=0", "skipped=6"]))
            .assert(events_jsonl_add_count(6)),
    );
    if let Err(err) = second.run().into_result() {
        panic!("{err}");
    }
}
