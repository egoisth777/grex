//! Predicate evaluator used by [`super::plan`].
//!
//! Scope (M4-C + post-review fix bundle):
//! * `path_exists`, `cmd_available`, `os`, `symlink_ok` — real checks.
//! * `reg_key` — Windows-only `winreg` probe (`RegOpenKeyEx` +
//!   `RegQueryValueEx`); non-Windows returns
//!   [`ExecError::PredicateNotSupported`]. Forward-slash separators are
//!   normalized to `\`. ACL-denied / transient OS errors surface as
//!   [`ExecError::PredicateProbeFailed`] rather than collapsing to
//!   `Ok(false)`.
//! * `psversion` — Windows-only shell-out to
//!   `powershell.exe -NoProfile -Command
//!   "$($PSVersionTable.PSVersion.Major).$($PSVersionTable.PSVersion.Minor)"`,
//!   parsed and compared as a `(major, minor)` tuple against a
//!   `>=<major>[.<minor>]` / `<major>[.<minor>]` spec; non-Windows
//!   returns [`ExecError::PredicateNotSupported`]. The probe is bounded
//!   by a 5s timeout, prefers the `%SystemRoot%` absolute path to
//!   resist PATH hijack, distinguishes "powershell.exe missing" (→
//!   `Ok(false)`, graceful degrade) from genuine spawn / exit failures
//!   (→ [`ExecError::PredicateProbeFailed`]), strips a leading UTF-8
//!   BOM / banner noise, and surfaces non-zero exits with a stderr
//!   excerpt.
//! * `all_of` / `any_of` / `none_of` at the [`Predicate`] level —
//!   short-circuit recursion. Within these combiners (and within
//!   [`WhenSpec`]'s own `all_of` / `any_of` / `none_of` lists) a leg
//!   that returns [`ExecError::PredicateNotSupported`] is treated as
//!   `false` so the other legs still get a chance — this preserves the
//!   pre-M4-C cross-platform rescue pattern
//!   (`any_of: [{reg_key: ...}, {path_exists: /etc/foo}]`). The
//!   *top-level* [`Combiner`] attached to a `RequireSpec` stays strict:
//!   a single unsupported leaf under a top-level `require` still
//!   bubbles the typed error, matching spec §M4 req 5.
//!
//! Expansion failures short-circuit to `Ok(false)` at the leaf level: a
//! predicate that references an undefined variable can never be satisfied,
//! and pushing the expansion error up through the tree would entangle
//! evaluation with parse diagnostics. Callers wanting fail-loud behaviour
//! should run [`super::plan`] over the owning action directly — it surfaces
//! the underlying [`crate::execute::ExecError::VarExpand`] from the action
//! field, which is strictly more informative.

use std::path::Path;

use crate::pack::{OsKind, Predicate, WhenSpec};
use crate::vars::{expand, VarEnv};

use super::ctx::ExecCtx;
use super::error::ExecError;

/// Evaluate the composite `when` gate.
///
/// `os` and each combiner compose with AND semantics per `actions.md`.
/// Shared between [`super::plan::PlanExecutor`] and
/// [`super::fs_executor::FsExecutor`] so dry-run and wet-run agree on which
/// branches are taken.
///
/// Predicate legs inside `all_of` / `any_of` / `none_of` are evaluated
/// tolerantly: a leg that returns [`ExecError::PredicateNotSupported`] is
/// treated as `false` so other legs can rescue the expression. See module
/// docs for the rationale and the top-level strictness boundary.
pub(super) fn evaluate_when_gate(spec: &WhenSpec, ctx: &ExecCtx<'_>) -> Result<bool, ExecError> {
    if let Some(os) = spec.os {
        // The `os:` shorthand is strict — it is never `PredicateNotSupported`
        // (every platform answers the OS predicate).
        if !evaluate(&Predicate::Os(os), ctx)? {
            return Ok(false);
        }
    }
    if let Some(list) = &spec.all_of {
        for p in list {
            if !evaluate_tolerant(p, ctx)? {
                return Ok(false);
            }
        }
    }
    if let Some(list) = &spec.any_of {
        let mut any = false;
        for p in list {
            if evaluate_tolerant(p, ctx)? {
                any = true;
                break;
            }
        }
        if !any {
            return Ok(false);
        }
    }
    if let Some(list) = &spec.none_of {
        for p in list {
            if evaluate_tolerant(p, ctx)? {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Evaluate a predicate tree against `ctx`.
pub(super) fn evaluate(predicate: &Predicate, ctx: &ExecCtx<'_>) -> Result<bool, ExecError> {
    match predicate {
        Predicate::PathExists(raw) => Ok(eval_path_exists(raw, ctx.vars)),
        Predicate::CmdAvailable(name) => Ok(eval_cmd_available(name, ctx.vars)),
        Predicate::RegKey { path, name } => eval_reg_key(path, name.as_deref()),
        Predicate::Os(os) => Ok(eval_os(*os, ctx)),
        Predicate::PsVersion(spec) => eval_ps_version(spec),
        Predicate::SymlinkOk { src, dst } => Ok(eval_symlink_ok(src, dst, ctx.vars)),
        Predicate::AllOf(children) => eval_all_of(children, ctx),
        Predicate::AnyOf(children) => eval_any_of(children, ctx),
        Predicate::NoneOf(children) => eval_none_of(children, ctx),
    }
}

/// Evaluate `predicate`, converting [`ExecError::PredicateNotSupported`]
/// into `Ok(false)`. Used by combiner legs where an unsupported predicate
/// should not veto the other legs. [`ExecError::PredicateProbeFailed`]
/// and every other error shape still bubbles — a broken probe is not a
/// rescue-eligible condition.
pub(super) fn evaluate_tolerant(
    predicate: &Predicate,
    ctx: &ExecCtx<'_>,
) -> Result<bool, ExecError> {
    match evaluate(predicate, ctx) {
        Err(ExecError::PredicateNotSupported { .. }) => Ok(false),
        other => other,
    }
}

fn eval_all_of(children: &[Predicate], ctx: &ExecCtx<'_>) -> Result<bool, ExecError> {
    for p in children {
        if !evaluate_tolerant(p, ctx)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn eval_any_of(children: &[Predicate], ctx: &ExecCtx<'_>) -> Result<bool, ExecError> {
    for p in children {
        if evaluate_tolerant(p, ctx)? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn eval_none_of(children: &[Predicate], ctx: &ExecCtx<'_>) -> Result<bool, ExecError> {
    for p in children {
        if evaluate_tolerant(p, ctx)? {
            return Ok(false);
        }
    }
    Ok(true)
}

fn eval_path_exists(raw: &str, env: &VarEnv) -> bool {
    let Ok(expanded) = expand(raw, env) else { return false };
    Path::new(&expanded).exists()
}

fn eval_cmd_available(raw: &str, env: &VarEnv) -> bool {
    let Ok(expanded) = expand(raw, env) else { return false };
    if expanded.is_empty() {
        return false;
    }
    // PATHEXT handles `.exe`/`.bat` on Windows; on Unix we probe the bare
    // name. `which`-style scan: walk PATH, return first hit that resolves
    // to a regular file.
    let path = match env.get("PATH") {
        Some(v) => v.to_string(),
        None => std::env::var("PATH").unwrap_or_default(),
    };
    if path.is_empty() {
        return false;
    }
    #[cfg(windows)]
    let sep = ';';
    #[cfg(not(windows))]
    let sep = ':';

    #[cfg(windows)]
    let extensions: Vec<String> = env
        .get("PATHEXT")
        .map(str::to_string)
        .or_else(|| std::env::var("PATHEXT").ok())
        .unwrap_or_else(|| ".COM;.EXE;.BAT;.CMD".to_string())
        .split(';')
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect();
    #[cfg(not(windows))]
    let extensions: Vec<String> = vec![String::new()];

    for dir in path.split(sep) {
        if dir.is_empty() {
            continue;
        }
        for ext in &extensions {
            let candidate = Path::new(dir).join(format!("{expanded}{ext}"));
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------- reg_key

/// Windows hive tag used by [`split_hive`]. Keeps the parser layer free of
/// `winreg::HKEY` so non-Windows builds compile the same enum. The tag is
/// mapped to the concrete `HKEY_*` constant inside [`eval_reg_key`] on
/// Windows only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HiveTag {
    Hklm,
    Hkcu,
    Hkcr,
    Hku,
}

/// Split `HKCU\Software\X` (or `HKCU/Software/X`) into (`HiveTag::Hkcu`,
/// `"Software\\X"`). Accepts the four hive prefixes in use by real-world
/// packs and both `\` and `/` separators (normalized to `\` before the
/// split so downstream `winreg` calls see the canonical form). Returns
/// `None` if the prefix is unrecognised or the subpath is empty.
fn split_hive(path: &str) -> Option<(HiveTag, String)> {
    let normalized = path.replace('/', "\\");
    let (prefix, rest) = normalized.split_once('\\')?;
    let hive = match prefix.to_ascii_uppercase().as_str() {
        "HKCU" | "HKEY_CURRENT_USER" => HiveTag::Hkcu,
        "HKLM" | "HKEY_LOCAL_MACHINE" => HiveTag::Hklm,
        "HKCR" | "HKEY_CLASSES_ROOT" => HiveTag::Hkcr,
        "HKU" | "HKEY_USERS" => HiveTag::Hku,
        _ => return None,
    };
    if rest.is_empty() {
        None
    } else {
        Some((hive, rest.to_string()))
    }
}

#[cfg(windows)]
fn eval_reg_key(path: &str, value: Option<&str>) -> Result<bool, ExecError> {
    use winreg::enums::{HKEY_CLASSES_ROOT, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, HKEY_USERS};
    use winreg::RegKey;
    let Some((hive, subpath)) = split_hive(path) else {
        // Unknown/missing hive — treat as absent (conservative, no key
        // exists under an unsupported hive prefix). Same shape as other
        // leaf predicates when input is unparseable.
        return Ok(false);
    };
    let hkey = match hive {
        HiveTag::Hkcu => HKEY_CURRENT_USER,
        HiveTag::Hklm => HKEY_LOCAL_MACHINE,
        HiveTag::Hkcr => HKEY_CLASSES_ROOT,
        HiveTag::Hku => HKEY_USERS,
    };
    let root = RegKey::predef(hkey);
    match root.open_subkey(&subpath) {
        Ok(key) => match value {
            None => Ok(true),
            Some(name) => Ok(key.get_raw_value(name).is_ok()),
        },
        Err(err) => classify_reg_open_err(err, path),
    }
}

#[cfg(windows)]
fn classify_reg_open_err(err: std::io::Error, path: &str) -> Result<bool, ExecError> {
    // ERROR_FILE_NOT_FOUND (2) and ERROR_PATH_NOT_FOUND (3) both mean the
    // key simply is not present — the conservative `false` shape matches
    // the rest of the leaf predicates. Everything else (ACL denial,
    // transient registry I/O) must surface loud, otherwise an ACL-denied
    // probe silently lies about presence.
    match err.raw_os_error() {
        Some(2) | Some(3) => Ok(false),
        _ => Err(ExecError::PredicateProbeFailed {
            predicate: "reg_key",
            detail: format!("{err}: {path}"),
        }),
    }
}

#[cfg(not(windows))]
fn eval_reg_key(_path: &str, _value: Option<&str>) -> Result<bool, ExecError> {
    Err(ExecError::PredicateNotSupported { predicate: "reg_key", platform: std::env::consts::OS })
}

// ---------------------------------------------------------------- psversion

#[cfg(windows)]
fn eval_ps_version(spec: &str) -> Result<bool, ExecError> {
    let Some(target) = parse_ps_version_spec(spec) else {
        // Unparseable spec — same conservative-false shape we inherited
        // from the M3 stub so a typo in the pack does not halt sync.
        return Ok(false);
    };
    let Some(installed) = probe_ps_version()? else {
        // powershell.exe genuinely absent from known locations — treat
        // the predicate as unsatisfied so cross-platform any_of rescue
        // still works. Matches the `reg_key` NotFound shape.
        return Ok(false);
    };
    Ok(installed >= target)
}

#[cfg(not(windows))]
fn eval_ps_version(_spec: &str) -> Result<bool, ExecError> {
    Err(ExecError::PredicateNotSupported { predicate: "psversion", platform: std::env::consts::OS })
}

/// Parse the minimum `(major, minor)` tuple from specs like `">=5.1"`,
/// `">=7"`, `"5.1"`, `"7"`. A missing minor component is treated as `0`.
/// Returns `None` on any unrecognised shape; callers treat that as
/// unsatisfied rather than failing loudly (the spec vocabulary is informal
/// and a hard parse error would be a regression vs. the M3 stub).
fn parse_ps_version_spec(spec: &str) -> Option<(u32, u32)> {
    let trimmed = spec.trim();
    let rest = trimmed.strip_prefix(">=").unwrap_or(trimmed).trim();
    let mut parts = rest.split('.');
    let major = parts.next()?.parse::<u32>().ok()?;
    let minor = match parts.next() {
        Some(m) => m.parse::<u32>().ok()?,
        None => 0,
    };
    Some((major, minor))
}

#[cfg(windows)]
fn probe_ps_version() -> Result<Option<(u32, u32)>, ExecError> {
    let Some(stdout) = spawn_powershell_version()? else {
        return Ok(None);
    };
    Ok(parse_ps_stdout(&stdout))
}

/// Spawn `powershell.exe`, wait up to 5 s, return its stdout on success.
/// Returns `Ok(None)` when the binary is not present at either the
/// absolute `%SystemRoot%` path or bare `powershell.exe` (graceful
/// degrade, matches `reg_key` NotFound shape). Timeout, non-zero exit,
/// or unexpected I/O all surface as [`ExecError::PredicateProbeFailed`].
#[cfg(windows)]
fn spawn_powershell_version() -> Result<Option<String>, ExecError> {
    use std::io::ErrorKind;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    const ARGS: &[&str] = &[
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "\"$($PSVersionTable.PSVersion.Major).$($PSVersionTable.PSVersion.Minor)\"",
    ];
    const TIMEOUT: Duration = Duration::from_secs(5);

    // Resolve an absolute path first to resist PATH-hijack. `SystemRoot`
    // is set on every supported Windows install; if a stripped image
    // loses it we fall back to the bare name so the probe still works.
    let explicit = std::env::var("SystemRoot")
        .ok()
        .map(|root| format!("{root}\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"));

    for program in explicit.iter().map(String::as_str).chain(std::iter::once("powershell.exe")) {
        match Command::new(program)
            .args(ARGS)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(child) => return Ok(Some(wait_with_timeout(child, TIMEOUT)?)),
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(ExecError::PredicateProbeFailed {
                    predicate: "psversion",
                    detail: format!("spawn `{program}`: {err}"),
                });
            }
        }
    }
    // Both the absolute and bare paths returned NotFound — graceful
    // degrade to `Ok(false)` (matches the `reg_key` NotFound shape).
    Ok(None)
}

/// Poll `child` up to `timeout`, returning captured stdout on a clean
/// zero exit. Non-zero exit → [`ExecError::PredicateProbeFailed`] with a
/// truncated stderr excerpt; timeout → kill + `PredicateProbeFailed`.
#[cfg(windows)]
fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: std::time::Duration,
) -> Result<String, ExecError> {
    use std::io::Read;
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + timeout;
    let poll = Duration::from_millis(50);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                if let Some(mut s) = child.stdout.take() {
                    let _ = s.read_to_string(&mut out);
                }
                if !status.success() {
                    let mut err = String::new();
                    if let Some(mut e) = child.stderr.take() {
                        let _ = e.read_to_string(&mut err);
                    }
                    return Err(ExecError::PredicateProbeFailed {
                        predicate: "psversion",
                        detail: format!(
                            "exit {}: {}",
                            status.code().unwrap_or(-1),
                            truncate_stderr(&err),
                        ),
                    });
                }
                return Ok(out);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ExecError::PredicateProbeFailed {
                        predicate: "psversion",
                        detail: format!("timeout after {}s", timeout.as_secs()),
                    });
                }
                sleep(poll);
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ExecError::PredicateProbeFailed {
                    predicate: "psversion",
                    detail: format!("wait: {err}"),
                });
            }
        }
    }
}

/// Match the 2 KiB stderr cap used by `ExecError::ExecNonZero` so probe
/// failures stay log-line-bounded.
#[cfg(windows)]
fn truncate_stderr(s: &str) -> String {
    const MAX: usize = 2048;
    if s.len() <= MAX {
        return s.trim().to_string();
    }
    let mut cut = MAX;
    while !s.is_char_boundary(cut) && cut > 0 {
        cut -= 1;
    }
    format!("{}... (truncated)", s[..cut].trim_end())
}

/// Parse `"7.4\r\n"` / `"\u{feff}7.4"` / banner-prefixed output into a
/// `(major, minor)` tuple. Scans line-by-line, strips a leading UTF-8
/// BOM if present, returns the first line that parses as `N` or `N.M`.
fn parse_ps_stdout(stdout: &str) -> Option<(u32, u32)> {
    let stripped = stdout.strip_prefix('\u{feff}').unwrap_or(stdout);
    stripped.lines().filter_map(parse_ps_version_spec).next()
}

fn eval_os(os: OsKind, ctx: &ExecCtx<'_>) -> bool {
    let token = match os {
        OsKind::Windows => "windows",
        OsKind::Linux => "linux",
        OsKind::Macos => "macos",
    };
    ctx.platform.matches_os_token(token)
}

fn eval_symlink_ok(src: &str, dst: &str, env: &VarEnv) -> bool {
    let Ok(src_exp) = expand(src, env) else { return false };
    let Ok(dst_exp) = expand(dst, env) else { return false };
    let dst_path = Path::new(&dst_exp);
    let Ok(meta) = std::fs::symlink_metadata(dst_path) else { return false };
    if !meta.file_type().is_symlink() {
        return false;
    }
    match std::fs::read_link(dst_path) {
        Ok(target) => target == Path::new(&src_exp),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ps_version_spec_accepts_common_shapes() {
        assert_eq!(parse_ps_version_spec(">=5.1"), Some((5, 1)));
        assert_eq!(parse_ps_version_spec(">=7"), Some((7, 0)));
        assert_eq!(parse_ps_version_spec("5.1"), Some((5, 1)));
        assert_eq!(parse_ps_version_spec("7"), Some((7, 0)));
        assert_eq!(parse_ps_version_spec("  >= 5.1  "), Some((5, 1)));
    }

    #[test]
    fn parse_ps_version_spec_captures_minor() {
        // F1: regression — pre-fix-bundle dropped the minor component,
        // so `>=7.9` passed on 7.0. Confirm the tuple is captured.
        assert_eq!(parse_ps_version_spec(">=7.9"), Some((7, 9)));
        assert!(parse_ps_version_spec(">=7.9") > parse_ps_version_spec("7.0"));
    }

    #[test]
    fn parse_ps_version_spec_rejects_garbage() {
        assert_eq!(parse_ps_version_spec(""), None);
        assert_eq!(parse_ps_version_spec("abc"), None);
        assert_eq!(parse_ps_version_spec(">=abc"), None);
        assert_eq!(parse_ps_version_spec("7.abc"), None);
    }

    #[test]
    fn parse_ps_stdout_strips_bom() {
        // F8: UTF-8 BOM (EF BB BF) sometimes leads PowerShell stdout.
        let with_bom = "\u{feff}7.4\r\n";
        assert_eq!(parse_ps_stdout(with_bom), Some((7, 4)));
    }

    #[test]
    fn parse_ps_stdout_skips_banner_lines() {
        // F8: execution-policy banner or profile noise preceding the
        // numeric line must not defeat parse.
        let noisy = "Warning: something\r\n\r\n5.1\r\n";
        assert_eq!(parse_ps_stdout(noisy), Some((5, 1)));
    }

    #[test]
    fn parse_ps_stdout_empty_returns_none() {
        assert_eq!(parse_ps_stdout(""), None);
        assert_eq!(parse_ps_stdout("\r\n  \r\n"), None);
    }

    #[test]
    fn split_hive_accepts_forward_slash() {
        // F6: pack authors often write `HKCU/Software/X` in YAML.
        let (hive, rest) = split_hive("HKCU/Software/Microsoft/Windows").expect("parse");
        assert_eq!(hive, HiveTag::Hkcu);
        assert_eq!(rest, "Software\\Microsoft\\Windows");
    }

    #[test]
    fn split_hive_accepts_backslash() {
        let (hive, rest) = split_hive("HKLM\\Software").expect("parse");
        assert_eq!(hive, HiveTag::Hklm);
        assert_eq!(rest, "Software");
    }

    #[test]
    fn split_hive_unknown_returns_none() {
        assert!(split_hive("HKXX\\Whatever").is_none());
        assert!(split_hive("HKCU").is_none());
        assert!(split_hive("").is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn reg_key_returns_not_supported_on_non_windows() {
        let err =
            eval_reg_key("HKCU\\Software\\Grex", None).expect_err("non-Windows reg_key must error");
        match err {
            ExecError::PredicateNotSupported { predicate, platform } => {
                assert_eq!(predicate, "reg_key");
                // Also guards the `platform` field against drift (covers
                // the brief's "platform field matches std::env::consts::OS"
                // sanity check).
                assert_eq!(platform, std::env::consts::OS);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(not(windows))]
    #[test]
    fn ps_version_returns_not_supported_on_non_windows() {
        let err = eval_ps_version(">=5.1").expect_err("non-Windows psversion must error");
        match err {
            ExecError::PredicateNotSupported { predicate, platform } => {
                assert_eq!(predicate, "psversion");
                assert_eq!(platform, std::env::consts::OS);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[cfg(windows)]
    #[test]
    fn reg_key_finds_well_known_hklm_software() {
        // HKLM\Software exists on every Windows install.
        let ok = eval_reg_key("HKLM\\Software", None).expect("probe succeeds");
        assert!(ok, "HKLM\\Software must be present");
    }

    #[cfg(windows)]
    #[test]
    fn reg_key_forward_slash_matches_backslash() {
        // F6: the two separator spellings must be semantically equal.
        let back = eval_reg_key("HKLM\\Software", None).expect("probe succeeds");
        let fwd = eval_reg_key("HKLM/Software", None).expect("probe succeeds");
        assert_eq!(back, fwd);
        assert!(fwd, "HKLM/Software must resolve identically to HKLM\\Software");
    }

    #[cfg(windows)]
    #[test]
    fn reg_key_missing_path_returns_false() {
        let ok = eval_reg_key("HKCU\\Software\\Grex-Probe-DoesNotExist-4f2a9d", None)
            .expect("probe succeeds");
        assert!(!ok, "bogus subkey must not be reported present");
    }

    #[cfg(windows)]
    #[test]
    fn reg_key_rejects_unknown_hive() {
        let ok = eval_reg_key("HKXX\\Whatever", None).expect("probe succeeds");
        assert!(!ok, "unknown hive prefix must evaluate to false");
    }

    #[cfg(windows)]
    #[test]
    fn ps_version_returns_plausible_major() {
        // Every supported Windows host ships PowerShell >= 1.
        let ok = eval_ps_version(">=1").expect("probe succeeds");
        assert!(ok, "PowerShell major >= 1 expected on Windows");
    }

    #[cfg(windows)]
    #[test]
    fn ps_version_rejects_unreachable_future_minor() {
        // F1: a minor far beyond anything real must be rejected rather
        // than silently accepted (the bug we are closing).
        let ok = eval_ps_version(">=9999.0").expect("probe succeeds");
        assert!(!ok, ">=9999.0 must not be reported installed");
    }

    #[cfg(windows)]
    #[test]
    fn ps_version_boundary_51_against_real_host() {
        // F1: boundary test — every supported Windows host ships >= 5.1.
        let ok = eval_ps_version(">=5.1").expect("probe succeeds");
        assert!(ok, "PowerShell (major, minor) >= (5, 1) expected on Windows");
    }
}
