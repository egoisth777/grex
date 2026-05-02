//! v1.2.6 — drift regression test.
//!
//! Asserts that the patterns shipped in the workspace `.gitignore` (the
//! `.grex/` runtime state directory and the `*statusline-probe*.txt`
//! CWD-fossil glob) actually cause `git check-ignore` to ignore matching
//! files inside a fresh git repo. Prevents accidental regressions where
//! someone trims the patterns and the fossils start showing up in
//! `git status` again.
//!
//! Auto-skipped when `git` is not on PATH (`git --version` fails) so the
//! test does not block environments that lack git (Lean-only CI shards,
//! minimal containers, etc.).

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

/// Mirror of the v1.2.6 drift-prevention block in the workspace `.gitignore`.
/// Kept inline (not loaded from disk) so the test exercises the *contract*,
/// not whatever the on-disk file happens to say at the moment.
const GITIGNORE_DRIFT_BLOCK: &str = "\
.grex/
**/.grex/
claude-statusline-probe.txt
*statusline-probe*.txt
";

fn git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn git_init(dir: &Path) -> bool {
    let status = Command::new("git").arg("-C").arg(dir).args(["init", "-q"]).status();
    matches!(status, Ok(s) if s.success())
}

/// Returns `Some(true)` when `git check-ignore` reports the path is ignored,
/// `Some(false)` when not ignored, `None` on subprocess error.
fn check_ignored(repo: &Path, rel: &str) -> Option<bool> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["check-ignore", "-q", "--", rel])
        .status()
        .ok()?;
    // Per `git help check-ignore`: exit 0 = ignored, 1 = not ignored, 128 = error.
    match out.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

#[test]
fn drift_files_remain_gitignored() {
    if !git_available() {
        eprintln!("drift_norec: `git` not on PATH — skipping (env without git CLI)");
        return;
    }

    let tmp = TempDir::new().expect("tempdir");
    let repo = tmp.path();
    assert!(git_init(repo), "git init failed in tempdir");

    // Seed the .gitignore with the same drift-prevention block the workspace ships.
    fs::write(repo.join(".gitignore"), GITIGNORE_DRIFT_BLOCK).expect("write .gitignore");

    // Materialise the two drift artifacts the v1.2.6 cleanup targets:
    //   1. nested .grex/events.jsonl  (runtime state directory)
    //   2. claude-statusline-probe.txt at repo root  (cc-cfg CWD fossil)
    let nested = repo.join("nested-pack").join(".grex");
    fs::create_dir_all(&nested).expect("mkdir nested .grex");
    fs::write(nested.join("events.jsonl"), b"{}\n").expect("write events.jsonl");

    let probe = repo.join("claude-statusline-probe.txt");
    fs::write(&probe, b"probe\n").expect("write probe");

    // git check-ignore takes paths relative to the repo root; use forward slashes
    // (git accepts them on Windows too).
    let cases: &[(&str, &str)] = &[
        ("nested-pack/.grex/events.jsonl", "runtime state dir contents"),
        ("nested-pack/.grex", "runtime state dir itself"),
        ("claude-statusline-probe.txt", "cc-cfg CWD fossil"),
    ];

    for (rel, label) in cases {
        match check_ignored(repo, rel) {
            Some(true) => {} // expected
            Some(false) => panic!(
                "drift regression: `{rel}` ({label}) is NOT gitignored — \
                 .gitignore drift-prevention block is broken"
            ),
            None => panic!("git check-ignore failed unexpectedly for `{rel}`"),
        }
    }
}
