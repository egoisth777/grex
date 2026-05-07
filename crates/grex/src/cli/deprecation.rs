//! v1.3.0 deprecation diagnostics for the CLI surface.
//!
//! v1.3.0 introduces `--pack` as the canonical pack-root flag on the four
//! verbs that previously took `--workspace` (`sync`, `serve`,
//! `migrate-lockfile`, `teardown`). The old `--workspace` spelling is
//! preserved as a clap `alias` so existing scripts keep parsing, but
//! every invocation that uses the alias surfaces a one-time deprecation
//! warning so operators can migrate before removal.
//!
//! Detection runs against `std::env::args()` rather than the parsed
//! `clap::ArgMatches` because clap collapses `--pack` and `--workspace`
//! into the same field; once parsing completes there is no signal left
//! that distinguishes which spelling the operator typed.
//!
//! Removal target: `v2.0.0` (per `inst/var/freeze-v1.3.0.md` Table M1).

/// Emit a `tracing::warn!` line when the current process was invoked with
/// the deprecated `--workspace` spelling on a verb that now prefers
/// `--pack`. Each verb handler calls this at the top of its `run`/
/// `execute` entry point so the diagnostic fires regardless of which
/// dispatch path the operator took.
///
/// The check accepts both bare-flag form (`--workspace /path`) and
/// `=`-joined form (`--workspace=/path`) so it does not regress on the
/// less common but valid clap syntax. Unrelated args that happen to
/// embed the substring (e.g. a positional `--workspace-something` token)
/// do NOT trigger the warning because the match is anchored to the full
/// flag spelling.
pub fn warn_workspace_alias_used() {
    if std::env::args().any(|a| a == "--workspace" || a.starts_with("--workspace=")) {
        tracing::warn!(
            target: "grex::cli::deprecation",
            "--workspace is deprecated since v1.3.0; use --pack. Removal scheduled for v2.0.0."
        );
    }
}
