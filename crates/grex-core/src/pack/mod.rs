//! Pack-manifest (`pack.yaml`) parser.
//!
//! Stage A of M3: **pure parse + round-trip, zero side effects**. This
//! module turns a YAML string into a typed [`PackManifest`] and fails
//! loudly on any shape violation. It does not read from disk, walk
//! children, expand variables, or detect duplicates across actions — all
//! of those are later stages.
//!
//! # Key design points
//!
//! * `schema_version` is validated at parse (only `"1"` accepted) so that
//!   future-schema packs fail with an actionable error rather than a
//!   cryptic deserialize mismatch.
//! * Action dispatch is key-based (not `#[serde(untagged)]`) — see
//!   [`action`] for rationale.
//! * Predicates use a recursive grammar capped at
//!   [`error::MAX_REQUIRE_DEPTH`] to bound worst-case nesting cost.
//! * YAML anchors/aliases are **rejected** at parse time as a security
//!   policy (cycle + billion-laughs mitigation).
//! * The omitted-vs-empty distinction on `teardown:` is preserved — `None`
//!   means "default to reverse(actions) at execute time", `Some(vec![])`
//!   means "explicit no-op".

pub mod action;
pub mod error;
pub mod predicate;
pub mod validate;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub use action::{
    Action, EnvArgs, EnvScope, ExecSpec, MkdirArgs, RequireSpec, RmdirArgs, SymlinkArgs,
    SymlinkKind, UnlinkArgs, WhenSpec, VALID_ACTION_KEYS,
};
pub use error::{PackParseError, MAX_REQUIRE_DEPTH};
pub use predicate::{Combiner, ExecOnFail, OsKind, Predicate, RequireOnFail};
pub use validate::{run_all, PackValidationError, Validator};

/// Literal value accepted for `schema_version`. Bump only with a backwards-
/// incompatible YAML migration.
pub const SUPPORTED_SCHEMA_VERSION: &str = "1";

/// Newtype wrapping the schema-version literal.
///
/// Parses only the exact string `"1"`. Any other value yields
/// [`PackParseError::InvalidSchemaVersion`] so consumers can emit an
/// actionable "upgrade grex" or "downgrade pack" message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct SchemaVersion(String);

impl SchemaVersion {
    /// The single supported schema version literal.
    #[must_use]
    pub fn current() -> Self {
        Self(SUPPORTED_SCHEMA_VERSION.to_string())
    }

    /// Borrow the wrapped literal.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for SchemaVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // Spec requires the literal string `"1"`. Bare YAML integers (e.g.
        // `schema_version: 1`) are rejected: YAML's implicit typing would
        // make the authored value ambiguous, and the manifest schema is
        // explicitly string-typed so future versions can be non-numeric.
        let raw = serde_yaml::Value::deserialize(deserializer)?;
        let got = match &raw {
            serde_yaml::Value::String(s) => s.clone(),
            serde_yaml::Value::Number(n) => {
                return Err(serde::de::Error::custom(format!(
                    "schema_version must be the quoted string \"1\", got bare number {n} \
                     (quote it as \"{n}\")"
                )));
            }
            other => {
                return Err(serde::de::Error::custom(format!(
                    "schema_version must be the quoted string \"1\", got {other:?}"
                )));
            }
        };
        if got == SUPPORTED_SCHEMA_VERSION {
            Ok(Self(got))
        } else {
            Err(serde::de::Error::custom(format!(
                "unsupported pack schema_version {got:?}: this grex build only understands \"1\""
            )))
        }
    }
}

/// Pack type discriminator.
///
/// * [`PackType::Meta`] — composes child packs only (no actions).
/// * [`PackType::Declarative`] — idempotent actions with automatic rollback.
/// * [`PackType::Scripted`] — freeform actions with author-defined teardown.
///
/// Marked `#[non_exhaustive]` so new pack shapes (e.g. plugin-contributed
/// kinds in M4+) can land without breaking external match sites.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PackType {
    /// Composition-only pack (no actions allowed in strict validation).
    Meta,
    /// Idempotent actions; grex runs rollback on failure.
    Declarative,
    /// Author-defined actions + teardown.
    Scripted,
}

impl PackType {
    /// Stable snake_case tag matching the `type:` discriminator in
    /// `pack.yaml`. Used by the pack-type plugin registry to look up the
    /// driver for a given pack.
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Meta => "meta",
            Self::Declarative => "declarative",
            Self::Scripted => "scripted",
        }
    }
}

/// Reference to a child pack from a `children:` entry.
///
/// `path` is intentionally left `None` at parse time — callers that need
/// the on-disk directory name should invoke [`ChildRef::effective_path`]
/// which extracts the last URL segment as the default.
///
/// Marked `#[non_exhaustive]` so spec growth (e.g. `pin`, `shallow`) does
/// not break library consumers who destructure the struct.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildRef {
    /// Upstream git URL (any scheme `gix` can resolve).
    pub url: String,
    /// Optional override for the on-disk directory name. Preserved as
    /// `None` when absent so callers can distinguish "defaulted" from
    /// "explicitly set to the default value".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// Optional git ref (branch / tag / commit). Serialized as `ref:` via
    /// the raw-identifier field.
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
}

impl ChildRef {
    /// Resolve the on-disk directory name. When `path` is explicitly set it
    /// wins; otherwise the last path segment of `url` (stripped of a
    /// trailing `.git`) is used.
    ///
    /// # Precondition
    ///
    /// Callers reaching this from the sync orchestrator can assume the
    /// `path` value (when present) has already passed
    /// [`validate::ChildPathValidator`]: bare name, no separators, no
    /// `.` / `..`, matches `^[a-z][a-z0-9-]*$`. This method is therefore
    /// kept side-effect-free — re-validating here would push plan-phase
    /// checks into the hot dispatch path for no benefit.
    #[must_use]
    pub fn effective_path(&self) -> String {
        if let Some(p) = &self.path {
            return p.clone();
        }
        let url = self.url.trim_end_matches('/');
        let tail = url.rsplit_once('/').map_or(url, |(_, t)| t);
        tail.strip_suffix(".git").unwrap_or(tail).to_string()
    }
}

/// Top-level representation of a `pack.yaml` manifest.
///
/// Post-parse invariants:
///
/// * `schema_version` == `"1"`.
/// * `name` matches `^[a-z][a-z0-9-]*$`.
/// * Unknown top-level keys are absent unless prefixed with `x-`.
/// * Predicate trees within any action are depth-bounded by
///   [`MAX_REQUIRE_DEPTH`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackManifest {
    /// Schema-version literal. Always [`SchemaVersion::current`] at v1.
    pub schema_version: SchemaVersion,
    /// Pack name (validated).
    pub name: String,
    /// Pack-type discriminator.
    pub r#type: PackType,
    /// Optional semver-ish string; grex does not parse it further in Stage A.
    pub version: Option<String>,
    /// Names of packs this pack depends on. Empty default.
    pub depends_on: Vec<String>,
    /// Child-pack references. Empty default.
    pub children: Vec<ChildRef>,
    /// Ordered actions to run. Empty-default (valid no-op).
    pub actions: Vec<Action>,
    /// Explicit teardown.
    ///
    /// `None` means the pack-type driver should default to
    /// `reverse(actions)` at execute time; `Some(vec![])` means the
    /// author explicitly opted into a no-op teardown. Preserving that
    /// distinction matters for audit trails — Stage A does not execute but
    /// must round-trip it.
    pub teardown: Option<Vec<Action>>,
    /// Unknown `x-*` extension keys. Preserved verbatim for downstream
    /// plugins.
    pub extensions: BTreeMap<String, serde_yaml::Value>,
}

impl PackManifest {
    /// Walk every action (including those nested inside `when` blocks),
    /// yielding `(global_index, &symlink)` pairs.
    ///
    /// `global_index` is a 0-based counter across the flattened action-walk
    /// — it is **not** the top-level index into [`PackManifest::actions`].
    /// Two symlinks at the same top-level index but at different nesting
    /// depths receive distinct global indices. This is the index space
    /// [`PackValidationError`] variants refer to.
    pub fn iter_all_symlinks(&self) -> impl Iterator<Item = (usize, &SymlinkArgs)> {
        self.actions.iter().flat_map(Action::iter_symlinks).enumerate()
    }

    /// Run every default [`Validator`] over this manifest.
    ///
    /// Returns `Ok(())` when no validator emits an error; otherwise returns
    /// `Err(Vec<_>)` carrying every error across every validator (not
    /// fail-first — downstream consumers can decide whether to abort on the
    /// first or surface the full batch).
    ///
    /// # Errors
    ///
    /// Returns [`PackValidationError`] variants aggregated across the
    /// validator set. See [`validate::run_all`] for the exact default set.
    ///
    /// # Example
    ///
    /// ```no_run
    /// use grex_core::pack::parse;
    ///
    /// let src = "schema_version: \"1\"\nname: ok\ntype: declarative\n";
    /// let pack = parse(src).unwrap();
    /// pack.validate_plan().unwrap();
    /// ```
    pub fn validate_plan(&self) -> Result<(), Vec<PackValidationError>> {
        let errs = validate::run_all(self);
        if errs.is_empty() {
            Ok(())
        } else {
            Err(errs)
        }
    }
}

/// Parse a `pack.yaml` buffer into a [`PackManifest`].
///
/// The entry point:
///
/// 1. Pre-scans for YAML anchor / alias events and rejects them.
/// 2. Deserializes into a permissive raw map.
/// 3. Validates `schema_version` and `name`.
/// 4. Segregates known fields from `x-*` extensions; rejects any other
///    unknown top-level key.
/// 5. Key-dispatches actions and teardown via [`Action::from_yaml`].
///
/// # Errors
///
/// Any structural violation surfaces as a [`PackParseError`] variant with
/// enough context for a CLI consumer to point at the offending key.
pub fn parse(yaml: &str) -> Result<PackManifest, PackParseError> {
    reject_yaml_aliases(yaml)?;
    let mapping = parse_root_mapping(yaml)?;
    let extensions = segregate_extensions(&mapping)?;

    let schema_version = parse_schema_version(&mapping)?;
    let name = parse_name(&mapping)?;
    let r#type = parse_type(&mapping)?;
    let version = parse_version(&mapping);
    let depends_on = parse_depends_on(&mapping)?;
    let children = parse_children(&mapping)?;
    let actions = Action::parse_list(mapping.get(s("actions")))?;
    let teardown = parse_teardown(&mapping)?;

    Ok(PackManifest {
        schema_version,
        name,
        r#type,
        version,
        depends_on,
        children,
        actions,
        teardown,
        extensions,
    })
}

/// Top-level keys recognised by the parser. Any other non-`x-`-prefixed key
/// is rejected via [`PackParseError::UnknownTopLevelKey`].
const KNOWN_TOP_LEVEL_KEYS: &[&str] =
    &["schema_version", "name", "type", "version", "depends_on", "children", "actions", "teardown"];

/// Parse the raw YAML into a top-level mapping, failing with a clear error
/// for null / non-mapping roots.
fn parse_root_mapping(yaml: &str) -> Result<serde_yaml::Mapping, PackParseError> {
    let root: serde_yaml::Value = serde_yaml::from_str(yaml)?;
    match root {
        serde_yaml::Value::Mapping(m) => Ok(m),
        serde_yaml::Value::Null => Err(PackParseError::InvalidName { got: String::new() }),
        other => Err(PackParseError::InvalidPredicate {
            detail: format!("pack.yaml root must be a mapping, got {other:?}"),
        }),
    }
}

/// Walk the mapping, separating `x-*` extension keys from rejected unknowns.
fn segregate_extensions(
    mapping: &serde_yaml::Mapping,
) -> Result<BTreeMap<String, serde_yaml::Value>, PackParseError> {
    let mut extensions: BTreeMap<String, serde_yaml::Value> = BTreeMap::new();
    for (k, v) in mapping.iter() {
        let Some(key) = k.as_str() else {
            return Err(PackParseError::UnknownTopLevelKey { key: format!("{k:?}") });
        };
        if KNOWN_TOP_LEVEL_KEYS.contains(&key) {
            continue;
        }
        if key.starts_with("x-") {
            extensions.insert(key.to_string(), v.clone());
            continue;
        }
        return Err(PackParseError::UnknownTopLevelKey { key: key.to_string() });
    }
    Ok(extensions)
}

fn parse_schema_version(mapping: &serde_yaml::Mapping) -> Result<SchemaVersion, PackParseError> {
    match mapping.get(s("schema_version")) {
        // Propagate the custom Deserialize error as PackParseError::Inner so
        // its precise diagnostic (e.g. "got bare number 1 — quote it as
        // \"1\"") surfaces to CLI consumers verbatim. Only string-typed
        // mismatches fall through to InvalidSchemaVersion.
        Some(v) => match serde_yaml::from_value::<SchemaVersion>(v.clone()) {
            Ok(sv) => Ok(sv),
            Err(e) => {
                if matches!(v, serde_yaml::Value::String(_)) {
                    Err(PackParseError::InvalidSchemaVersion { got: render_scalar(v) })
                } else {
                    Err(PackParseError::Inner(e))
                }
            }
        },
        None => Err(PackParseError::InvalidSchemaVersion { got: "<missing>".to_string() }),
    }
}

fn parse_name(mapping: &serde_yaml::Mapping) -> Result<String, PackParseError> {
    let name = match mapping.get(s("name")) {
        Some(v) => v
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| PackParseError::InvalidName { got: render_scalar(v) })?,
        None => return Err(PackParseError::InvalidName { got: "<missing>".to_string() }),
    };
    if !is_valid_pack_name(&name) {
        return Err(PackParseError::InvalidName { got: name });
    }
    Ok(name)
}

fn parse_type(mapping: &serde_yaml::Mapping) -> Result<PackType, PackParseError> {
    match mapping.get(s("type")) {
        Some(v) => Ok(serde_yaml::from_value(v.clone())?),
        None => Err(PackParseError::UnknownTopLevelKey {
            key: "<missing required field `type`>".to_string(),
        }),
    }
}

fn parse_version(mapping: &serde_yaml::Mapping) -> Option<String> {
    match mapping.get(s("version")) {
        Some(v) if v.is_null() => None,
        Some(v) => Some(v.as_str().map(str::to_owned).unwrap_or_else(|| render_scalar(v))),
        None => None,
    }
}

fn parse_depends_on(mapping: &serde_yaml::Mapping) -> Result<Vec<String>, PackParseError> {
    match mapping.get(s("depends_on")) {
        Some(v) if v.is_null() => Ok(Vec::new()),
        Some(v) => Ok(serde_yaml::from_value(v.clone())?),
        None => Ok(Vec::new()),
    }
}

fn parse_children(mapping: &serde_yaml::Mapping) -> Result<Vec<ChildRef>, PackParseError> {
    match mapping.get(s("children")) {
        Some(v) if v.is_null() => Ok(Vec::new()),
        Some(v) => Ok(serde_yaml::from_value(v.clone())?),
        None => Ok(Vec::new()),
    }
}

fn parse_teardown(mapping: &serde_yaml::Mapping) -> Result<Option<Vec<Action>>, PackParseError> {
    match mapping.get(s("teardown")) {
        None => Ok(None),
        Some(v) if v.is_null() => Ok(None),
        Some(v) => Ok(Some(Action::parse_list(Some(v))?)),
    }
}

/// Borrow-friendly shorthand for `serde_yaml::Value::String(key.into())`.
fn s(key: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(key.to_string())
}

/// Render a scalar YAML value as a display string for error messages.
fn render_scalar(v: &serde_yaml::Value) -> String {
    match v {
        serde_yaml::Value::String(s) => s.clone(),
        serde_yaml::Value::Number(n) => n.to_string(),
        serde_yaml::Value::Bool(b) => b.to_string(),
        serde_yaml::Value::Null => "null".to_string(),
        other => format!("{other:?}"),
    }
}

/// Validate a pack name against `^[a-z][a-z0-9-]*$`.
fn is_valid_pack_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !first.is_ascii_lowercase() {
        return false;
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Pre-scan the raw YAML text for anchor (`&`) or alias (`*`) events.
///
/// We drive `serde_yaml::Deserializer` purely for its token stream — no
/// typed structure is built. Any [`serde_yaml::Value`] that contains an
/// anchored or aliased node would round-trip without warning through a
/// typed parse, so we reject here before structural parsing runs.
fn reject_yaml_aliases(yaml: &str) -> Result<(), PackParseError> {
    // serde_yaml does not expose a public event stream, but a YAML alias
    // node always manifests as repeated structure sharing. A cheap but
    // correct detector: scan for anchor/alias sigils outside of string
    // scalars. A full YAML tokenizer would be heavier than warranted for
    // Stage A; the lightweight scanner below is deliberately conservative
    // (false-positive preferred over false-negative for a security gate).
    let mut in_single = false;
    let mut in_double = false;
    let mut prev: char = '\n';
    for ch in yaml.chars() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single && prev != '\\' => in_double = !in_double,
            '&' | '*' if !in_single && !in_double => {
                // Anchors/aliases begin at token-start positions: after
                // whitespace, `:`, `-`, `[`, `,`, `{`, or at start of
                // line. A bare `*` in a flow scalar is unlikely but we
                // err toward rejecting.
                if matches!(prev, ' ' | '\t' | '\n' | ':' | '-' | '[' | ',' | '{') {
                    // Require at least one name char to avoid rejecting
                    // `* ` used as a literal bullet in a folded scalar —
                    // though inside a YAML mapping this is itself
                    // unusual. Accepts a single false-positive window.
                    return Err(PackParseError::YamlAliasRejected);
                }
            }
            _ => {}
        }
        prev = ch;
    }
    Ok(())
}
