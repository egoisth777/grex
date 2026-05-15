pub mod add;
pub mod doctor;
pub mod exec;
pub mod import;
pub mod init;
pub mod ls;
pub mod migrate_lockfile;
pub mod rm;
pub mod run;
pub mod serve;
pub mod status;
pub mod sync;
pub mod teardown;
pub mod update;

/// v1.3.1 B2 — Resolve a verb's `<pack_root>` positional with cwd
/// default when the operator runs the verb from inside a pack root.
///
/// Returns the explicit positional when set; otherwise falls back to
/// `std::env::current_dir()` ONLY when the cwd carries the pack-marker
/// `.grex/pack.yaml`. Returns `None` to preserve the legacy
/// "<pack_root> required" usage error for cwd that lacks the marker.
///
/// Mirrors how `git` tools (e.g. `git status`) default to cwd when
/// `.git/` is present without forcing the operator to repeat the path.
pub(crate) fn resolve_pack_root_or_cwd(
    explicit: Option<&std::path::Path>,
) -> Option<std::path::PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    let cwd = std::env::current_dir().ok()?;
    if cwd.join(".grex").join("pack.yaml").is_file() {
        Some(cwd)
    } else {
        None
    }
}
