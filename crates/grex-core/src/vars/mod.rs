//! Variable expansion for action-argument strings.
//!
//! Action definitions in `.grex/pack.yaml` preserve variable placeholders as
//! literals at pack-parse time (see [`crate::pack`]). At action-execute time
//! each string is passed through [`expand`], which resolves the placeholders
//! against a [`VarEnv`].
//!
//! # Accepted forms
//!
//! - `$NAME`    — POSIX bare. NAME runs while bytes match `[A-Za-z0-9_]`.
//! - `${NAME}`  — POSIX braced. Must close with `}`.
//! - `%NAME%`   — Windows. Must close with a second `%`.
//!
//! # Escapes
//!
//! - `$$` → literal `$`.
//! - `%%` → literal `%`.
//!
//! Backslash escapes (`\$`, `\%`) are **not** recognised; the backslash
//! passes through literally. See the authoritative spec in
//! `.omne/cfg/actions.md` §Variable expansion and `openspec/feat-grex/spec.md`
//! §"M3 Stage B — Variable expansion".
//!
//! # Non-recursive
//!
//! Expanded values are NOT re-scanned. If `$A` expands to the literal string
//! `$B`, the final output contains the two bytes `$B`; `$B` is not
//! subsequently resolved.

pub mod error;

use std::collections::HashMap;

pub use self::error::VarExpandError;

/// Environment map used by [`expand`].
///
/// Keys are stored case-sensitively in `inner` so `iter`-style consumers
/// (and debug output) see the original casing pack authors wrote. Lookup
/// via [`VarEnv::get`] is platform-aware:
///
/// * **Unix/macOS** — direct case-sensitive lookup.
/// * **Windows** — case-insensitive: a secondary `lookup_index` maps the
///   ASCII-lowercased key to the original-cased key in `inner`. This
///   mirrors OS behaviour where `%Path%` and `%PATH%` name the same var.
///
/// The double-map costs ~1 pointer per entry on Windows only. No new deps
/// (`UniCase` et al. considered and rejected).
#[derive(Debug, Default, Clone)]
pub struct VarEnv {
    inner: HashMap<String, String>,
    /// Windows-only: lowercase key → original-cased key present in `inner`.
    ///
    /// Kept in lock-step with `inner` by [`VarEnv::insert`]. Absent on
    /// non-Windows targets so Unix behaviour is bit-identical to the prior
    /// case-sensitive implementation.
    #[cfg(windows)]
    lookup_index: HashMap<String, String>,
}

impl VarEnv {
    /// Construct an empty environment.
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
            #[cfg(windows)]
            lookup_index: HashMap::new(),
        }
    }

    /// Construct an environment snapshot from the current process.
    ///
    /// Uses [`std::env::vars`]. On Windows, after collecting the snapshot a
    /// `HOME → %USERPROFILE%` fallback is materialised as a real entry when
    /// `HOME` is absent and `USERPROFILE` is present, so pack authors can
    /// write `${HOME}` portably. The fallback is applied in `from_os` only
    /// — never in [`VarEnv::new`] or [`VarEnv::insert`] — so tests that
    /// build envs explicitly see only what they inserted.
    #[must_use]
    pub fn from_os() -> Self {
        let map: HashMap<String, String> = std::env::vars().collect();
        Self::from_map(map)
    }

    /// Build a `VarEnv` from an explicit map, applying the same Windows
    /// HOME→USERPROFILE fallback as [`VarEnv::from_os`].
    ///
    /// Exposed primarily for tests and advanced callers that construct a
    /// synthetic environment. On non-Windows targets this is a thin
    /// wrapper around the map; on Windows it populates `lookup_index` and
    /// the HOME fallback.
    #[must_use]
    pub fn from_map(map: HashMap<String, String>) -> Self {
        let mut env = Self::new();
        for (k, v) in map {
            env.insert(k, v);
        }
        #[cfg(windows)]
        {
            if env.get("HOME").is_none() {
                if let Some(userprofile) = env.get("USERPROFILE").map(str::to_owned) {
                    env.insert("HOME", userprofile);
                }
            }
        }
        env
    }

    /// Insert or overwrite a variable.
    ///
    /// On Windows, also refreshes the case-insensitive lookup index so
    /// subsequent [`VarEnv::get`] calls match any casing of `name`.
    pub fn insert(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = value.into();
        #[cfg(windows)]
        {
            let lower = name.to_ascii_lowercase();
            // Drop any prior original-cased entry that maps to the same
            // lowercase slot so `iter()` does not surface two casings for
            // one logical variable.
            if let Some(prior) = self.lookup_index.get(&lower) {
                if prior != &name {
                    self.inner.remove(prior);
                }
            }
            self.lookup_index.insert(lower, name.clone());
        }
        self.inner.insert(name, value);
    }

    /// Look up a variable by name.
    ///
    /// On Windows, an exact-case hit is tried first; on miss, the lookup
    /// falls back to an ASCII-lowercased match via the secondary index.
    /// On Unix/macOS the lookup is strictly case-sensitive.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&str> {
        if let Some(v) = self.inner.get(name) {
            return Some(v.as_str());
        }
        #[cfg(windows)]
        {
            let lower = name.to_ascii_lowercase();
            if let Some(original) = self.lookup_index.get(&lower) {
                return self.inner.get(original).map(String::as_str);
            }
        }
        None
    }
}

/// Expand variable placeholders in `input` against `env`.
///
/// See the [module-level docs](self) for accepted forms, escape rules, and
/// the non-recursive guarantee.
///
/// # Errors
///
/// Returns a [`VarExpandError`] variant on any malformed placeholder, invalid
/// variable name, or lookup miss. All errors carry a byte offset into
/// `input` pointing at the opening sigil of the offending placeholder.
pub fn expand(input: &str, env: &VarEnv) -> Result<String, VarExpandError> {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0usize;

    while i < bytes.len() {
        match bytes[i] {
            b'$' => i = scan_dollar(bytes, i, env, &mut out)?,
            b'%' => i = scan_percent(bytes, i, env, &mut out)?,
            b => {
                out.push(b as char);
                i += 1;
            }
        }
    }

    Ok(out)
}

/// Handle a `$`-introduced token. On entry `bytes[start] == b'$'`. Returns
/// the index of the first byte after the consumed token.
fn scan_dollar(
    bytes: &[u8],
    start: usize,
    env: &VarEnv,
    out: &mut String,
) -> Result<usize, VarExpandError> {
    debug_assert_eq!(bytes[start], b'$');
    let next = bytes.get(start + 1).copied();

    match next {
        // `$$` → literal `$`
        Some(b'$') => {
            out.push('$');
            Ok(start + 2)
        }
        // `${NAME}`
        Some(b'{') => scan_braced(bytes, start, env, out),
        // `$NAME`
        Some(b) if is_name_start(b) => {
            let name_start = start + 1;
            let mut end = name_start;
            while end < bytes.len() && is_name_cont(bytes[end]) {
                end += 1;
            }
            let name = &bytes[name_start..end];
            resolve(name, start, env, out)?;
            Ok(end)
        }
        // `$` followed by digit or other non-name-start byte, or EOF.
        _ => {
            let (name_end, found_non_name) = scan_trailing_name(bytes, start + 1);
            let got = String::from_utf8_lossy(&bytes[start + 1..name_end]).into_owned();
            // If we saw no bytes at all after `$`, report the `$` itself.
            let got = if got.is_empty() && !found_non_name { String::new() } else { got };
            Err(VarExpandError::InvalidVariableName { got, offset: start })
        }
    }
}

/// Scan a greedy run of `[A-Za-z0-9_]` bytes starting at `from` for error
/// reporting. Returns `(end_index, saw_non_name_byte)` where
/// `saw_non_name_byte` is true if scanning stopped on a non-name byte
/// (vs end-of-input).
fn scan_trailing_name(bytes: &[u8], from: usize) -> (usize, bool) {
    let mut end = from;
    while end < bytes.len() && is_name_cont(bytes[end]) {
        end += 1;
    }
    let stopped_on_byte = end < bytes.len();
    (end, stopped_on_byte)
}

/// Scan `${NAME}`. On entry `bytes[start..start+2] == b"${"`.
fn scan_braced(
    bytes: &[u8],
    start: usize,
    env: &VarEnv,
    out: &mut String,
) -> Result<usize, VarExpandError> {
    debug_assert!(bytes[start] == b'$' && bytes[start + 1] == b'{');
    let name_start = start + 2;
    let mut end = name_start;
    while end < bytes.len() && bytes[end] != b'}' {
        end += 1;
    }
    if end >= bytes.len() {
        return Err(VarExpandError::UnclosedBraceExpansion { offset: start });
    }
    let name = &bytes[name_start..end];
    if name.is_empty() {
        return Err(VarExpandError::EmptyBraceExpansion { offset: start });
    }
    resolve(name, start, env, out)?;
    Ok(end + 1)
}

/// Handle a `%`-introduced token. On entry `bytes[start] == b'%'`. Returns
/// the index of the first byte after the consumed token.
fn scan_percent(
    bytes: &[u8],
    start: usize,
    env: &VarEnv,
    out: &mut String,
) -> Result<usize, VarExpandError> {
    debug_assert_eq!(bytes[start], b'%');
    // `%%` → literal `%`
    if bytes.get(start + 1).copied() == Some(b'%') {
        out.push('%');
        return Ok(start + 2);
    }
    // `%NAME%`
    let name_start = start + 1;
    let mut end = name_start;
    while end < bytes.len() && bytes[end] != b'%' {
        end += 1;
    }
    if end >= bytes.len() {
        return Err(VarExpandError::UnclosedPercentExpansion { offset: start });
    }
    let name = &bytes[name_start..end];
    // An empty name between `%%` is impossible here because the leading `%%`
    // branch is taken above; but a stray `%` immediately followed by `%`
    // elsewhere would have been consumed as the escape. Defensive check:
    if name.is_empty() {
        // This indicates adjacent `%%` that somehow got here — treat as
        // literal `%` pair to be safe. Scanner invariants make this branch
        // unreachable via the `%%` check above, but we refuse to panic.
        out.push('%');
        return Ok(end + 1);
    }
    resolve(name, start, env, out)?;
    Ok(end + 1)
}

/// Validate `name` against `^[A-Za-z_][A-Za-z0-9_]*$`, look it up, and push
/// the resolved value onto `out`. `offset` is the byte offset of the opening
/// sigil (for error context).
fn resolve(
    name: &[u8],
    offset: usize,
    env: &VarEnv,
    out: &mut String,
) -> Result<(), VarExpandError> {
    if !is_valid_name(name) {
        return Err(VarExpandError::InvalidVariableName {
            got: String::from_utf8_lossy(name).into_owned(),
            offset,
        });
    }
    // `is_valid_name` guarantees ASCII bytes, so from_utf8 is infallible.
    let name_str = std::str::from_utf8(name).expect("validated ASCII");
    match env.get(name_str) {
        Some(value) => {
            out.push_str(value);
            Ok(())
        }
        None => Err(VarExpandError::MissingVariable { name: name_str.to_owned(), offset }),
    }
}

#[inline]
fn is_name_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

#[inline]
fn is_name_cont(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_valid_name(name: &[u8]) -> bool {
    match name.first() {
        Some(&b) if is_name_start(b) => name[1..].iter().all(|&c| is_name_cont(c)),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> VarEnv {
        let mut e = VarEnv::new();
        for (k, v) in pairs {
            e.insert(*k, *v);
        }
        e
    }

    #[test]
    fn expand_noop_no_vars() {
        let e = VarEnv::new();
        assert_eq!(expand("plain text / no sigils", &e).unwrap(), "plain text / no sigils");
        assert_eq!(expand("", &e).unwrap(), "");
    }

    #[test]
    fn expand_posix_bare() {
        let e = env(&[("HOME", "/h")]);
        assert_eq!(expand("$HOME/foo", &e).unwrap(), "/h/foo");
    }

    #[test]
    fn expand_posix_braced() {
        let e = env(&[("USER", "yueyang")]);
        assert_eq!(expand("${USER}-log", &e).unwrap(), "yueyang-log");
    }

    #[test]
    fn expand_windows_percent() {
        let e = env(&[("USERPROFILE", "C:\\Users\\y")]);
        assert_eq!(expand("%USERPROFILE%\\x", &e).unwrap(), "C:\\Users\\y\\x");
    }

    #[test]
    fn expand_escape_dollar() {
        // `$$HOME` → literal `$HOME` (escape consumes `$$`, then `HOME`
        // passes through as plain text).
        let e = VarEnv::new();
        assert_eq!(expand("$$HOME", &e).unwrap(), "$HOME");
    }

    #[test]
    fn expand_escape_percent() {
        // `%%PATH%%` → `%%` + `PATH` + `%%` → `%PATH%` literal.
        let e = VarEnv::new();
        assert_eq!(expand("%%PATH%%", &e).unwrap(), "%PATH%");
    }

    #[test]
    fn expand_missing_var_errors() {
        let e = VarEnv::new();
        assert_eq!(
            expand("$UNDEFINED", &e).unwrap_err(),
            VarExpandError::MissingVariable { name: "UNDEFINED".into(), offset: 0 }
        );
    }

    #[test]
    fn expand_unclosed_brace() {
        let e = VarEnv::new();
        assert_eq!(
            expand("${FOO", &e).unwrap_err(),
            VarExpandError::UnclosedBraceExpansion { offset: 0 }
        );
    }

    #[test]
    fn expand_unclosed_percent() {
        let e = VarEnv::new();
        assert_eq!(
            expand("%FOO", &e).unwrap_err(),
            VarExpandError::UnclosedPercentExpansion { offset: 0 }
        );
    }

    #[test]
    fn expand_empty_brace() {
        let e = VarEnv::new();
        assert_eq!(
            expand("${}", &e).unwrap_err(),
            VarExpandError::EmptyBraceExpansion { offset: 0 }
        );
    }

    #[test]
    fn expand_invalid_name_digit_led() {
        let e = VarEnv::new();
        let err = expand("$0FOO", &e).unwrap_err();
        match err {
            VarExpandError::InvalidVariableName { got, offset } => {
                assert_eq!(got, "0FOO");
                assert_eq!(offset, 0);
            }
            other => panic!("expected InvalidVariableName, got {other:?}"),
        }
    }

    #[test]
    fn expand_invalid_name_hyphen() {
        let e = VarEnv::new();
        let err = expand("${BAD-NAME}", &e).unwrap_err();
        match err {
            VarExpandError::InvalidVariableName { got, offset } => {
                assert_eq!(got, "BAD-NAME");
                assert_eq!(offset, 0);
            }
            other => panic!("expected InvalidVariableName, got {other:?}"),
        }
    }

    #[test]
    fn expand_no_recursive() {
        // A=$B, B=boom. Expanding $A yields literal "$B", NOT "boom".
        let e = env(&[("A", "$B"), ("B", "boom")]);
        assert_eq!(expand("$A", &e).unwrap(), "$B");
    }

    #[test]
    fn expand_boundary_adjacent() {
        let e = env(&[("HOME", "/h"), ("USER", "y")]);
        assert_eq!(expand("$HOME/path_$USER", &e).unwrap(), "/h/path_y");
    }

    #[test]
    fn expand_dollar_at_end() {
        // Bare `$` with no following name char → InvalidVariableName at offset 0.
        let e = VarEnv::new();
        let err = expand("trailing$", &e).unwrap_err();
        match err {
            VarExpandError::InvalidVariableName { got, offset } => {
                assert_eq!(got, "");
                assert_eq!(offset, 8);
            }
            other => panic!("expected InvalidVariableName, got {other:?}"),
        }
    }

    #[test]
    fn expand_percent_isolated_mid() {
        // Policy: single `%` with no matching close is UnclosedPercentExpansion.
        // Literal `%` requires `%%`.
        let e = VarEnv::new();
        assert_eq!(
            expand("50% off", &e).unwrap_err(),
            VarExpandError::UnclosedPercentExpansion { offset: 2 }
        );
    }

    #[test]
    fn expand_offset_is_sigil_position() {
        // Verify reported offset tracks position of the opening sigil, not
        // the start of the input.
        let e = VarEnv::new();
        let err = expand("prefix-${MISSING}", &e).unwrap_err();
        match err {
            VarExpandError::MissingVariable { name, offset } => {
                assert_eq!(name, "MISSING");
                assert_eq!(offset, 7);
            }
            other => panic!("expected MissingVariable, got {other:?}"),
        }
    }

    #[test]
    fn var_env_from_os() {
        // Smoke test: from_os captures the current process env. On every
        // supported platform at least PATH is set in CI and locally.
        let e = VarEnv::from_os();
        assert!(e.get("PATH").is_some() || e.get("Path").is_some());
    }

    #[test]
    fn var_env_get_and_insert() {
        let mut e = VarEnv::new();
        assert_eq!(e.get("X"), None);
        e.insert("X", "1");
        assert_eq!(e.get("X"), Some("1"));
        e.insert("X", "2");
        assert_eq!(e.get("X"), Some("2"));
    }

    #[cfg(windows)]
    #[test]
    fn var_env_windows_case_insensitive_get() {
        let mut e = VarEnv::new();
        e.insert("PATH", "c:/bin");
        assert_eq!(e.get("PATH"), Some("c:/bin"));
        assert_eq!(e.get("Path"), Some("c:/bin"));
        assert_eq!(e.get("path"), Some("c:/bin"));
    }

    #[cfg(windows)]
    #[test]
    fn var_env_windows_home_fallback_from_userprofile() {
        // from_map (same fallback path as from_os) synthesises HOME when
        // USERPROFILE is present and HOME is absent.
        let mut seed = HashMap::new();
        seed.insert("USERPROFILE".to_string(), r"C:\Users\y".to_string());
        let env = VarEnv::from_map(seed);
        assert_eq!(env.get("HOME"), Some(r"C:\Users\y"));
        // Case-insensitive lookup also finds it.
        assert_eq!(env.get("home"), Some(r"C:\Users\y"));
    }

    #[cfg(windows)]
    #[test]
    fn var_env_windows_home_fallback_not_applied_by_insert() {
        // Plain insert() does NOT synthesise HOME — the fallback is a
        // from_os/from_map-only convenience.
        let mut e = VarEnv::new();
        e.insert("USERPROFILE", r"C:\Users\y");
        assert_eq!(e.get("HOME"), None);
    }

    #[cfg(unix)]
    #[test]
    fn var_env_unix_case_sensitive_still() {
        let mut e = VarEnv::new();
        e.insert("PATH", "/usr/bin");
        assert_eq!(e.get("PATH"), Some("/usr/bin"));
        assert_eq!(e.get("Path"), None);
        assert_eq!(e.get("path"), None);
    }
}
