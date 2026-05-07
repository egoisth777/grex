use clap::{Args, Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "grex",
    version,
    about = "grex — nested meta-repo manager. Pack-based, agent-native, Rust-fast.",
    long_about = "grex manages trees of git repositories as a single addressable graph. \
        Each node is a \"pack\" — a plain git repo plus a `.grex/` contract — and every \
        pack is a meta-pack by construction (zero children = leaf, N children = orchestrator \
        of N more packs, recursively). One uniform command surface (`sync`, `add`, `rm`, \
        `update`, `status`, `import`, `doctor`, `teardown`, `exec`, `run`, `serve`) operates \
        over the whole graph regardless of depth."
)]
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
    /// Migrate a v1.1.x lockfile in place to the v1.2.0 schema (opt-in,
    /// idempotent). Thin shim over the v1.2.0 Stage 1.h library
    /// migrator (`grex_core::lockfile::migrate_v1_1_1`).
    #[command(name = "migrate-lockfile")]
    MigrateLockfile(MigrateLockfileArgs),
}

#[derive(Args, Debug)]
pub struct InitArgs {}

#[derive(Args, Debug)]
pub struct AddArgs {
    /// Git URL of the pack repo.
    pub url: String,
    /// Optional local path (defaults to repo name).
    pub path: Option<String>,
    /// v1.3.3 B10 — git ref to pin at add time. Accepts `<branch>`,
    /// `<commit>` (7..40 hex chars), or `<branch>@<commit>` (single
    /// flag, `@` delimiter). When omitted, defaults to the remote's
    /// `main` branch HEAD per the 8-cell folder-FA design.
    #[arg(long = "ref", value_name = "GIT-REF", value_parser = non_empty_string)]
    pub git_ref: Option<String>,
}

#[derive(Args, Debug)]
pub struct RmArgs {
    /// Local path of the pack to remove.
    pub path: String,
}

#[derive(Args, Debug)]
pub struct LsArgs {
    /// Pack root. Directory holding `.grex/pack.yaml`, or the YAML file
    /// itself. Defaults to the current working directory. Pass `.` to
    /// substitute the current working directory explicitly (v1.3.3 B3).
    #[arg(value_parser = pack_path)]
    pub pack_root: Option<std::path::PathBuf>,
}

#[derive(Args, Debug)]
pub struct StatusArgs {}

#[derive(Args, Debug)]
pub struct SyncArgs {
    /// Recurse into child packs.
    #[arg(long, default_value_t = true)]
    pub recursive: bool,

    /// Pack root. Directory holding `.grex/pack.yaml`, or the YAML file
    /// itself. When omitted, `sync` prints the legacy M1 stub and exits 0.
    /// Pass `.` to substitute the current working directory explicitly
    /// (v1.3.3 B3).
    #[arg(value_parser = pack_path)]
    pub pack_root: Option<std::path::PathBuf>,

    /// Path to the pack root (formerly `--workspace`). Defaults to the
    /// pack root directory (where `.grex/pack.yaml` lives). When set,
    /// this path becomes the canonical meta directory: children resolve
    /// parent-relatively as `<pack>/<child.path>`. The path MUST exist;
    /// symlinks are resolved to their canonical inode (logged as
    /// `pack: <input> → <canonical>` when it differs). The legacy
    /// `--workspace` spelling is preserved as a deprecated alias and
    /// emits a one-time warning per process; removal scheduled for
    /// v2.0.0. Pass `.` to substitute the current working directory
    /// explicitly (v1.3.3 B3).
    #[arg(long = "pack", alias = "workspace", value_parser = pack_path)]
    pub pack: Option<std::path::PathBuf>,

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

    /// v1.2.0 Stage 1.l — Override Phase 2 prune-safety refusal for
    /// dirty (tracked or untracked-non-ignored) working trees. Still
    /// refuses ignored content unless `--force-prune-with-ignored` is
    /// also set. Never overrides `GitInProgress` (mid-rebase / merge /
    /// cherry-pick / revert / bisect).
    #[arg(long = "force-prune")]
    pub force_prune: bool,

    /// v1.2.0 Stage 1.l — Strongest prune override. Implies
    /// `--force-prune` and additionally drops trees whose only dirt is
    /// in `--ignored` paths (build artefacts, `target/`, `node_modules/`).
    /// Never overrides `GitInProgress`.
    #[arg(long = "force-prune-with-ignored")]
    pub force_prune_with_ignored: bool,

    /// v1.2.1 Item 5b — Recursively snapshot Phase 2 prune targets to
    /// `<meta>/.grex/trash/<ISO8601>/<basename>/` BEFORE deletion.
    /// Audit log entry (`QuarantineStart`) is appended + fsync'd
    /// before any byte is copied; on snapshot failure the prune
    /// aborts and the original dest is left intact for forensics.
    /// Requires `--force-prune` or `--force-prune-with-ignored` —
    /// quarantine only applies to overridden prunes; clean-consent
    /// prunes still go through the direct-unlink fast path. The
    /// "requires one of" check is enforced in the verb handler
    /// (see `crates/grex/src/cli/verbs/sync.rs`) since clap's
    /// `requires`/`required_unless_present_any` semantics don't
    /// model "X requires (A or B)" cleanly without an `ArgGroup`.
    /// Matches Lean theorem `quarantine_snapshot_precedes_delete`.
    #[arg(long = "quarantine")]
    pub quarantine: bool,

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
    /// `inst/concurrency.md`.
    #[arg(
        long = "parallel",
        env = "GREX_PARALLEL",
        value_parser = clap::value_parser!(u32).range(0..=1024),
    )]
    pub parallel: Option<u32>,

    /// v1.2.5 — sweep `<meta>/.grex/trash/` of entries older than
    /// `N` days at the start of every meta sync (best-effort). When
    /// omitted, no GC fires (v1.2.1 indefinite-retention behavior is
    /// preserved). When set, sweep failures log via tracing and do
    /// NOT halt the sync.
    #[arg(long = "retain-days", value_name = "N")]
    pub retain_days: Option<u32>,
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

/// v1.3.3 B3 — Clap `value_parser` for `--pack` / `--workspace` / pack-root
/// positional path args. Substitutes the literal `.` token with
/// `std::env::current_dir()` at parse time so all downstream code sees a
/// concrete cwd path. Per design.md § B3 (Q2): cwd-only resolution, **no
/// walk-up**. Other relative path forms (`..`, `./subdir`, etc.) flow
/// through unchanged.
fn pack_path(s: &str) -> Result<std::path::PathBuf, String> {
    if s == "." {
        std::env::current_dir().map_err(|e| format!("`.` shorthand: cannot resolve cwd: {e}"))
    } else {
        Ok(std::path::PathBuf::from(s))
    }
}

#[derive(Args, Debug)]
pub struct UpdateArgs {
    /// Optional pack path; if omitted, update all.
    pub pack: Option<String>,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    /// Heal gitignore drift by re-emitting the managed block. Safety:
    /// NEVER touches the manifest or the filesystem on other checks.
    #[arg(long)]
    pub fix: bool,

    /// Run the opt-in config-lint check (`openspec/config.yaml` +
    /// `inst/cfg/*.md`). Skipped by default.
    #[arg(long = "lint-config")]
    pub lint_config: bool,

    /// v1.2.0 Stage 1.j — bound the recursive ManifestTree walk.
    /// Omitted: walk every nested meta exhaustively (default).
    /// `--shallow 0`: root meta only.
    /// `--shallow N`: recurse up to `N` levels of nesting (root is
    /// depth 0; depth-`N` metas are visited but their children are
    /// not). The walk is read-only at every frame.
    #[arg(long = "shallow", value_name = "N")]
    pub shallow: Option<usize>,

    /// v1.2.1 item 4 — opt-in full-filesystem scan for `.git/`
    /// directories that are not registered in the manifest tree.
    /// Read-only audit; complements the manifest-driven default walk.
    /// Composes with `--shallow` (which bounds the manifest walk).
    /// Use `--depth N` to bound the filesystem scan independently.
    #[arg(long = "scan-undeclared")]
    pub scan_undeclared: bool,

    /// v1.2.1 item 4 — bound the `--scan-undeclared` filesystem walk.
    /// Omitted: scan every level under the workspace (default).
    /// `--depth 0`: workspace root only.
    /// `--depth N`: descend up to `N` directory levels below the
    /// workspace root. Has no effect unless `--scan-undeclared` is
    /// also set.
    #[arg(long = "depth", value_name = "N", requires = "scan_undeclared")]
    pub depth: Option<usize>,

    /// v1.2.5 — sweep `<workspace>/.grex/trash/` of entries older than
    /// the supplied retention window (in days). Pairs with the
    /// canonical retention default surfaced by
    /// [`grex_core::tree::DEFAULT_RETAIN_DAYS`] when the operator
    /// omits a value. Best-effort: per-entry failures log via
    /// `tracing::warn!` and do not halt the doctor run.
    #[arg(long = "prune-quarantine")]
    pub prune_quarantine: bool,

    /// v1.2.5 — explicit retention window for `--prune-quarantine`
    /// (and `grex sync`'s GC sweep). Defaults to
    /// [`grex_core::tree::DEFAULT_RETAIN_DAYS`] when `--prune-quarantine`
    /// is set without an explicit value. Has no effect unless
    /// `--prune-quarantine` is also passed (or threaded into
    /// `grex sync` via the matching flag there).
    #[arg(long = "retain-days", value_name = "N")]
    pub retain_days: Option<u32>,

    /// v1.2.5 — restore the snapshot at
    /// `<workspace>/.grex/trash/<TS>/<BASENAME>/` back into the
    /// workspace. When BASENAME is omitted the `<TS>/` slot must hold
    /// exactly one child entry (otherwise restore is refused as
    /// ambiguous). Refuses to clobber an existing dest unless
    /// `--force` is also passed.
    #[arg(long = "restore-quarantine", value_name = "TS[:BASENAME]", num_args = 1)]
    pub restore_quarantine: Option<String>,

    /// v1.2.5 — paired with `--restore-quarantine`: when set, remove
    /// the existing dest before the rename. Without this flag,
    /// restore refuses to clobber an existing dest.
    #[arg(long = "force", requires = "restore_quarantine")]
    pub force: bool,
}

#[derive(Args, Debug)]
pub struct ServeArgs {
    /// Path to the `.grex/events.jsonl` event log. Captured at server
    /// launch and immutable for the session (per spec §"Manifest binding").
    /// Defaults to `<workspace>/.grex/events.jsonl` when omitted, where
    /// `<workspace>` is resolved by walking up from cwd to the nearest
    /// `.grex/` marker. v1.x `<workspace>/grex.jsonl` event logs are
    /// auto-migrated to the canonical location on first access.
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<std::path::PathBuf>,

    /// Path to the pack root (formerly `--workspace`) the MCP server
    /// resolves relative paths against. Defaults to the current working
    /// directory when omitted. The legacy `--workspace` spelling is
    /// preserved as a deprecated alias and emits a one-time warning per
    /// process; removal scheduled for v2.0.0. Pass `.` to substitute
    /// the current working directory explicitly (v1.3.3 B3).
    #[arg(long = "pack", alias = "workspace", value_name = "PATH", value_parser = pack_path)]
    pub pack: Option<std::path::PathBuf>,

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

    /// Target event log (`.grex/events.jsonl`). Defaults to
    /// `<workspace>/.grex/events.jsonl` where `<workspace>` is resolved
    /// by walking up from cwd to the nearest `.grex/` marker.
    #[arg(long, value_name = "PATH")]
    pub manifest: Option<std::path::PathBuf>,

    /// Verb-scoped dry-run. Alias for the global `--dry-run`; either
    /// flag short-circuits before any manifest write.
    #[arg(long = "dry-run", short = 'n')]
    pub dry_run: bool,
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
pub struct MigrateLockfileArgs {
    /// Path to the pack root (formerly `--workspace`) — the meta whose
    /// `.grex/grex.lock.jsonl` is migrated. Defaults to the current
    /// working directory. The legacy `--workspace` spelling is preserved
    /// as a deprecated alias and emits a one-time warning per process;
    /// removal scheduled for v2.0.0. Pass `.` to substitute the current
    /// working directory explicitly (v1.3.3 B3).
    #[arg(long = "pack", alias = "workspace", value_name = "PATH", value_parser = pack_path)]
    pub pack: Option<std::path::PathBuf>,

    /// Inspect-only: detect schema version and report what would happen
    /// without writing. Lockfile bytes are unchanged.
    #[arg(long = "dry-run", short = 'n')]
    pub dry_run: bool,
}

#[derive(Args, Debug)]
pub struct TeardownArgs {
    /// Pack root. Directory holding `.grex/pack.yaml`, or the YAML file
    /// itself. When omitted, `teardown` prints a usage stub and exits 0.
    /// Pass `.` to substitute the current working directory explicitly
    /// (v1.3.3 B3).
    #[arg(value_parser = pack_path)]
    pub pack_root: Option<std::path::PathBuf>,

    /// Path to the pack root (formerly `--workspace`). Defaults to the
    /// pack root directory (where `.grex/pack.yaml` lives). When set,
    /// this path becomes the canonical meta directory: children resolve
    /// parent-relatively as `<pack>/<child.path>`. The path MUST exist;
    /// symlinks are resolved to their canonical inode (logged as
    /// `pack: <input> → <canonical>` when it differs). The legacy
    /// `--workspace` spelling is preserved as a deprecated alias and
    /// emits a one-time warning per process; removal scheduled for
    /// v2.0.0. Pass `.` to substitute the current working directory
    /// explicitly (v1.3.3 B3).
    #[arg(long = "pack", alias = "workspace", value_parser = pack_path)]
    pub pack: Option<std::path::PathBuf>,

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
    fn test_cli_force_prune_flag_parsed() {
        // v1.2.0 Stage 1.l — `--force-prune` toggles the SyncArgs
        // bool. Default is `false`.
        let cli = parse(&["sync", "."]).expect("sync . parses");
        match cli.verb {
            Verb::Sync(ref a) => {
                assert!(!a.force_prune, "default --force-prune must be false");
                assert!(
                    !a.force_prune_with_ignored,
                    "default --force-prune-with-ignored must be false"
                );
            }
            _ => panic!("expected Sync variant"),
        }
        let cli = parse(&["sync", ".", "--force-prune"]).expect("sync --force-prune parses");
        match cli.verb {
            Verb::Sync(a) => {
                assert!(a.force_prune, "--force-prune must set true");
                assert!(
                    !a.force_prune_with_ignored,
                    "--force-prune-with-ignored stays default false"
                );
            }
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn test_cli_force_prune_with_ignored_flag_parsed() {
        // v1.2.0 Stage 1.l — `--force-prune-with-ignored` toggles
        // independently of `--force-prune`. Walker layer interprets the
        // matrix; CLI just parses the bools.
        let cli = parse(&["sync", ".", "--force-prune-with-ignored"])
            .expect("sync --force-prune-with-ignored parses");
        match cli.verb {
            Verb::Sync(a) => {
                assert!(
                    !a.force_prune,
                    "--force-prune is independent of --force-prune-with-ignored at parse layer"
                );
                assert!(a.force_prune_with_ignored, "--force-prune-with-ignored must set true");
            }
            _ => panic!("expected Sync variant"),
        }
        // Both flags together: caller's documented "stronger" combo.
        let cli = parse(&["sync", ".", "--force-prune", "--force-prune-with-ignored"])
            .expect("sync --force-prune --force-prune-with-ignored parses");
        match cli.verb {
            Verb::Sync(a) => {
                assert!(a.force_prune);
                assert!(a.force_prune_with_ignored);
            }
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn b3_dot_resolves_to_cwd_for_pack_flag_on_sync() {
        // v1.3.3 B3 — `--pack .` substitutes process cwd at parse time.
        let cwd = std::env::current_dir().expect("cwd available");
        let cli = parse(&["sync", "--pack", "."]).expect("sync --pack . parses");
        match cli.verb {
            Verb::Sync(a) => assert_eq!(a.pack.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn b3_dot_resolves_to_cwd_for_workspace_alias() {
        // v1.3.3 B3 — legacy `--workspace .` spelling honors `.` shorthand
        // identically to `--pack .` (the alias is the same flag).
        let cwd = std::env::current_dir().expect("cwd available");
        let cli = parse(&["sync", "--workspace", "."]).expect("sync --workspace . parses");
        match cli.verb {
            Verb::Sync(a) => assert_eq!(a.pack.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn b3_dot_resolves_for_positional_pack_root() {
        // v1.3.3 B3 — positional `<pack_root>` arg also honors `.` for
        // sync / teardown / ls verbs.
        let cwd = std::env::current_dir().expect("cwd available");
        let cli = parse(&["sync", "."]).expect("sync . parses");
        match cli.verb {
            Verb::Sync(a) => assert_eq!(a.pack_root.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected Sync variant"),
        }
        let cli = parse(&["teardown", "."]).expect("teardown . parses");
        match cli.verb {
            Verb::Teardown(a) => assert_eq!(a.pack_root.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected Teardown variant"),
        }
        let cli = parse(&["ls", "."]).expect("ls . parses");
        match cli.verb {
            Verb::Ls(a) => assert_eq!(a.pack_root.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected Ls variant"),
        }
    }

    #[test]
    fn b3_dot_resolves_on_serve_and_migrate_lockfile() {
        // v1.3.3 B3 — `--pack .` is uniform across every verb that
        // accepts the flag. Cover the verbs not exercised above.
        let cwd = std::env::current_dir().expect("cwd available");
        let cli = parse(&["serve", "--pack", "."]).expect("serve --pack . parses");
        match cli.verb {
            Verb::Serve(a) => assert_eq!(a.pack.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected Serve variant"),
        }
        let cli = parse(&["migrate-lockfile", "--pack", "."])
            .expect("migrate-lockfile --pack . parses");
        match cli.verb {
            Verb::MigrateLockfile(a) => assert_eq!(a.pack.as_deref(), Some(cwd.as_path())),
            _ => panic!("expected MigrateLockfile variant"),
        }
    }

    #[test]
    fn b3_other_relative_paths_unchanged() {
        // v1.3.3 B3 — only the literal `.` token is rewritten. Other
        // relative path forms (`./subdir`, `..`, plain names) flow
        // through unchanged so existing path handling stays intact.
        let cli = parse(&["sync", "--pack", "./subdir"]).expect("sync --pack ./subdir parses");
        match cli.verb {
            Verb::Sync(a) => {
                assert_eq!(a.pack.as_deref(), Some(std::path::Path::new("./subdir")));
            }
            _ => panic!("expected Sync variant"),
        }
        let cli = parse(&["sync", "--pack", ".."]).expect("sync --pack .. parses");
        match cli.verb {
            Verb::Sync(a) => {
                assert_eq!(a.pack.as_deref(), Some(std::path::Path::new("..")));
            }
            _ => panic!("expected Sync variant"),
        }
        let cli = parse(&["sync", "--pack", "some/pack"]).expect("sync --pack some/pack parses");
        match cli.verb {
            Verb::Sync(a) => {
                assert_eq!(a.pack.as_deref(), Some(std::path::Path::new("some/pack")));
            }
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn b3_absolute_path_unchanged() {
        // v1.3.3 B3 — absolute paths are passed through verbatim.
        #[cfg(windows)]
        let abs = "C:\\abs\\path";
        #[cfg(not(windows))]
        let abs = "/abs/path";
        let cli = parse(&["sync", "--pack", abs]).expect("sync --pack <abs> parses");
        match cli.verb {
            Verb::Sync(a) => assert_eq!(a.pack.as_deref(), Some(std::path::Path::new(abs))),
            _ => panic!("expected Sync variant"),
        }
    }

    #[test]
    fn b10_add_ref_flag_parses_bare_branch() {
        // v1.3.3 B10 — `--ref main` populates `git_ref`.
        let cli = parse(&["add", "https://example.com/repo.git", "--ref", "main"])
            .expect("add --ref main parses");
        match cli.verb {
            Verb::Add(a) => assert_eq!(a.git_ref.as_deref(), Some("main")),
            _ => panic!("expected Add variant"),
        }
    }

    #[test]
    fn b10_add_ref_flag_parses_branch_at_commit() {
        // v1.3.3 B10 — `@`-delimited branch+commit pin parses verbatim;
        // `parse_ref` (in grex-core) splits the components.
        let cli = parse(&["add", "https://example.com/repo.git", "--ref", "main@a3f9c1d"])
            .expect("add --ref main@a3f9c1d parses");
        match cli.verb {
            Verb::Add(a) => assert_eq!(a.git_ref.as_deref(), Some("main@a3f9c1d")),
            _ => panic!("expected Add variant"),
        }
    }

    #[test]
    fn b10_add_ref_flag_parses_bare_commit() {
        // v1.3.3 B10 — bare 7-char SHA token routes to `git_ref`.
        let cli = parse(&["add", "https://example.com/repo.git", "--ref", "a3f9c1d"])
            .expect("add --ref a3f9c1d parses");
        match cli.verb {
            Verb::Add(a) => assert_eq!(a.git_ref.as_deref(), Some("a3f9c1d")),
            _ => panic!("expected Add variant"),
        }
    }

    #[test]
    fn b10_add_ref_flag_optional() {
        // v1.3.3 B10 — `--ref` is opt-in; omitting it leaves `git_ref` None.
        let cli = parse(&["add", "https://example.com/repo.git"])
            .expect("add without --ref parses");
        match cli.verb {
            Verb::Add(a) => assert!(a.git_ref.is_none(), "default --ref must be None"),
            _ => panic!("expected Add variant"),
        }
    }

    #[test]
    fn b10_add_ref_rejects_whitespace() {
        // v1.3.3 B10 — `--ref ""` / `--ref " "` rejected by value parser.
        for bad in ["", " ", "\t"] {
            let err = parse(&["add", "https://example.com/repo.git", "--ref", bad])
                .expect_err("whitespace --ref must be rejected");
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation, "for {bad:?}");
        }
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
