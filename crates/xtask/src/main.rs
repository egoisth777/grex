//! `cargo xtask` — internal build-tooling binary.
//!
//! Subcommands:
//!
//! * `gen-man` — regenerate every Unix man page under `man/` from the
//!   `clap::Command` tree built by `grex-cli`. The output is a passive
//!   projection of the derive tree in `grex-cli::cli::args`; we never
//!   edit the `.1` files by hand. CI's `man-drift` job enforces this
//!   by diffing the committed outputs against a freshly generated set.
//! * `doc-site-prep` (v1.0.1+) — copy `man/**/*.md` into `grex-doc/src/`
//!   so `mdbook build grex-doc/` can render the documentation site
//!   without symlinks (Windows-hostile). Skips `*.1` man-page binaries
//!   and any pre-existing authored files in `grex-doc/src/`
//!   (`SUMMARY.md`, `introduction.md`).
//!
//! Usage:
//!
//! ```sh
//! cargo xtask gen-man                    # write to <workspace>/man/
//! cargo xtask gen-man --out-dir /tmp/m   # write elsewhere
//! cargo xtask doc-site-prep              # copy man/**/*.md → grex-doc/src/
//! ```
//!
//! Command naming:
//!
//! * Root page → `grex.1`
//! * Subcommands → `grex-<verb>.1` (one per `Verb` variant)
//! * Implicit `help` subcommand → `grex-help.1` (clap auto-injects
//!   `help` into the root command at parse time; we emit a matching man
//!   page because `grex.1`'s SUBCOMMANDS section lists `grex-help(1)`.
//!   Without the page, the list links a non-existent page — bad UX and
//!   a lintable inconsistency).
//!
//! 13 subcommands + 1 implicit `help` + 1 root = 15 files total. Drift
//! against the spec in `crates/grex/src/cli/args.rs` is a CI failure,
//! not a soft warning.

use std::{fs, path::PathBuf};

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use clap_mangen::Man;
use grex_cli::cli::args::Cli;

#[derive(Parser, Debug)]
#[command(
    name = "xtask",
    about = "Internal build tooling for grex (not published).",
    disable_version_flag = true
)]
struct Xtask {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Regenerate man pages under `<workspace-root>/man/` from the
    /// `clap::Command` tree in `grex-cli`.
    GenMan(GenManArgs),
    /// Copy `man/**/*.md` into `grex-doc/src/` so `mdbook build
    /// grex-doc/` can render the documentation site. Skips `*.1` and
    /// the hand-authored `SUMMARY.md` / `introduction.md` already in
    /// `grex-doc/src/`. Idempotent (overwrites; never appends).
    DocSitePrep(DocSitePrepArgs),
}

#[derive(clap::Args, Debug)]
struct GenManArgs {
    /// Output directory. Defaults to `<workspace-root>/man`.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
}

#[derive(clap::Args, Debug)]
struct DocSitePrepArgs {
    /// Source directory holding the `man/**/*.md` tree.
    /// Defaults to `<workspace-root>/man`.
    #[arg(long, value_name = "DIR")]
    src_dir: Option<PathBuf>,
    /// Destination directory rooted at the mdBook `src/`.
    /// Defaults to `<workspace-root>/grex-doc/src`.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let xt = Xtask::parse();
    match xt.cmd {
        Cmd::GenMan(a) => gen_man(a),
        Cmd::DocSitePrep(a) => doc_site_prep(a),
    }
}

/// Resolve the workspace root — the parent of the `crates/` directory
/// that holds this xtask crate. We compute it from `CARGO_MANIFEST_DIR`
/// at build time so the resolved path is deterministic regardless of
/// where `cargo xtask` is invoked from.
fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` is set by Cargo when building xtask and
    // also when running it via `cargo run -p xtask`. It points at
    // `<workspace>/crates/xtask`. Two `pop`s land on `<workspace>`.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop(); // -> crates/
    p.pop(); // -> workspace root
    p
}

fn gen_man(args: GenManArgs) -> Result<()> {
    let out = args.out_dir.unwrap_or_else(|| workspace_root().join("man"));
    fs::create_dir_all(&out).with_context(|| format!("create out_dir {}", out.display()))?;

    // Root `clap::Command`. `Cli::command()` is injected by the derive
    // macro; we re-bind the binary name to `grex` because the bin
    // crate is `grex-cli` but ships the `grex` executable.
    let root = Cli::command().name("grex");

    // Root page: grex.1
    write_man(&root, &out.join("grex.1"))?;

    // One page per subcommand. `get_subcommands()` walks the children
    // of the root `Command`. The derive macro guarantees every `Verb`
    // variant shows up here, so we don't hardcode the list — adding a
    // new verb in `args.rs` surfaces automatically on next `gen-man`.
    //
    // Track names we've emitted so the implicit `help` page is only
    // written once — clap may or may not surface `help` in
    // `get_subcommands()` depending on version; either way we end up
    // with exactly one `grex-help.1` by the time the loop + the explicit
    // fall-back below finish.
    let mut emitted_help = false;
    for sub in root.get_subcommands() {
        // `sub` already has its own `get_name()` (e.g. "init"). We
        // synthesise a fresh `Command` rooted at `grex-<name>` so the
        // generated `.TH` header reads sensibly (man convention is
        // hyphen-joined for subcommand pages).
        let page_name = format!("grex-{}", sub.get_name());
        let file = out.join(format!("{page_name}.1"));
        // `Command::name` requires `impl Into<clap::builder::Str>`, which
        // in practice means `&'static str`. Leak the short, one-shot-per-
        // verb string — the binary exits immediately after generation,
        // so the ~15 tiny allocations never matter. Cloning the subtree
        // first so we can rename without mutating the root tree.
        let leaked: &'static str = Box::leak(page_name.into_boxed_str());
        let cmd = sub.clone().name(leaked);
        write_man(&cmd, &file)?;
        if sub.get_name() == "help" {
            emitted_help = true;
        }
    }

    // clap's auto-injected `help` subcommand is listed in the root man
    // page's SUBCOMMANDS section (from the derive-generated
    // `list_subcommands`), but older clap versions don't surface it via
    // `get_subcommands()`. Emit an explicit `grex-help.1` whenever the
    // loop above did not, so the SUBCOMMANDS link in `grex.1` never
    // dangles.
    if !emitted_help {
        let file = out.join("grex-help.1");
        let help_cmd = clap::Command::new("grex-help")
            .about("Print this message or the help of the given subcommand(s)")
            .disable_help_flag(true)
            .disable_version_flag(true)
            .arg(
                clap::Arg::new("command")
                    .help("Subcommand whose help page to print")
                    .value_name("COMMAND")
                    .num_args(0..),
            );
        write_man(&help_cmd, &file)?;
    }

    Ok(())
}

fn write_man(cmd: &clap::Command, path: &std::path::Path) -> Result<()> {
    // `Man::new` owns the `Command`, so clone is unavoidable here.
    let man = Man::new(cmd.clone());
    let mut buf: Vec<u8> = Vec::new();
    man.render(&mut buf).context("render man page")?;
    fs::write(path, buf).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Filenames inside `grex-doc/src/` that are hand-authored and must NOT
/// be overwritten by `doc-site-prep`. Everything else under
/// `grex-doc/src/` is treated as derived content from `man/**/*.md`.
const DOC_SITE_AUTHORED: &[&str] = &["SUMMARY.md", "introduction.md"];

/// Copy `man/**/*.md` into `grex-doc/src/`, preserving subdirectory
/// structure. Idempotent — overwrites existing copies, never appends.
///
/// Skipped on the source side:
/// * `*.1` — generated troff man pages (binaries to mdBook).
/// * `README.md` — `man/README.md` is the directory entry point; the
///   mdBook site has its own `introduction.md`.
///
/// Skipped on the destination side:
/// * Files in [`DOC_SITE_AUTHORED`] are never overwritten — they are
///   the mdBook site's own scaffolding.
fn doc_site_prep(args: DocSitePrepArgs) -> Result<()> {
    let root = workspace_root();
    let src = args.src_dir.unwrap_or_else(|| root.join("man"));
    let out = args.out_dir.unwrap_or_else(|| root.join("grex-doc").join("src"));

    let files = discover_man_files(&src)?;
    fs::create_dir_all(&out).with_context(|| format!("create out_dir {}", out.display()))?;
    let copied = copy_man_to_site(&files, &src, &out)?;

    println!(
        "doc-site-prep: copied {copied} markdown file(s) from {} into {}",
        src.display(),
        out.display()
    );
    Ok(())
}

/// Walk `src` and return every `*.md` path that should ship into the
/// mdBook site. Skips `README.md` (man/ entry point) and any non-`.md`
/// files (e.g. generated `*.1` troff man pages).
fn discover_man_files(src: &std::path::Path) -> Result<Vec<PathBuf>> {
    if !src.is_dir() {
        anyhow::bail!("doc-site-prep: source dir does not exist: {}", src.display());
    }
    let files: Vec<PathBuf> = walkdir::WalkDir::new(src)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .filter(|p| is_site_source(p))
        .collect();
    Ok(files)
}

/// True when `path` is a markdown file we want to ship into the site.
/// `.md` extension required; `man/README.md` excluded — the mdBook site
/// has its own `introduction.md` landing page.
fn is_site_source(path: &std::path::Path) -> bool {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    name.ends_with(".md") && name != "README.md"
}

/// True when `rel` lands at the destination root AND its filename is in
/// [`DOC_SITE_AUTHORED`]. Used to skip hand-authored scaffolding files
/// (`SUMMARY.md`, `introduction.md`) that already live in
/// `grex-doc/src/`. Nested `*/SUMMARY.md` (unlikely) is *not* protected.
fn is_authored_at_root(rel: &std::path::Path) -> bool {
    let at_root = rel.parent().map(|p| p.as_os_str().is_empty()).unwrap_or(true);
    let name = rel.file_name().and_then(|n| n.to_str()).unwrap_or("");
    at_root && DOC_SITE_AUTHORED.contains(&name)
}

/// Copy each `*.md` under `src` into `out`, preserving the
/// subdirectory layout. Returns the number of files actually written
/// (skipping hand-authored scaffolding files at the destination root).
fn copy_man_to_site(
    files: &[PathBuf],
    src: &std::path::Path,
    out: &std::path::Path,
) -> Result<usize> {
    let mut copied = 0usize;
    for path in files {
        let rel = path
            .strip_prefix(src)
            .with_context(|| format!("strip_prefix on {}", path.display()))?;
        if is_authored_at_root(rel) {
            continue;
        }
        copy_one_file(path, &out.join(rel))?;
        copied += 1;
    }
    Ok(copied)
}

/// Copy a single source file into `dst`, creating the parent directory
/// if it does not already exist. Overwrites any existing destination
/// file (idempotent — running `doc-site-prep` twice yields the same
/// tree).
fn copy_one_file(path: &std::path::Path, dst: &std::path::Path) -> Result<()> {
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create parent {}", parent.display()))?;
    }
    fs::copy(path, dst).with_context(|| format!("copy {} -> {}", path.display(), dst.display()))?;
    Ok(())
}
