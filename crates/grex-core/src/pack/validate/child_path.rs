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
use crate::pack::PackManifest;

/// Validates that every `children[].path` (when explicitly set) is a
/// bare name matching the same regex as `pack.name`:
/// `^[a-z][a-z0-9-]*$`.
///
/// When `path` is absent the implicit URL-tail derivation in
/// [`crate::pack::ChildRef::effective_path`] is trusted — that derivation
/// strips trailing `.git` and the last URL segment, both of which are
/// already constrained by the URL grammar. Authors who want stricter
/// enforcement on URL-derived paths can set `path` explicitly.
pub struct ChildPathValidator;

impl Validator for ChildPathValidator {
    fn name(&self) -> &'static str {
        "child_path_bare_name"
    }

    fn check(&self, pack: &PackManifest) -> Vec<PackValidationError> {
        let mut errs = Vec::new();
        for child in &pack.children {
            let Some(path) = child.path.as_deref() else { continue };
            if let Some(reason) = reject_reason(path) {
                errs.push(PackValidationError::ChildPathInvalid {
                    child_name: derive_child_label(child),
                    path: path.to_string(),
                    reason: reason.to_string(),
                });
            }
        }
        errs
    }
}

/// Reject `path` with a one-line reason string when it violates the
/// bare-name rule. Returns `None` when the path is acceptable.
///
/// Order matters for the message — the most specific failure mode wins
/// so authors get a useful diagnostic instead of "regex did not match".
fn reject_reason(path: &str) -> Option<&'static str> {
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

/// Pick a stable label for the offending child in the error message.
/// Prefers the literal path (already known invalid) so authors can grep;
/// falls back to the URL when the path field is somehow absent (which
/// `check` guards against, but defensive in case the surface evolves).
fn derive_child_label(child: &crate::pack::ChildRef) -> String {
    child.path.clone().unwrap_or_else(|| child.url.clone())
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

    fn check_one(path: &str) -> Vec<PackValidationError> {
        ChildPathValidator.check(&pack_with_child_paths(&[path]))
    }

    #[test]
    fn accepts_bare_lowercase_name() {
        assert!(check_one("foo").is_empty());
        assert!(check_one("a").is_empty());
        assert!(check_one("algo-leet").is_empty());
        assert!(check_one("foo-bar").is_empty());
        assert!(check_one("foo123").is_empty());
        assert!(check_one("a1-b2").is_empty());
    }

    #[test]
    fn rejects_empty_string() {
        let errs = check_one("");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackValidationError::ChildPathInvalid { path, reason, .. } => {
                assert_eq!(path, "");
                assert!(reason.contains("empty"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_forward_slash() {
        let errs = check_one("foo/bar");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackValidationError::ChildPathInvalid { reason, .. } => {
                assert!(reason.contains("separator"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_backslash() {
        let errs = check_one("foo\\bar");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackValidationError::ChildPathInvalid { reason, .. } => {
                assert!(reason.contains("separator"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_dot_and_dotdot() {
        for bad in [".", ".."] {
            let errs = check_one(bad);
            assert_eq!(errs.len(), 1, "input {bad:?}");
            match &errs[0] {
                PackValidationError::ChildPathInvalid { reason, .. } => {
                    assert!(reason.contains("`.`") && reason.contains("`..`"), "reason: {reason}");
                }
                other => panic!("wrong variant for {bad:?}: {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_parent_traversal() {
        let errs = check_one("../escape");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            // `../escape` contains `/` so it trips the separator check first.
            PackValidationError::ChildPathInvalid { reason, .. } => {
                assert!(reason.contains("separator"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_absolute_unix_path() {
        let errs = check_one("/abs");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackValidationError::ChildPathInvalid { reason, .. } => {
                assert!(reason.contains("separator"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_uppercase_letter_lead() {
        let errs = check_one("Foo");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackValidationError::ChildPathInvalid { reason, .. } => {
                assert!(reason.contains("`^[a-z]"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn rejects_digit_lead() {
        let errs = check_one("1foo");
        assert_eq!(errs.len(), 1);
        match &errs[0] {
            PackValidationError::ChildPathInvalid { reason, .. } => {
                assert!(reason.contains("letter-led"), "reason: {reason}");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn skips_check_when_path_absent() {
        // No `path:` field — derivation from URL takes over and is trusted.
        let pack = PackManifest {
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
        assert!(ChildPathValidator.check(&pack).is_empty());
    }

    #[test]
    fn aggregates_errors_across_multiple_children() {
        let pack = pack_with_child_paths(&["good", "foo/bar", "..", "ALSO-BAD"]);
        let errs = ChildPathValidator.check(&pack);
        // 3 bad: "foo/bar", "..", "ALSO-BAD". "good" is fine.
        assert_eq!(errs.len(), 3, "errs: {errs:?}");
    }
}
