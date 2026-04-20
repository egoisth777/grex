//! Predicate grammar for `require` and `when` blocks.
//!
//! A predicate tree is a recursive structure of leaf checks (path exists,
//! command available, etc.) composed by Boolean combiners (`all_of`,
//! `any_of`, `none_of`). Parsing is key-dispatched — never `#[serde(untagged)]`
//! — so error messages can cite the offending key precisely.
//!
//! Execute-time evaluation is a later stage; Stage A only parses and
//! preserves the tree.

use serde::{Deserialize, Serialize};

use super::error::{PackParseError, MAX_REQUIRE_DEPTH};

/// Operating-system matcher used by `os:` predicates and `when.os`.
///
/// Marked `#[non_exhaustive]` so future OS tags (BSD variants, WASM,
/// embedded) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OsKind {
    /// Microsoft Windows.
    Windows,
    /// Linux kernel (any distro).
    Linux,
    /// Apple macOS.
    Macos,
}

/// Behaviour when a `require` block evaluates to false.
///
/// Per `actions.md` §require, the legal set here is `error | skip | warn`.
/// `ignore` (an `exec`-only form) is deliberately rejected at parse time.
///
/// Marked `#[non_exhaustive]` so future on-fail modes (e.g. `prompt`) can
/// land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum RequireOnFail {
    /// Abort pack install with a non-zero exit code.
    #[default]
    Error,
    /// Skip remaining actions in this pack; lifecycle reports "skipped".
    Skip,
    /// Log a warning and continue.
    Warn,
}

/// Behaviour when an `exec` invocation returns a non-zero exit code.
///
/// Per `actions.md` §exec, the legal set here is `error | warn | ignore`.
/// `skip` (a `require`-only form) is deliberately rejected at parse time.
///
/// Marked `#[non_exhaustive]` so future on-fail modes can land without
/// breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ExecOnFail {
    /// Propagate the non-zero exit code and fail the pack lifecycle.
    #[default]
    Error,
    /// Log a warning but continue running remaining actions.
    Warn,
    /// Treat the non-zero exit as success (used for idempotency workarounds).
    Ignore,
}

/// A single leaf check or a nested combiner.
///
/// Parsed from a single-key YAML map via [`Predicate::from_yaml`]. The enum
/// intentionally mirrors the key set documented in `actions.md`.
///
/// Marked `#[non_exhaustive]` so new leaf predicates (plugin-contributed or
/// spec-extension) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Predicate {
    /// Filesystem path must exist.
    PathExists(String),
    /// Named command is resolvable via `PATH`.
    CmdAvailable(String),
    /// Windows registry value is present. Only the map form
    /// `{ path, name }` is accepted — the legacy `hive\path!name` string
    /// form is rejected at parse time for unambiguity.
    RegKey {
        /// Registry path including hive (e.g. `HKCU\Software\...`).
        path: String,
        /// Optional value name within the key.
        name: Option<String>,
    },
    /// Current OS matches.
    Os(OsKind),
    /// PowerShell version spec (e.g. `>=5.1`).
    PsVersion(String),
    /// Privilege / developer-mode permits symlink creation for `src` → `dst`.
    SymlinkOk {
        /// Symlink source path.
        src: String,
        /// Symlink destination path.
        dst: String,
    },
    /// Nested AND combiner.
    AllOf(Vec<Predicate>),
    /// Nested OR combiner.
    AnyOf(Vec<Predicate>),
    /// Nested NOR combiner.
    NoneOf(Vec<Predicate>),
}

/// The one-of combiner declared at the top level of a `require` or `when`
/// block. Exactly one variant is populated at parse time.
///
/// Marked `#[non_exhaustive]` so new combiner shapes (e.g. `xor_of`,
/// `majority_of`) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Combiner {
    /// `all_of:` — every predicate must hold (AND).
    AllOf(Vec<Predicate>),
    /// `any_of:` — at least one predicate must hold (OR).
    AnyOf(Vec<Predicate>),
    /// `none_of:` — no predicate may hold (NOR).
    NoneOf(Vec<Predicate>),
}

impl Predicate {
    /// Parse a single predicate entry from a `serde_yaml::Value`.
    ///
    /// Each entry must be a map with **exactly one** key naming the
    /// predicate kind. Depth-limited by [`MAX_REQUIRE_DEPTH`] to bound
    /// pathological nesting.
    pub fn from_yaml(value: &serde_yaml::Value, depth: usize) -> Result<Self, PackParseError> {
        let (key, v) = predicate_single_key(value, depth)?;
        match key.as_str() {
            "path_exists" => parse_path_exists(v),
            "cmd_available" => parse_cmd_available(v),
            "reg_key" => parse_reg_key(v),
            "os" => parse_os(v),
            "psversion" => parse_ps_version(v),
            "symlink_ok" => parse_symlink_ok(v),
            "all_of" => parse_all_of(v, depth),
            "any_of" => parse_any_of(v, depth),
            "none_of" => parse_none_of(v, depth),
            other => Err(unknown_predicate_err(other)),
        }
    }
}

/// Enforce the depth bound and unwrap the single-key mapping shape shared by
/// every predicate entry. Returns the owned key name and a reference to its
/// value.
fn predicate_single_key(
    value: &serde_yaml::Value,
    depth: usize,
) -> Result<(String, &serde_yaml::Value), PackParseError> {
    if depth >= MAX_REQUIRE_DEPTH {
        return Err(PackParseError::RequireDepthExceeded { depth, max: MAX_REQUIRE_DEPTH });
    }

    let mapping = value.as_mapping().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: "predicate must be a single-key mapping".to_string(),
    })?;

    if mapping.len() != 1 {
        return Err(PackParseError::InvalidPredicate {
            detail: format!("predicate must be a single-key mapping (got {} keys)", mapping.len()),
        });
    }

    let (k, v) = mapping.iter().next().expect("len==1 checked above");
    let key = k.as_str().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: "predicate key must be a string".to_string(),
    })?;
    Ok((key.to_string(), v))
}

fn parse_path_exists(value: &serde_yaml::Value) -> Result<Predicate, PackParseError> {
    Ok(Predicate::PathExists(string_arg(value, "path_exists")?))
}

fn parse_cmd_available(value: &serde_yaml::Value) -> Result<Predicate, PackParseError> {
    Ok(Predicate::CmdAvailable(string_arg(value, "cmd_available")?))
}

fn parse_reg_key(value: &serde_yaml::Value) -> Result<Predicate, PackParseError> {
    Ok(Predicate::RegKey { path: reg_path(value)?, name: reg_name(value)? })
}

fn parse_os(value: &serde_yaml::Value) -> Result<Predicate, PackParseError> {
    Ok(Predicate::Os(serde_yaml::from_value::<OsKind>(value.clone())?))
}

fn parse_ps_version(value: &serde_yaml::Value) -> Result<Predicate, PackParseError> {
    Ok(Predicate::PsVersion(string_arg(value, "psversion")?))
}

fn parse_symlink_ok(value: &serde_yaml::Value) -> Result<Predicate, PackParseError> {
    Ok(Predicate::SymlinkOk {
        src: map_string(value, "symlink_ok", "src")?,
        dst: map_string(value, "symlink_ok", "dst")?,
    })
}

fn parse_all_of(value: &serde_yaml::Value, depth: usize) -> Result<Predicate, PackParseError> {
    Ok(Predicate::AllOf(parse_list(value, depth + 1)?))
}

fn parse_any_of(value: &serde_yaml::Value, depth: usize) -> Result<Predicate, PackParseError> {
    Ok(Predicate::AnyOf(parse_list(value, depth + 1)?))
}

fn parse_none_of(value: &serde_yaml::Value, depth: usize) -> Result<Predicate, PackParseError> {
    Ok(Predicate::NoneOf(parse_list(value, depth + 1)?))
}

fn unknown_predicate_err(key: &str) -> PackParseError {
    PackParseError::InvalidPredicate {
        detail: format!(
            "unknown predicate {key:?}: valid kinds are path_exists, cmd_available, \
reg_key, os, psversion, symlink_ok, all_of, any_of, none_of"
        ),
    }
}

impl Combiner {
    /// Parse a combiner from a YAML mapping. Caller is responsible for
    /// handing down only the subset of keys relevant to combiner selection
    /// (typically the full mapping; non-combiner keys are ignored by this
    /// fn).
    ///
    /// Exactly one of `all_of` / `any_of` / `none_of` must be present.
    pub fn from_mapping(
        mapping: &serde_yaml::Mapping,
        depth: usize,
    ) -> Result<Self, PackParseError> {
        let mut seen: Vec<(&'static str, &serde_yaml::Value)> = Vec::new();
        for key in ["all_of", "any_of", "none_of"] {
            if let Some(v) = mapping.get(serde_yaml::Value::String(key.to_string())) {
                seen.push((key, v));
            }
        }
        if seen.len() != 1 {
            return Err(PackParseError::RequireCombinerArity { count: seen.len() });
        }
        let (key, value) = seen[0];
        let list = parse_list(value, depth + 1)?;
        Ok(match key {
            "all_of" => Self::AllOf(list),
            "any_of" => Self::AnyOf(list),
            "none_of" => Self::NoneOf(list),
            _ => unreachable!("iteration set is fixed"),
        })
    }
}

/// Parse a YAML sequence of predicate entries.
fn parse_list(value: &serde_yaml::Value, depth: usize) -> Result<Vec<Predicate>, PackParseError> {
    let seq = value.as_sequence().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: "combiner value must be a sequence of predicate entries".to_string(),
    })?;
    seq.iter().map(|v| Predicate::from_yaml(v, depth)).collect()
}

fn string_arg(value: &serde_yaml::Value, key: &str) -> Result<String, PackParseError> {
    value.as_str().map(str::to_owned).ok_or_else(|| PackParseError::InvalidPredicate {
        detail: format!("{key} expects a string argument"),
    })
}

/// `reg_key` only accepts the map form `{ path, name }`. The legacy
/// `hive\path!name` string form is rejected — the spec never defined it and
/// ambiguity between a literal `!` in a registry path and the name
/// separator motivates the strict shape.
fn reg_path(value: &serde_yaml::Value) -> Result<String, PackParseError> {
    if value.as_str().is_some() {
        return Err(PackParseError::InvalidPredicate {
            detail: "reg_key string form is not supported: use { path, name } map".to_string(),
        });
    }
    map_string(value, "reg_key", "path")
}

fn reg_name(value: &serde_yaml::Value) -> Result<Option<String>, PackParseError> {
    if value.as_str().is_some() {
        return Err(PackParseError::InvalidPredicate {
            detail: "reg_key string form is not supported: use { path, name } map".to_string(),
        });
    }
    match value.as_mapping() {
        Some(m) => match m.get(serde_yaml::Value::String("name".to_string())) {
            Some(v) if v.is_null() => Ok(None),
            Some(v) => v.as_str().map(str::to_owned).map(Some).ok_or_else(|| {
                PackParseError::InvalidPredicate {
                    detail: "reg_key.name must be a string".to_string(),
                }
            }),
            None => Ok(None),
        },
        None => Err(PackParseError::InvalidPredicate {
            detail: "reg_key expects a { path, name } map".to_string(),
        }),
    }
}

fn map_string(
    value: &serde_yaml::Value,
    outer: &str,
    field: &str,
) -> Result<String, PackParseError> {
    let map = value.as_mapping().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: format!("{outer} expects a mapping argument"),
    })?;
    let v = map.get(serde_yaml::Value::String(field.to_string())).ok_or_else(|| {
        PackParseError::InvalidPredicate {
            detail: format!("{outer} missing required field {field:?}"),
        }
    })?;
    v.as_str().map(str::to_owned).ok_or_else(|| PackParseError::InvalidPredicate {
        detail: format!("{outer}.{field} must be a string"),
    })
}
