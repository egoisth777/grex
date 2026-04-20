//! Tier-1 action variants and their key-dispatched deserializer.
//!
//! Every action in a pack's `actions:` list is a YAML map with **exactly
//! one key**. That key names the action; its value carries the typed
//! arguments. We reject `#[serde(untagged)]` for dispatch because its
//! error messages collapse all variant attempts into "did not match any
//! variant" — useless for authors. Instead we:
//!
//! 1. Deserialize each entry into a `RawAction` (a single-key map).
//! 2. Inspect the key, dispatch to the correct typed arg deserializer.
//! 3. Emit a precise error citing the offending key.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::error::PackParseError;
use super::predicate::{Combiner, ExecOnFail, OsKind, Predicate, RequireOnFail};

/// Symlink link-kind selector.
///
/// Marked `#[non_exhaustive]` so future platform-specific kinds (e.g. NTFS
/// junctions) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SymlinkKind {
    /// Infer from `src` (file → file-link, dir → dir-link). Default.
    #[default]
    Auto,
    /// Force a file-symlink (Windows `symlink_file`).
    File,
    /// Force a directory-symlink (Windows `symlink_dir`).
    Directory,
}

/// Environment-variable persistence scope.
///
/// Marked `#[non_exhaustive]` so future scopes (e.g. per-shell rc-file,
/// systemd user-session) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EnvScope {
    /// Current user (HKCU / shell rc). Default.
    #[default]
    User,
    /// System-wide (HKLM / `/etc/environment`). Needs admin.
    Machine,
    /// Current process only.
    Session,
}

/// `- symlink: { ... }`
///
/// Marked `#[non_exhaustive]` so spec additions (e.g. `relative`, `force`)
/// in later milestones do not break external library consumers who
/// destructure the struct.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SymlinkArgs {
    /// Source path, relative to pack workdir.
    pub src: String,
    /// Destination path (may contain env-var tokens; not expanded at parse).
    pub dst: String,
    /// Rename existing `dst` before creating the link. Defaults to `false`.
    #[serde(default)]
    pub backup: bool,
    /// Canonicalize both sides. Defaults to `true`.
    #[serde(default = "default_true")]
    pub normalize: bool,
    /// Link kind selector.
    #[serde(default)]
    pub kind: SymlinkKind,
}

fn default_true() -> bool {
    true
}

impl SymlinkArgs {
    /// Construct a [`SymlinkArgs`] with all current fields in canonical
    /// order. Exposed so external callers (and in-workspace test crates)
    /// can materialise values even though the struct is
    /// `#[non_exhaustive]`.
    #[must_use]
    pub fn new(src: String, dst: String, backup: bool, normalize: bool, kind: SymlinkKind) -> Self {
        Self { src, dst, backup, normalize, kind }
    }
}

impl EnvArgs {
    /// Construct an [`EnvArgs`] with all current fields in canonical order.
    #[must_use]
    pub fn new(name: String, value: String, scope: EnvScope) -> Self {
        Self { name, value, scope }
    }
}

impl MkdirArgs {
    /// Construct a [`MkdirArgs`] with all current fields in canonical order.
    #[must_use]
    pub fn new(path: String, mode: Option<String>) -> Self {
        Self { path, mode }
    }
}

impl RmdirArgs {
    /// Construct a [`RmdirArgs`] with all current fields in canonical order.
    #[must_use]
    pub fn new(path: String, backup: bool, force: bool) -> Self {
        Self { path, backup, force }
    }
}

impl RequireSpec {
    /// Construct a [`RequireSpec`] with all current fields in canonical order.
    #[must_use]
    pub fn new(combiner: Combiner, on_fail: RequireOnFail) -> Self {
        Self { combiner, on_fail }
    }
}

impl WhenSpec {
    /// Construct a [`WhenSpec`] with all current fields in canonical order.
    #[must_use]
    pub fn new(
        os: Option<OsKind>,
        all_of: Option<Vec<Predicate>>,
        any_of: Option<Vec<Predicate>>,
        none_of: Option<Vec<Predicate>>,
        actions: Vec<Action>,
    ) -> Self {
        Self { os, all_of, any_of, none_of, actions }
    }
}

impl ExecSpec {
    /// Construct an [`ExecSpec`] with all current fields in canonical order.
    #[must_use]
    pub fn new(
        cmd: Option<Vec<String>>,
        cmd_shell: Option<String>,
        shell: bool,
        cwd: Option<String>,
        env: Option<BTreeMap<String, String>>,
        on_fail: ExecOnFail,
    ) -> Self {
        Self { cmd, cmd_shell, shell, cwd, env, on_fail }
    }
}

/// `- env: { ... }`
///
/// Marked `#[non_exhaustive]` so the spec can grow new knobs (e.g.
/// `append`, `only_if_unset`) without breaking library consumers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvArgs {
    /// Variable name.
    pub name: String,
    /// Variable value (pre-expansion form).
    pub value: String,
    /// Persistence scope. Defaults to [`EnvScope::User`].
    #[serde(default)]
    pub scope: EnvScope,
}

/// `- mkdir: { ... }`
///
/// Marked `#[non_exhaustive]` so spec-level growth (ownership, umask
/// overrides, …) is non-breaking for library consumers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MkdirArgs {
    /// Directory to create.
    pub path: String,
    /// POSIX mode string (ignored on Windows).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

/// `- rmdir: { ... }`
///
/// Marked `#[non_exhaustive]` so spec-level growth (retention policy,
/// tombstone dir override, …) is non-breaking for library consumers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RmdirArgs {
    /// Directory to remove.
    pub path: String,
    /// Rename to `<path>.grex-bak.<ts>` instead of deleting. Default `false`.
    #[serde(default)]
    pub backup: bool,
    /// Allow recursive delete of non-empty directory. Default `false`.
    #[serde(default)]
    pub force: bool,
}

/// `- require: { ... }` — prerequisite / idempotency gate.
///
/// Marked `#[non_exhaustive]` so M4 lockfile integration can attach
/// additional audit fields (hash-pinning, cache tokens) without breaking
/// downstream destructuring.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequireSpec {
    /// Combiner populated by `all_of` / `any_of` / `none_of`.
    pub combiner: Combiner,
    /// Behaviour when the combiner evaluates to false.
    pub on_fail: RequireOnFail,
}

/// `- when: { ... }` — conditional gate wrapping nested actions.
///
/// Per `actions.md`, the shorthand `os:` and the explicit combiners
/// compose conjunctively. Stage A preserves all fields as-is; evaluation
/// logic is a later stage.
///
/// Marked `#[non_exhaustive]` so new shorthand gates can land without
/// breaking library consumers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WhenSpec {
    /// Shorthand OS gate (equivalent to `os:` predicate in an implicit AND).
    pub os: Option<OsKind>,
    /// Explicit AND combiner predicates.
    pub all_of: Option<Vec<Predicate>>,
    /// Explicit OR combiner predicates.
    pub any_of: Option<Vec<Predicate>>,
    /// Explicit NOR combiner predicates.
    pub none_of: Option<Vec<Predicate>>,
    /// Nested actions to run when the composite condition holds.
    pub actions: Vec<Action>,
}

/// `- exec: { ... }` — shell-escape hatch.
///
/// The `cmd` XOR `cmd_shell` invariant is enforced in the custom
/// deserializer. `shell=false` (default) requires `cmd`; `shell=true`
/// requires `cmd_shell`.
///
/// Marked `#[non_exhaustive]` so spec growth (timeout, stdout capture,
/// sandboxing flags) is non-breaking for library consumers.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ExecSpec {
    /// Argv form. Populated when `shell=false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd: Option<Vec<String>>,
    /// Single-string shell form. Populated when `shell=true`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cmd_shell: Option<String>,
    /// Whether to parse through `sh -c` / `cmd /c`.
    pub shell: bool,
    /// Working directory. Defaults to pack workdir at execute time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Extra environment variables for this invocation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env: Option<BTreeMap<String, String>>,
    /// Error-propagation policy.
    pub on_fail: ExecOnFail,
}

/// One entry in a pack's `actions:` (or `teardown:`) list.
///
/// Marked `#[non_exhaustive]` because M4 ships plugin-contributed action
/// kinds; external match sites must carry a `_` arm so the Tier-1 registry
/// can grow without a major-version bump.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `symlink` primitive.
    Symlink(SymlinkArgs),
    /// `env` primitive.
    Env(EnvArgs),
    /// `mkdir` primitive.
    Mkdir(MkdirArgs),
    /// `rmdir` primitive.
    Rmdir(RmdirArgs),
    /// `require` gate.
    Require(RequireSpec),
    /// `when` conditional block.
    When(WhenSpec),
    /// `exec` shell escape.
    Exec(ExecSpec),
}

/// Valid action keys. Re-exported for documentation + error-message
/// composition.
pub const VALID_ACTION_KEYS: &[&str] =
    &["symlink", "env", "mkdir", "rmdir", "require", "when", "exec"];

impl Action {
    /// Parse a single action entry from a YAML value.
    ///
    /// Rejects zero-key and multi-key entries with
    /// [`PackParseError::EmptyActionEntry`] / [`PackParseError::MultipleActionKeys`],
    /// and unknown keys with [`PackParseError::UnknownActionKey`].
    pub fn from_yaml(value: &serde_yaml::Value) -> Result<Self, PackParseError> {
        let (key, v) = single_key_entry(value)?;
        match key.as_str() {
            "symlink" => parse_symlink(v),
            "env" => parse_env(v),
            "mkdir" => parse_mkdir(v),
            "rmdir" => parse_rmdir(v),
            "require" => parse_require(v).map(Self::Require),
            "when" => parse_when(v).map(Self::When),
            "exec" => parse_exec(v).map(Self::Exec),
            other => Err(PackParseError::UnknownActionKey { key: other.to_string() }),
        }
    }

    /// Parse an entire `actions:` sequence.
    pub fn parse_list(value: Option<&serde_yaml::Value>) -> Result<Vec<Self>, PackParseError> {
        let Some(value) = value else {
            return Ok(Vec::new());
        };
        if value.is_null() {
            return Ok(Vec::new());
        }
        let seq = value.as_sequence().ok_or_else(|| PackParseError::UnknownActionKey {
            key: "<actions must be a sequence>".to_string(),
        })?;
        seq.iter().map(Self::from_yaml).collect()
    }

    /// Short kebab-case identifier matching the YAML key that produced this
    /// variant (and the name plugins register under). Returned as
    /// `&'static str` so callers can zero-cost compare against constants
    /// like [`crate::execute::ACTION_SYMLINK`].
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Symlink(_) => "symlink",
            Self::Env(_) => "env",
            Self::Mkdir(_) => "mkdir",
            Self::Rmdir(_) => "rmdir",
            Self::Require(_) => "require",
            Self::When(_) => "when",
            Self::Exec(_) => "exec",
        }
    }

    /// Walk this action (and any nested `when.actions`) yielding every
    /// [`SymlinkArgs`] reached.
    ///
    /// * [`Action::Symlink`] yields the wrapped args.
    /// * [`Action::When`] recurses into `when.actions` (which themselves may
    ///   be `when` blocks — recursion is unbounded because the parse-time
    ///   depth bound applies to predicate trees, not action nesting; in
    ///   practice authors do not nest `when` deeply, and validators consume
    ///   whatever the parser accepted).
    /// * Every other variant yields an empty iterator.
    ///
    /// The iterator is boxed so variant-specific concrete iterator types can
    /// share a single return shape. Boxing cost is negligible against the
    /// outer YAML parse and well-bounded action lists; swapping to a custom
    /// enum-iterator later is YAGNI for now.
    #[must_use]
    pub fn iter_symlinks(&self) -> Box<dyn Iterator<Item = &SymlinkArgs> + '_> {
        match self {
            Self::Symlink(s) => Box::new(std::iter::once(s)),
            Self::When(w) => Box::new(w.actions.iter().flat_map(Self::iter_symlinks)),
            _ => Box::new(std::iter::empty()),
        }
    }
}

/// Validate that `value` is a single-key mapping and return the owned key
/// plus a reference to its value. Emits the same errors the inline form did.
fn single_key_entry(
    value: &serde_yaml::Value,
) -> Result<(String, &serde_yaml::Value), PackParseError> {
    let mapping = value.as_mapping().ok_or(PackParseError::EmptyActionEntry)?;
    match mapping.len() {
        0 => return Err(PackParseError::EmptyActionEntry),
        1 => {}
        _ => {
            let keys = mapping.iter().filter_map(|(k, _)| k.as_str().map(str::to_owned)).collect();
            return Err(PackParseError::MultipleActionKeys { keys });
        }
    }
    let (k, v) = mapping.iter().next().expect("len==1 checked above");
    let key =
        k.as_str().ok_or_else(|| PackParseError::UnknownActionKey { key: format!("{k:?}") })?;
    Ok((key.to_string(), v))
}

fn parse_symlink(value: &serde_yaml::Value) -> Result<Action, PackParseError> {
    Ok(Action::Symlink(serde_yaml::from_value(value.clone())?))
}

fn parse_env(value: &serde_yaml::Value) -> Result<Action, PackParseError> {
    Ok(Action::Env(serde_yaml::from_value(value.clone())?))
}

fn parse_mkdir(value: &serde_yaml::Value) -> Result<Action, PackParseError> {
    Ok(Action::Mkdir(serde_yaml::from_value(value.clone())?))
}

fn parse_rmdir(value: &serde_yaml::Value) -> Result<Action, PackParseError> {
    Ok(Action::Rmdir(serde_yaml::from_value(value.clone())?))
}

fn parse_require(value: &serde_yaml::Value) -> Result<RequireSpec, PackParseError> {
    let mapping = value.as_mapping().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: "require: expects a mapping".to_string(),
    })?;
    let combiner = Combiner::from_mapping(mapping, 0)?;
    let on_fail = match mapping.get(serde_yaml::Value::String("on_fail".to_string())) {
        Some(v) => serde_yaml::from_value::<RequireOnFail>(v.clone())?,
        None => RequireOnFail::default(),
    };
    Ok(RequireSpec { combiner, on_fail })
}

fn parse_when(value: &serde_yaml::Value) -> Result<WhenSpec, PackParseError> {
    let mapping = value.as_mapping().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: "when: expects a mapping".to_string(),
    })?;

    let os = match mapping.get(serde_yaml::Value::String("os".to_string())) {
        Some(v) => Some(serde_yaml::from_value::<OsKind>(v.clone())?),
        None => None,
    };

    let all_of = optional_predicate_list(mapping, "all_of")?;
    let any_of = optional_predicate_list(mapping, "any_of")?;
    let none_of = optional_predicate_list(mapping, "none_of")?;

    let actions_value = mapping.get(serde_yaml::Value::String("actions".to_string()));
    let actions = Action::parse_list(actions_value)?;

    Ok(WhenSpec { os, all_of, any_of, none_of, actions })
}

fn optional_predicate_list(
    mapping: &serde_yaml::Mapping,
    key: &str,
) -> Result<Option<Vec<Predicate>>, PackParseError> {
    let Some(value) = mapping.get(serde_yaml::Value::String(key.to_string())) else {
        return Ok(None);
    };
    let seq = value.as_sequence().ok_or_else(|| PackParseError::InvalidPredicate {
        detail: format!("{key} must be a sequence of predicates"),
    })?;
    let preds: Vec<Predicate> =
        seq.iter().map(|v| Predicate::from_yaml(v, 1)).collect::<Result<_, _>>()?;
    Ok(Some(preds))
}

fn parse_exec(value: &serde_yaml::Value) -> Result<ExecSpec, PackParseError> {
    // Shape-flex deserialize: use a helper struct with all fields optional,
    // then enforce the XOR post-parse.
    #[derive(Deserialize)]
    struct Raw {
        #[serde(default)]
        cmd: Option<Vec<String>>,
        #[serde(default)]
        cmd_shell: Option<String>,
        #[serde(default)]
        shell: bool,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        env: Option<BTreeMap<String, String>>,
        #[serde(default)]
        on_fail: ExecOnFail,
    }

    let raw: Raw = serde_yaml::from_value(value.clone())?;

    let cmd_present = raw.cmd.is_some();
    let cmd_shell_present = raw.cmd_shell.is_some();

    let valid = match raw.shell {
        false => cmd_present && !cmd_shell_present,
        true => !cmd_present && cmd_shell_present,
    };
    if !valid {
        return Err(PackParseError::ExecCmdMutex {
            shell: raw.shell,
            cmd_present,
            cmd_shell_present,
        });
    }

    Ok(ExecSpec {
        cmd: raw.cmd,
        cmd_shell: raw.cmd_shell,
        shell: raw.shell,
        cwd: raw.cwd,
        env: raw.env,
        on_fail: raw.on_fail,
    })
}
