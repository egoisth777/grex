//! Scripted user-journey runner for real-smoke tests.
//!
//! A [`Journey`] is an ordered list of [`Step`]s, each describing a
//! `grex` invocation plus assertions on the captured stdout / stderr /
//! exit code and the on-disk state after the call. Steps run in order,
//! short-circuiting on the first hard failure. The runner accumulates
//! per-step outcomes and renders a single, human-readable
//! [`JourneyReport`] that test bodies can `.unwrap()` (hard fail) or
//! inspect for soft assertions.
//!
//! # Design intent
//!
//! Smoke tests want to model a *user session*, not a single command:
//! `init` → `add` → `add` → `ls` → `sync` → `doctor`. Spelling that as
//! seven separate `#[test]`s loses the temporal coupling (state from
//! one feeds the next) and re-spelunks setup boilerplate each time.
//! A [`Journey`] keeps that flow in one place.
//!
//! # No magic
//!
//! The runner is deliberately thin. Steps execute via
//! [`crate::grex_cli::run`]; assertions are plain closures returning
//! `Result<()>`. No proc-macros, no DSL parser — adding a new
//! assertion is a one-liner.

use anyhow::{anyhow, Result};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::grex_cli::{run as run_cli, CliResult};

/// Closure type for step assertions. Receives the captured CLI result
/// and the workspace path; returns `Ok(())` on pass or `Err(reason)`
/// on fail.
pub type Assertion = Box<dyn Fn(&CliResult, &Path) -> Result<()>>;

/// Single command + assertions inside a journey.
pub struct Step {
    pub label: String,
    pub args: Vec<String>,
    pub expect_exit: ExitExpectation,
    /// Optional assertions. See [`Assertion`] for the closure signature.
    pub assertions: Vec<Assertion>,
}

/// What the step expects from the grex exit code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitExpectation {
    /// Exit code must be exactly `0`.
    Success,
    /// Exit code must match this specific value.
    Code(i32),
    /// Exit code is informational — any value is accepted.
    Any,
}

impl Step {
    pub fn new(label: impl Into<String>, args: &[&str]) -> Self {
        Self {
            label: label.into(),
            args: args.iter().map(|s| (*s).to_string()).collect(),
            expect_exit: ExitExpectation::Success,
            assertions: Vec::new(),
        }
    }

    pub fn exit(mut self, code: i32) -> Self {
        self.expect_exit = ExitExpectation::Code(code);
        self
    }

    pub fn exit_any(mut self) -> Self {
        self.expect_exit = ExitExpectation::Any;
        self
    }

    pub fn assert(mut self, f: impl Fn(&CliResult, &Path) -> Result<()> + 'static) -> Self {
        self.assertions.push(Box::new(f));
        self
    }
}

/// Top-level journey runner.
pub struct Journey {
    pub name: String,
    pub workspace: PathBuf,
    pub steps: Vec<Step>,
}

impl Journey {
    pub fn new(name: impl Into<String>, workspace: impl Into<PathBuf>) -> Self {
        Self { name: name.into(), workspace: workspace.into(), steps: Vec::new() }
    }

    pub fn step(mut self, s: Step) -> Self {
        self.steps.push(s);
        self
    }

    /// Run every step in order. Returns the report regardless of
    /// individual step outcomes; callers decide whether to fail-fast.
    pub fn run(&self) -> JourneyReport {
        let mut report = JourneyReport::new(&self.name);
        for step in &self.steps {
            let outcome = self.run_one(step);
            let halted = outcome.fatal;
            report.steps.push(outcome);
            if halted {
                break;
            }
        }
        report
    }

    fn run_one(&self, step: &Step) -> StepOutcome {
        let args: Vec<&str> = step.args.iter().map(String::as_str).collect();
        let result = match run_cli(&args, &self.workspace) {
            Ok(r) => r,
            Err(err) => {
                return StepOutcome {
                    label: step.label.clone(),
                    args: step.args.clone(),
                    cli: None,
                    failures: vec![format!("spawn error: {err}")],
                    fatal: true,
                };
            }
        };
        let mut failures = Vec::new();
        match step.expect_exit {
            ExitExpectation::Success if !result.is_success() => {
                failures.push(format!(
                    "expected exit 0, got {} (stderr: {:?})",
                    result.exit_code,
                    truncate(&result.stderr, 256)
                ));
            }
            ExitExpectation::Code(c) if result.exit_code != c => {
                failures.push(format!("expected exit {c}, got {}", result.exit_code));
            }
            _ => {}
        }
        for assertion in &step.assertions {
            if let Err(err) = assertion(&result, &self.workspace) {
                failures.push(err.to_string());
            }
        }
        let fatal = !failures.is_empty();
        StepOutcome {
            label: step.label.clone(),
            args: step.args.clone(),
            cli: Some(result),
            failures,
            fatal,
        }
    }
}

pub struct StepOutcome {
    pub label: String,
    pub args: Vec<String>,
    pub cli: Option<CliResult>,
    pub failures: Vec<String>,
    pub fatal: bool,
}

impl StepOutcome {
    pub fn is_ok(&self) -> bool {
        self.failures.is_empty()
    }

    pub fn elapsed(&self) -> Option<Duration> {
        self.cli.as_ref().map(|c| c.elapsed)
    }
}

pub struct JourneyReport {
    pub name: String,
    pub steps: Vec<StepOutcome>,
}

impl JourneyReport {
    fn new(name: &str) -> Self {
        Self { name: name.to_string(), steps: Vec::new() }
    }

    /// `Ok(())` when every step's assertions passed; otherwise the
    /// human-readable journey transcript with each failing step
    /// annotated. Use as `report.into_result().unwrap()` for hard
    /// failure or `.into_result().is_err()` for soft inspection.
    pub fn into_result(self) -> Result<()> {
        if self.steps.iter().all(StepOutcome::is_ok) {
            return Ok(());
        }
        Err(anyhow!("{}", self.render()))
    }

    pub fn render(&self) -> String {
        let mut buf = String::new();
        writeln!(buf, "journey {}: failed", self.name).ok();
        for (idx, s) in self.steps.iter().enumerate() {
            let status = if s.is_ok() { "ok" } else { "FAIL" };
            let elapsed = s.elapsed().map(|d| format!(" ({:?})", d)).unwrap_or_default();
            writeln!(buf, "  step {idx} [{status}] {label}{elapsed}", label = s.label).ok();
            if !s.failures.is_empty() {
                writeln!(buf, "    args: {:?}", s.args).ok();
                if let Some(cli) = &s.cli {
                    if !cli.stdout.is_empty() {
                        writeln!(buf, "    stdout: {:?}", truncate(&cli.stdout, 1024)).ok();
                    }
                    if !cli.stderr.is_empty() {
                        writeln!(buf, "    stderr: {:?}", truncate(&cli.stderr, 1024)).ok();
                    }
                }
                for fail in &s.failures {
                    writeln!(buf, "    - {fail}").ok();
                }
            }
        }
        buf
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…(+{} bytes)", &s[..max], s.len() - max)
    }
}

// ---------- assertion helpers ----------

/// Asserts `result.stdout` contains every `needle`. Returns the first
/// missing needle on failure.
pub fn stdout_contains_all<'a>(
    needles: impl IntoIterator<Item = &'a str>,
) -> impl Fn(&CliResult, &Path) -> Result<()> + 'static {
    let needles: Vec<String> = needles.into_iter().map(str::to_string).collect();
    move |cli, _| {
        for n in &needles {
            if !cli.stdout.contains(n) {
                return Err(anyhow!("stdout missing `{n}`; got: {:?}", truncate(&cli.stdout, 512)));
            }
        }
        Ok(())
    }
}

/// Asserts a file exists at `<workspace>/<rel>`.
pub fn file_exists(
    rel: impl AsRef<Path> + 'static,
) -> impl Fn(&CliResult, &Path) -> Result<()> + 'static {
    let rel = rel.as_ref().to_path_buf();
    move |_, ws| {
        let p = ws.join(&rel);
        if !p.exists() {
            return Err(anyhow!("file missing: {}", p.display()));
        }
        Ok(())
    }
}

/// Asserts a file or directory at `<workspace>/<rel>` does NOT exist.
pub fn path_absent(
    rel: impl AsRef<Path> + 'static,
) -> impl Fn(&CliResult, &Path) -> Result<()> + 'static {
    let rel = rel.as_ref().to_path_buf();
    move |_, ws| {
        let p = ws.join(&rel);
        if p.exists() {
            return Err(anyhow!("path must NOT exist: {}", p.display()));
        }
        Ok(())
    }
}

/// Asserts that the `.grex/events.jsonl` log under `<workspace>` has
/// exactly `expected_adds` records with `op == "add"`.
pub fn events_jsonl_add_count(
    expected_adds: usize,
) -> impl Fn(&CliResult, &Path) -> Result<()> + 'static {
    move |_, ws| {
        let p = ws.join(".grex/events.jsonl");
        let raw =
            std::fs::read_to_string(&p).map_err(|err| anyhow!("read {}: {}", p.display(), err))?;
        let actual = raw
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v.get("op").and_then(|x| x.as_str()) == Some("add"))
            .count();
        if actual != expected_adds {
            return Err(anyhow!("events.jsonl: expected {expected_adds} add rows, got {actual}",));
        }
        Ok(())
    }
}

/// Asserts that `<workspace>/.grex/pack.yaml` lists each `child_path`
/// under its `children:` sequence.
pub fn pack_yaml_has_children(
    paths: &'static [&'static str],
) -> impl Fn(&CliResult, &Path) -> Result<()> + 'static {
    move |_, ws| {
        let p = ws.join(".grex/pack.yaml");
        let body =
            std::fs::read_to_string(&p).map_err(|err| anyhow!("read {}: {}", p.display(), err))?;
        for child in paths {
            let needle = format!("path: {child}");
            if !body.contains(&needle) {
                return Err(anyhow!("pack.yaml missing `{needle}`; full body:\n{body}"));
            }
        }
        Ok(())
    }
}
