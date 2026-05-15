//! `grex init` — bootstrap a meta-pack at the given path (or cwd).
//!
//! Writes the minimal v1 manifest skeleton (`schema_version: "1"`,
//! `name`, `type: meta`, empty `actions` + `children`) to
//! `<path>/.grex/pack.yaml`. Refuses to overwrite an existing manifest.

use crate::cli::args::{GlobalFlags, InitArgs};
use anyhow::Result;
use std::path::{Path, PathBuf};
use tokio_util::sync::CancellationToken;

/// Derive a pack name from a directory path that satisfies the v1
/// validator `^[a-z][a-z0-9-]*$`. Non-conforming characters become `-`;
/// leading non-alphabetic chars are trimmed; an empty result falls back
/// to `"workspace"`.
fn derive_pack_name(dir: &Path) -> String {
    let raw = dir
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("workspace");
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        let c = ch.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
        } else {
            out.push('-');
        }
    }
    while !out.is_empty()
        && !out.chars().next().unwrap_or('-').is_ascii_lowercase()
    {
        out.remove(0);
    }
    if out.is_empty() {
        "workspace".to_string()
    } else {
        out
    }
}

fn minimal_pack_yaml(name: &str) -> String {
    format!(
        "schema_version: \"1\"\nname: {name}\ntype: meta\nactions: []\nchildren: []\n"
    )
}

pub fn run(args: InitArgs, global: &GlobalFlags, _cancel: &CancellationToken) -> Result<()> {
    match run_impl(args.path, global.json) {
        Outcome::Ok => Ok(()),
        Outcome::AlreadyInitialized => std::process::exit(1),
        Outcome::Io => std::process::exit(2),
    }
}

enum Outcome {
    Ok,
    AlreadyInitialized,
    Io,
}

fn run_impl(path: Option<PathBuf>, json: bool) -> Outcome {
    let dir = match path {
        Some(p) => p,
        None => match std::env::current_dir() {
            Ok(cwd) => cwd,
            Err(err) => {
                emit_error(json, "io", &format!("resolve cwd: {err}"));
                return Outcome::Io;
            }
        },
    };

    if let Err(err) = std::fs::create_dir_all(&dir) {
        emit_error(
            json,
            "io",
            &format!("create workspace dir {}: {err}", dir.display()),
        );
        return Outcome::Io;
    }

    let grex_dir = dir.join(".grex");
    let manifest_path = grex_dir.join("pack.yaml");
    if manifest_path.exists() {
        emit_error(
            json,
            "already_initialized",
            &format!("{} already initialized", dir.display()),
        );
        return Outcome::AlreadyInitialized;
    }

    if let Err(err) = std::fs::create_dir_all(&grex_dir) {
        emit_error(
            json,
            "io",
            &format!("create {}: {err}", grex_dir.display()),
        );
        return Outcome::Io;
    }

    let name = derive_pack_name(&dir);
    if let Err(err) = std::fs::write(&manifest_path, minimal_pack_yaml(&name)) {
        emit_error(
            json,
            "io",
            &format!("write {}: {err}", manifest_path.display()),
        );
        return Outcome::Io;
    }

    emit_ok(json, &dir, &manifest_path);
    Outcome::Ok
}

fn emit_ok(json: bool, dir: &Path, manifest: &Path) {
    if json {
        let doc = serde_json::json!({
            "verb": "init",
            "status": "ok",
            "path": dir.display().to_string(),
            "manifest": manifest.display().to_string(),
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        println!("grex init: wrote {}", manifest.display());
    }
}

fn emit_error(json: bool, kind: &str, msg: &str) {
    if json {
        let doc = serde_json::json!({
            "verb": "init",
            "error": { "kind": kind, "message": msg },
        });
        println!("{}", serde_json::to_string(&doc).unwrap_or_default());
    } else {
        eprintln!("grex init: {msg}");
    }
}

