//! `grex exec` — run an arbitrary command inside a pack root.
//!
//! Resolves the pack root via the shared cwd-default helper (same as
//! `sync` / `teardown`). The first positional element is the program
//! name; subsequent elements are passed verbatim. Stdio is inherited
//! in human mode and captured under `--json`. The child's exit code
//! is propagated, clamped at 125 so it never collides with the sync
//! exit-code band (1/2/3).

use crate::cli::args::{ExecArgs, GlobalFlags};
use anyhow::Result;
use std::path::PathBuf;
use std::process::Command;
use tokio_util::sync::CancellationToken;

pub fn run(args: ExecArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    let json = global.json;
    let cwd = match resolve_cwd(args.pack) {
        Some(p) => p,
        None => {
            emit_error(json, "usage", "no pack root: cwd lacks .grex/pack.yaml");
            std::process::exit(2);
        }
    };

    let (program, rest) = match args.cmd.split_first() {
        Some((p, r)) => (p.clone(), r.to_vec()),
        None => {
            // clap enforces required=true, but defend the boundary anyway.
            emit_error(json, "usage", "command is required");
            std::process::exit(2);
        }
    };

    let mut cmd = Command::new(&program);
    cmd.args(&rest).current_dir(&cwd);

    let exit_code = if json {
        run_capture(&mut cmd, &cwd, &program)
    } else {
        run_inherit(&mut cmd, &program)
    };
    std::process::exit(exit_code.min(125));
}

fn resolve_cwd(explicit: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return Some(p);
    }
    let cwd = std::env::current_dir().ok()?;
    if cwd.join(".grex").join("pack.yaml").is_file() {
        Some(cwd)
    } else {
        None
    }
}

fn run_inherit(cmd: &mut Command, program: &str) -> i32 {
    match cmd.status() {
        Ok(status) => status.code().unwrap_or(125),
        Err(err) => {
            eprintln!("grex exec: spawn `{program}` failed: {err}");
            127
        }
    }
}

fn run_capture(cmd: &mut Command, cwd: &std::path::Path, program: &str) -> i32 {
    match cmd.output() {
        Ok(out) => {
            let code = out.status.code().unwrap_or(125);
            let doc = serde_json::json!({
                "verb": "exec",
                "cwd": cwd.display().to_string(),
                "exit_code": code,
                "stdout": String::from_utf8_lossy(&out.stdout).into_owned(),
                "stderr": String::from_utf8_lossy(&out.stderr).into_owned(),
            });
            println!("{}", serde_json::to_string(&doc).unwrap_or_default());
            code
        }
        Err(err) => {
            let doc = serde_json::json!({
                "verb": "exec",
                "error": { "kind": "spawn_failed", "program": program, "message": err.to_string() },
            });
            println!("{}", serde_json::to_string(&doc).unwrap_or_default());
            127
        }
    }
}

fn emit_error(json: bool, kind: &str, msg: &str) {
    if json {
        let doc = serde_json::json!({
            "verb": "exec",
            "error": { "kind": kind, "message": msg },
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        eprintln!("grex exec: {msg}");
    }
}
