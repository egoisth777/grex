//! Bare-name validator for `children[].path`.
//!
//! Per the pack-spec ([`man/concepts/pack-spec.md`] §"Validation rules"):
//! `children[].path` must be a bare name — no path separators, no
//! `.` / `..`, no empty string. The same regex as the pack `name` field
//! is enforced: `^[a-z][a-z0-9-]*$` (letter-led, lowercase, hyphens
//! allowed).
//!
//! # Why enforce now
//!
//! Before v1.1.0 the runtime resolved children inside a fixed
//! sub-directory of the pack root, which bounded any path-traversal
//! attempt. v1.1.0 resolves children as flat siblings of the parent
//! pack root, so a hostile (or buggy) `path: "../escape"` would now
//! land directly under the parent root's siblings — far less
//! recoverable. The bare-name rule has been declared in the spec since
//! v1.0.0; this validator finally enforces it.
//!
//! # Why effective_path() is not the right place
//!
//! `ChildRef::effective_path` returns the literal `path` field
//! verbatim. Adding runtime checks there would push validation into the
//! hot dispatch path; the existing convention is plan-phase validation,
//! so this validator runs once via `run_all` before any walker /
//! executor reaches the field.

use super::{PackValidationError, Validator};
use crate::pack::{ChildRef, PackManifest};

/// Validates that every `children[].path` value (or the URL-derived tail
/// when `path:` is omitted) is a bare name matching the same regex as
/// `pack.name`: `^[a-z][a-z0-9-]*$`.
///
/// Two attribution modes:
///
/// * **Explicit `path:`** — rejected with the original literal value,
///   labelled by the `path` string itself.
/// * **Omitted `path:`** — the URL-tail derivation in
///   [`crate::pack::ChildRef::effective_path`] is computed and validated.
///   Rejected entries are labelled by the URL (since the user never
///   wrote a `path` to attribute against) and the `path` field of the
///   error carries the derived tail.
pub struct ChildPathValidator;

impl Validator for ChildPathValidator {
    fn name(&self) -> &'static str {
        "child_path_bare_name"
    }

    fn check(&self, pack: &PackManifest) -> Vec<PackValidationError> {
        let mut errs = Vec::new();
        for child in &pack.children {
            if let Some(err) = check_one(child) {
                errs.push(err);
            }
        }
        errs
    }
}

/// Validate one child: explicit `path:` is checked verbatim; otherwise
/// the URL-tail derivation is checked. Returns `None` when the child's
/// effective path is acceptable.
#[must_use]
pub fn check_one(child: &ChildRef) -> Option<PackValidationError> {
    let (effective, attribution) = match child.path.as_deref() {
        Some(p) => (p.to_string(), Attribution::Explicit(p.to_string())),
        None => (child.effective_path(), Attribution::UrlDerived(child.url.clone())),
    };
    let reason = reject_reason(&effective)?;
    let (child_name, path) = match attribution {
        Attribution::Explicit(label) => (label.clone(), label),
        Attribution::UrlDerived(url) => (url, effective),
    };
    Some(PackValidationError::ChildPathInvalid {
        child_name,
        path,
        reason: reason.to_string(),
    })
}

enum Attribution {
    Explicit(String),
    UrlDerived(String),
}

/// Reject `path` with a one-line reason string when it violates the
/// bare-name rule. Returns `None` when the path is acceptable.
///
/// Exposed at `pub(crate)` so the tree walker can run the same
/// rejection logic before any clone fires (closing the path-traversal
/// window between manifest load and `walker.resolve_destination`).
///
/// Order matters for the message — the most specific failure mode wins
/// so authors get a useful diagnostic instead of "regex did not match".
pub(crate) fn reject_reason(path: &str) -> Option<&'static str> {
    if path.is_empty() {
        return Some("empty string is not a valid child path");
    }
    if path.contains('/') || path.contains('\\') {
        return Some("path separators are not allowed (children[].path must be a bare name)");
    }
    if path == "." || path == ".." {
        return Some("`.` and `..` are not allowed (children[].path must be a bare name)");
    }
    if !matches_bare_name_regex(path) {
        return Some(
            "must match `^[a-z][a-z0-9-]*$` (letter-led, lowercase, digits and hyphens allowed)",
        );
    }
    None
}

/// Mirrors the `^[a-z][a-z0-9-]*$` regex used by `pack.name`. Inlined to
/// avoid pulling the `regex` crate into `grex-core` solely for this one
/// match; the predicate is small enough to verify by eye.
fn matches_bare_name_regex(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{ChildRef, PackManifest, PackType, SchemaVersion};
    use std::collections::BTreeMap;

    fn pack_with_child_paths(paths: &[&str]) -> PackManifest {
        let children = paths
            .iter()
            .map(|p| ChildRef {
                url: format!("https://example.invalid/{p}"),
                path: Some((*p).to_string()),
                r#ref: None,
            })
            .collect();
        PackManifest {
            schema_version: SchemaVersion::current(),
            name: "p".to_string(),
            r#type: PackType::Meta,
            version: None,
            depends_on: Vec::new(),
            children,
            actions: Vec::new(),
            teardown: None,
            extensions: BTreeMap::new(),
        }
    }

    fn validate_path(path: &str) -> Vec<PackValidationError> {
        ChildPathValidator.check(&pack_with_child_paths(&[path]))
    }

    /// Table-driven sweep of every rejection mode + every accept mode.
    /// Substring assertions on the reason string keep the test resilient
    /// to message rewording without losing the "which sub-rule fired"
    /// signal.
    #[test]
    fn rejection_table() {
        let cases: &[(&str, &str)] = &[
            ("", "empty"),
            ("foo/bar", "separator"),
            ("foo\\bar", "separator"),
            ("/abs", "separator"),
            ("../escape", "separator"),
            (".", "`.` and `..`"),
            ("..", "`.` and `..`"),
            ("Foo", "`^[a-z]"),
            ("1foo", "letter-led"),
        ];
        for (input, expected_reason_substr) in cases {
            let errs = validate_path(input);
            assert_eq!(errs.len(), 1, "input {input:?}");
            match &errs[0] {
                PackValidationError::ChildPathInvalid { path, reason, .. } => {
                    assert_eq!(path, input, "input {input:?}");
                    assert!(
                        reason.contains(expected_reason_substr),
                        "input {input:?} reason: {reason}",
                    );
                }
                other => panic!("input {input:?} wrong variant: {other:?}"),
            }
        }
    }

    #[test]
    fn accept_table() {
        for ok in ["foo", "a", "algo-leet", "foo-bar", "foo123", "a1-b2"] {
            assert!(validate_path(ok).is_empty(), "input {ok:?} should accept");
        }
    }

    #[test]
    fn url_derived_tail_is_validated_when_path_absent() {
        // Acceptable URL tail.
        let ok = PackManifest {
            schema_version: SchemaVersion::current(),
            name: "p".to_string(),
            r#type: PackType::Meta,
            version: None,
            depends_on: Vec::new(),
            children: vec![ChildRef {
                url: "https://example.invalid/foo.git".to_string(),
                path: None,
                r#ref: None,
            }],
            actions: Vec::new(),
            teardown: None,
            extensions: BTreeMap::new(),
        };
        assert!(ChildPathValidator.check(&ok).is_empty());

        // Hostile URL tail — `..` after stripping `.git`. Validator must
        // catch this even though `path:` is absent.
        let bad = PackManifest {
            schema_version: SchemaVersion::current(),
            name: "p".to_string(),
            r#type: PackType::Meta,
            version: None,
            depends_on: Vec::new(),
            children: vec![ChildRef {
                url: "https://example.invalid/...git".to_string(),
                path: None,
                r#ref: None,
            }],
            actions: Vec::new(),
            teardown: None,
            extensions: BTreeMap::new(),
        };
        let errs = ChildPathValidator.check(&bad);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        match &errs[0] {
            PackValidationError::ChildPathInvalid { child_name, path, .. } => {
                // URL-derived: child_name carries the URL (since the user
                // never wrote a path to attribute against), path carries
                // the derived tail.
                assert_eq!(child_name, "https://example.invalid/...git");
                assert_eq!(path, "..");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn aggregates_errors_across_multiple_children() {
        let pack = pack_with_child_paths(&["good", "foo/bar", "..", "ALSO-BAD"]);
        let errs = ChildPathValidator.check(&pack);
        // 3 bad: "foo/bar", "..", "ALSO-BAD". "good" is fine.
        assert_eq!(errs.len(), 3, "errs: {errs:?}");
    }
}
