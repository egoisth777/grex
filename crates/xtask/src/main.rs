//! `cargo xtask` — internal build-tooling binary.
//!
//! Currently exposes a single subcommand:
//!
//! * `gen-man` — regenerate every Unix man page under `man/` from the
//!   `clap::Command` tree built by `grex-cli`. The output is a passive
//!   projection of the derive tree in `grex-cli::cli::args`; we never
//!   edit the `.1` files by hand. CI's `man-drift` job enforces this
//!   by diffing the committed outputs against a freshly generated set.
//!
//! Usage:
//!
//! ```sh
//! cargo xtask gen-man                    # write to <workspace>/man/
//! cargo xtask gen-man --out-dir /tmp/m   # write elsewhere
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
}

#[derive(clap::Args, Debug)]
struct GenManArgs {
    /// Output directory. Defaults to `<workspace-root>/man`.
    #[arg(long, value_name = "DIR")]
    out_dir: Option<PathBuf>,
}

fn main() -> Result<()> {
    let xt = Xtask::parse();
    match xt.cmd {
        Cmd::GenMan(a) => gen_man(a),
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
