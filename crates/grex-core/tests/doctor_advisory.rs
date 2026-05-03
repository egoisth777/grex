//! v1.3.1 (B12) — `grex doctor` advisory: parent-git-tracks-pack-content.
//!
//! Emission contract:
//!
//! * Pack path tracked by parent meta-repo's git index → emit one
//!   `ParentGitTracksPackContent` finding with `Severity::Ok` so the
//!   exit-code roll-up is unaffected (advisory only).
//! * Pack path NOT tracked (or `.gitignore` covers it AND `git rm
//!   --cached` retired the index entry) → no finding emitted. The
//!   probe is `git ls-files --error-unmatch <rel>` so an
//!   ignored-but-still-tracked file DOES trigger the advisory by
//!   design — the right fix is to also run `git rm --cached`.
//! * Parent dir is not a git repo → no finding emitted; advisory is
//!   mute when there is no parent meta-repo to advise about.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use grex_core::doctor::{check_parent_git_tracks_pack_content, CheckKind, Severity};
use grex_core::manifest::{append_event, Event, SCHEMA_VERSION};
use tempfile::tempdir;

fn ts() -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc.with_ymd_and_hms(2026, 5, 2, 10, 0, 0).unwrap()
}

/// Seed a meta directory at `meta_dir` with one registered pack `id`
/// at sub-path `id` (a `declarative` pack-type). Creates the on-disk
/// pack directory plus a `.grex/events.jsonl` entry — same shape the
/// doctor unit tests use.
fn seed_meta_with_pack(meta_dir: &Path, id: &str) {
    fs::create_dir_all(meta_dir.join(id).join(".grex")).unwrap();
    fs::write(
        meta_dir.join(id).join(".grex").join("pack.yaml"),
        format!("schema_version: \"1\"\nname: {id}\ntype: declarative\n"),
    )
    .unwrap();
    let m = meta_dir.join(".grex").join("events.jsonl");
    fs::create_dir_all(m.parent().unwrap()).unwrap();
    append_event(
        &m,
        &Event::Add {
            ts: ts(),
            id: id.into(),
            url: format!("https://example.invalid/{id}.git"),
            path: id.into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        },
    )
    .unwrap();
}

/// Run `git init -q` in `dir`. Configures user.email/user.name so
/// commits don't fail on stripped-down CI runners.
fn git_init(dir: &Path) {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["init", "-q"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git init");
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.email", "doctor@grex.test"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git config email");
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["config", "user.name", "doctor-test"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git config name");
}

fn git_add_all_and_commit(dir: &Path) {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["add", "-A"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git add");
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["commit", "-q", "-m", "seed"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git commit");
}

/// Helper: read packs map for the meta dir.
fn folded_packs(
    meta_dir: &Path,
) -> std::collections::HashMap<String, grex_core::manifest::PackState> {
    let m = meta_dir.join(".grex").join("events.jsonl");
    let evs = grex_core::manifest::read_all(&m).unwrap();
    grex_core::manifest::fold(evs)
}

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// 1. Parent git tracks pack content → advisory emitted, severity Ok.
#[test]
fn parent_git_tracking_pack_emits_advisory() {
    if !git_available() {
        eprintln!("git binary unavailable; skipping");
        return;
    }
    let tmp = tempdir().unwrap();
    let parent = tmp.path().join("parent");
    let meta = parent.join("meta");
    fs::create_dir_all(&meta).unwrap();

    // Initialise the parent meta-repo and stage the meta dir contents
    // so the pack path lives in the parent's git index.
    git_init(&parent);
    seed_meta_with_pack(&meta, "alpha");
    // Need at least one tracked file inside the pack so ls-files
    // matches the pack path itself (or any file beneath it).
    fs::write(meta.join("alpha").join("README.md"), "hi\n").unwrap();
    git_add_all_and_commit(&parent);

    let packs = folded_packs(&meta);
    let result = check_parent_git_tracks_pack_content(&meta, Some(&packs));
    let alpha_findings: Vec<_> = result
        .findings
        .iter()
        .filter(|f| {
            f.check == CheckKind::ParentGitTracksPackContent && f.pack.as_deref() == Some("alpha")
        })
        .collect();
    assert_eq!(
        alpha_findings.len(),
        1,
        "expected exactly one advisory for `alpha`; got {:?}",
        result.findings,
    );
    let f = alpha_findings[0];
    assert_eq!(f.severity, Severity::Ok, "advisory must not bump exit code");
    assert!(
        f.detail.contains("tracked by the parent meta-repo"),
        "detail must explain the situation; got: {}",
        f.detail,
    );
}

// 2. Pack already in parent .gitignore but STILL tracked in index →
//    advisory still fires. This is the documented edge case: the
//    probe is `git ls-files --error-unmatch`, which sees the index
//    entry regardless of `.gitignore`. The right fix for the operator
//    is to also run `git rm --cached <path>`.
#[test]
fn ignored_but_still_tracked_pack_still_fires() {
    if !git_available() {
        eprintln!("git binary unavailable; skipping");
        return;
    }
    let tmp = tempdir().unwrap();
    let parent = tmp.path().join("parent");
    let meta = parent.join("meta");
    fs::create_dir_all(&meta).unwrap();

    git_init(&parent);
    seed_meta_with_pack(&meta, "beta");
    fs::write(meta.join("beta").join("README.md"), "hi\n").unwrap();
    git_add_all_and_commit(&parent);
    // Now retro-actively add the pack to .gitignore and commit. The
    // index still holds the entry, so ls-files --error-unmatch
    // still matches.
    fs::write(parent.join(".gitignore"), "meta/beta/\n").unwrap();
    git_add_all_and_commit(&parent);

    let packs = folded_packs(&meta);
    let result = check_parent_git_tracks_pack_content(&meta, Some(&packs));
    let hits: Vec<_> = result
        .findings
        .iter()
        .filter(|f| f.check == CheckKind::ParentGitTracksPackContent)
        .collect();
    assert_eq!(
        hits.len(),
        1,
        "advisory must still fire on ignored-but-tracked path: {:?}",
        result.findings,
    );
}

// 3. Parent dir is not a git repo → no finding. The advisory is mute
//    when there is no parent meta-repo to advise about.
#[test]
fn no_parent_git_repo_emits_no_finding() {
    let tmp = tempdir().unwrap();
    let standalone = tmp.path().join("standalone");
    fs::create_dir_all(&standalone).unwrap();
    seed_meta_with_pack(&standalone, "gamma");
    fs::write(standalone.join("gamma").join("README.md"), "hi\n").unwrap();

    // No `git init` anywhere up the tree.
    let packs = folded_packs(&standalone);
    let result = check_parent_git_tracks_pack_content(&standalone, Some(&packs));
    assert!(
        result.findings.is_empty(),
        "advisory must be mute outside a git repo: {:?}",
        result.findings,
    );
}
