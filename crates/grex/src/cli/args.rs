use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(name = "grex", version, about = "Pack-based dev-env orchestrator", long_about = None)]
pub struct Cli {
    #[command(flatten)]
    pub global: GlobalFlags,

    #[command(subcommand)]
    pub verb: Verb,
}

#[derive(Args, Debug)]
pub struct GlobalFlags {
    /// Emit output as JSON.
    #[arg(long, global = true, conflicts_with = "plain")]
    pub json: bool,

    /// Emit plain (non-color, non-table) output.
    #[arg(long, global = true)]
    pub plain: bool,

    /// Show planned actions without executing them.
    #[arg(long, global = true)]
    pub dry_run: bool,

    /// Filter packs by expression.
    #[arg(long, global = true)]
    pub filter: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Verb {
    /// Initialize a grex workspace.
    Init(InitArgs),
    /// Register and clone a pack.
    Add(AddArgs),
    /// Teardown and remove a pack.
    Rm(RmArgs),
    /// List registered packs.
    Ls(LsArgs),
    /// Report drift vs lockfile.
    Status(StatusArgs),
    /// Git fetch and pull (recurse by default).
    Sync(SyncArgs),
    /// Sync plus re-run install on lock change.
    Update(UpdateArgs),
    /// Run integrity checks.
    Doctor(DoctorArgs),
    /// Start MCP stdio server.
    Serve(ServeArgs),
    /// Import legacy REPOS.json.
    Import(ImportArgs),
    /// Run a named action across packs.
    Run(RunArgs),
    /// Execute a shell command in pack context.
    Exec(ExecArgs),
    /// Tear down a pack tree (reverse of `sync`/`install`).
    Teardown(TeardownArgs),
}

#[derive(Args, Debug)]
pub struct InitArgs {}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// Git URL of the pack repo.
    pub url: String,
    /// Optional local path (defaults to repo name).
    pub path: Option<String>,
}

#[derive(Args, Debug)]
pub struct RmArgs {
    /// Local path of the pack to remove.
    pub path: String,
}

#[derive(Args, Debug)]
pub struct LsArgs {}

#[derive(Args, Debug)]
pub struct StatusArgs {}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Recurse into child packs.
    #[arg(long, default_value_t = true)]
    pub recursive: bool,

    /// Pack root. Directory holding `.grex/pack.yaml`, or the YAML file
    /// itself. When omitted, `sync` prints the legacy M1 stub and exits 0.
    pub pack_root: Option<std::path::PathBuf>,

    /// Workspace directory for cloned children. Defaults to
    /// `<pack_root>/.grex/workspace`.
    #[arg(long)]
    pub workspace: Option<std::path::PathBuf>,

    /// Plan actions without touching the filesystem.
    #[arg(long, short = 'n')]
    pub dry_run: bool,

    /// Suppress per-action log lines.
    #[arg(long, short = 'q')]
    pub quiet: bool,

    /// Skip plan-phase validators. Debug-only escape hatch.
    #[arg(long)]
    pub no_validate: bool,

    /// Override the default ref for every pack in this sync invocation.
    /// Accepts a branch, tag, or commit SHA. Empty strings are rejected.
    #[arg(long = "ref", value_name = "REF", value_parser = non_empty_string)]
    pub ref_override: Option<String>,

    /// Restrict sync to packs whose workspace-relative path (or name)
    /// matches the glob. Repeat the flag to OR-combine multiple patterns
    /// (standard `*`/`**`/`?` semantics). Non-matching packs are skipped
    /// entirely — no action execution, no lockfile write.
    #[arg(long = "only", value_name = "GLOB", value_parser = non_empty_string)]
    pub only: Vec<String>,

    /// Re-execute every pack even when its `actions_hash` is unchanged
    /// from the prior lockfile. Overrides the M4-B skip-on-hash
    /// short-circuit; dry-run semantics are unchanged.
    #[arg(long)]
    pub force: bool,

    /// Max parallel pack ops during this sync run (feat-m6-1).
    ///
    /// Semantics:
    /// * Absent → default `num_cpus::get()` resolved in `verbs::sync`.
    /// * `0` → unbounded (`Semaphore::MAX_PERMITS`).
    /// * `1` → serial fast-path (preserves pre-M6 wall-order).
    /// * `2..=1024` → bounded parallel.
    ///
    /// Env fallback: `GREX_PARALLEL` is honoured only when the flag is
    /// absent. Clap reads the env var automatically via `env`.
    ///
    /// Distinct from the global `--parallel` on [`GlobalFlags`]; that
    /// knob is documented as the harness-level worker cap and rejects
    /// `0`. Sync parallelism uses `0` as the "unbounded" sentinel per
    /// `.omne/cfg/concurrency.md`.
    #[arg(
        long = "parallel",
        env = "GREX_PARALLEL",
        value_parser = clap::value_parser!(u32).range(0..=1024),
    )]
    pub parallel: Option<u32>,
}

/// Clap `value_parser` that rejects empty or whitespace-only strings.
/// Keeps `--ref ""`, `--ref " "`, `--only ""`, `--only "\t"` off the
/// fast path. Whitespace-only values are rejected because they
/// degrade silently inside the walker / globset layers rather than
/// producing a useful error.
fn non_empty_string(s: &str) -> Result<String, String> {
    if s.trim().is_empty() {
        Err("value must not be empty or whitespace-only".to_string())
    } else {
        Ok(s.to_string())
    }
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Optional pack path; if omitted, update all.
    pub pack: Option<String>,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Path to the `grex.jsonl` event-log manifest. Captured at server
    /// launch and immutable for the session (per spec §"Manifest binding").
    /// Defaults to `<cwd>/grex.jsonl` when omitted.
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<std::path::PathBuf>,

    /// Workspace root the MCP server resolves relative paths against.
    /// Defaults to the current working directory when omitted.
    #[arg(long, value_name = "PATH")]
    pub workspace: Option<std::path::PathBuf>,

    /// Harness-level worker cap inherited by the MCP server's
    /// `Scheduler` (feat-m7-1 stage 8.3). `1` = serial; range `1..=1024`.
    /// Defaults to `std::thread::available_parallelism()` when omitted.
    /// Distinct from `sync --parallel` which uses `0` = unbounded.
    #[arg(
        long = "parallel",
        value_parser = clap::value_parser!(u32).range(1..=1024),
    )]
    pub parallel: Option<u32>,
}

#[derive(Args, Debug)]
pub struct ImportArgs {
    /// Path to a legacy REPOS.json file.
    #[arg(long)]
    pub from_repos_json: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct RunArgs {
    /// Action name to run.
    pub action: String,
}

#[derive(Args, Debug)]
pub struct ExecArgs {
    /// Shell command and args to execute.
    #[arg(trailing_var_arg = true)]
    pub cmd: Vec<String>,
}

#[derive(Args, Debug)]
pub struct TeardownArgs {
    /// Pack root. Directory holding `.grex/pack.yaml`, or the YAML file
    /// itself. When omitted, `teardown` prints a usage stub and exits 0.
    pub pack_root: Option<std::path::PathBuf>,

    /// Workspace directory. Defaults to `<pack_root>/.grex/workspace`.
    #[arg(long)]
    pub workspace: Option<std::path::PathBuf>,

    /// Suppress per-action log lines.
    #[arg(long, short = 'q')]
    pub quiet: bool,

    /// Skip plan-phase validators. Debug-only escape hatch.
    #[arg(long)]
    pub no_validate: bool,
}

#[cfg(test)]
mod tests {
    //! Direct-parse unit tests. These bypass the spawned binary and hit
    //! `Cli::try_parse_from` in-process — much faster than `assert_cmd`.
    use super::*;
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Cli, clap::Error> {
        // clap's `try_parse_from` expects argv[0] to be the binary name.
        let mut full = vec!["grex"];
        full.extend_from_slice(args);
        Cli::try_parse_from(full)
    }

    #[test]
    fn init_parses_to_init_variant() {
        let cli = parse(&["init"]).expect("init parses");
        assert!(matches!(cli.verb, Verb::Init(_)));
    }

    #[test]
    fn add_parses_url_and_optional_path() {
        let cli = parse(&["add", "https://example.com/repo.git"]).expect("add url parses");
        match cli.verb {
            Verb::Add(a) => {
                assert_eq!(a.url, "https://example.com/repo.git");
                assert!(a.path.is_none());
            }
            _ => panic!("expected Add variant"),
        }

        let cli = parse(&["add", "https://example.com/repo.git", "local"])
            .expect("add url + path parses");
        match cli.verb {
            Verb::Add(a) => {
                assert_eq!(a.url, "https://example.com/repo.git");
                assert_eq!(a.path.as_deref(), Some("local"));
            }
            _ => panic!("expected Add variant"),
        }
    }

    #[test]
    fn rm_parses_path() {
        let cli = parse(&["rm", "pack-a"]).expect("rm parses");
        match cli.verb {
            Verb::Rm(a) => assert_eq!(a.path, "pack-a"),
            _ => panic!("expected Rm variant"),
        }
    }

    #[test]
    fn sync_recursive_defaults_to_true() {
        let cli = parse(&["sync"]).expect("sync parses");
        match cli.verb {
            Verb::Sync(a) => assert!(a.recursive, "sync should default to recursive=true"),
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn update_pack_is_optional() {
        let cli = parse(&["update"]).expect("update parses bare");
        match cli.verb {
            Verb::Update(a) => assert!(a.pack.is_none()),
            _ => panic!("expected Update variant"),
        }

        let cli = parse(&["update", "mypack"]).expect("update parses w/ pack");
        match cli.verb {
            Verb::Update(a) => assert_eq!(a.pack.as_deref(), Some("mypack")),
            _ => panic!("expected Update variant"),
        }
    }

    #[test]
    fn exec_collects_trailing_args() {
        let cli = parse(&["exec", "echo", "hi", "there"]).expect("exec parses");
        match cli.verb {
            Verb::Exec(a) => assert_eq!(a.cmd, vec!["echo", "hi", "there"]),
            _ => panic!("expected Exec variant"),
        }
    }

    #[test]
    fn universal_flags_populate_on_any_verb() {
        // `--json` and `--plain` are mutually exclusive, so split into two
        // parses to exercise the remaining flags on both modes.
        let cli = parse(&["ls", "--json", "--dry-run", "--filter", "kind=git"])
            .expect("ls w/ json+dry-run+filter parses");
        assert!(cli.global.json);
        assert!(!cli.global.plain);
        assert!(cli.global.dry_run);
        assert_eq!(cli.global.filter.as_deref(), Some("kind=git"));

        let cli = parse(&["ls", "--plain", "--dry-run"]).expect("ls w/ plain+dry-run parses");
        assert!(!cli.global.json);
        assert!(cli.global.plain);
    }

    #[test]
    fn json_and_plain_conflict() {
        let err =
            parse(&["init", "--json", "--plain"]).expect_err("--json and --plain must conflict");
        assert_eq!(err.kind(), clap::error::ErrorKind::ArgumentConflict);
    }

    #[test]
    fn parallel_not_global_rejected_on_non_sync_verb() {
        // feat-m6 B2 — `--parallel` is sync-scoped only; it must NOT
        // be accepted as a global flag on verbs like `init`/`ls`.
        let err =
            parse(&["init", "--parallel", "1"]).expect_err("--parallel on non-sync verb must fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn sync_parallel_one_accepted() {
        let cli = parse(&["sync", "--parallel", "1"]).expect("sync --parallel 1 parses");
        match cli.verb {
            Verb::Sync(a) => assert_eq!(a.parallel, Some(1)),
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn sync_parallel_max_accepted() {
        let cli = parse(&["sync", "--parallel", "1024"]).expect("sync --parallel 1024 parses");
        match cli.verb {
            Verb::Sync(a) => assert_eq!(a.parallel, Some(1024)),
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn sync_parallel_over_max_rejected() {
        let err =
            parse(&["sync", "--parallel", "1025"]).expect_err("sync --parallel 1025 must fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn import_from_repos_json_parses_as_pathbuf() {
        let cli =
            parse(&["import", "--from-repos-json", "./REPOS.json"]).expect("import parses path");
        match cli.verb {
            Verb::Import(a) => {
                assert_eq!(
                    a.from_repos_json.as_deref(),
                    Some(std::path::Path::new("./REPOS.json"))
                );
            }
            _ => panic!("expected Import variant"),
        }
    }

    #[test]
    fn run_requires_action() {
        let err = parse(&["run"]).expect_err("run w/o action must fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn unknown_verb_fails() {
        let err = parse(&["nope"]).expect_err("unknown verb must fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn unknown_flag_fails() {
        let err = parse(&["init", "--not-a-flag"]).expect_err("unknown flag must fail");
        assert_eq!(err.kind(), clap::error::ErrorKind::UnknownArgument);
    }

    #[test]
    fn cli_non_empty_string_rejects_whitespace() {
        // F8: `--ref " "` / `--only "\t"` must be rejected by the value
        // parser, not silently threaded into the walker / globset layer
        // where they degrade into useless errors.
        for bad in ["", " ", "\t", "  ", "\n"] {
            let err =
                parse(&["sync", ".", "--ref", bad]).expect_err("whitespace --ref must be rejected");
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "for {bad:?}");

            let err = parse(&["sync", ".", "--only", bad])
                .expect_err("whitespace --only must be rejected");
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "for {bad:?}");
        }
    }
}
