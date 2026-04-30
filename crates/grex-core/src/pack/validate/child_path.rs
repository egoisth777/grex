//! Bare-name validator for `children[].path`.
//!
//! Per the pack-spec (`man/concepts/pack-spec.md` §"Validation rules"):
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
///
/// Visibility: `pub(crate)` — the validator is registered in
/// [`super::run_all`] and reached via [`crate::pack::PackManifest::validate_plan`].
/// External consumers do not need to instantiate the struct directly;
/// promoting to `pub` later is a non-breaking additive change.
pub(crate) struct ChildPathValidator;

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
///
/// Visibility: `pub(crate)` — used by the walker pre-clone gate and the
/// duplicate-path validator within this crate; promoting later is a
/// non-breaking additive change.
#[must_use]
pub(crate) fn check_one(child: &ChildRef) -> Option<PackValidationError> {
    let (effective, attribution) = match child.path.as_deref() {
        Some(p) => (p.to_string(), Attribution::Explicit(p.to_string())),
        None => (child.effective_path(), Attribution::UrlDerived(child.url.clone())),
    };
    let reason = reject_reason(&effective)?;
    let (child_name, path) = match attribution {
        Attribution::Explicit(label) => (label.clone(), label),
        Attribution::UrlDerived(url) => (url, effective),
    };
    Some(PackValidationError::ChildPathInvalid { child_name, path, reason: reason.to_string() })
}

enum Attribution {
    Explicit(String),
    UrlDerived(String),
}

/// Validates that within a single parent pack's `children[]` no two
/// entries resolve to the same `effective_path()`. Two children at
/// the same on-disk slot would silently overwrite each other (or
/// worse — once both `.git`s exist, the walker's
/// `dest_has_git_repo` short-circuit would skip-fetch the wrong
/// upstream forever after).
///
/// Comparison is on the resolved effective path (the literal `path`
/// when set, else the URL-tail derivation), so a child with explicit
/// `path: foo` and a sibling with URL `https://x/foo.git` (no
/// `path:`) collide.
///
/// Visibility: `pub(crate)` — see [`ChildPathValidator`] rationale.
pub(crate) struct DupChildPathValidator;

impl Validator for DupChildPathValidator {
    fn name(&self) -> &'static str {
        "child_path_no_duplicates"
    }

    fn check(&self, pack: &PackManifest) -> Vec<PackValidationError> {
        use std::collections::BTreeMap;
        // Bucket URLs by effective path. Skip children that are
        // already invalid (their `effective_path()` may be garbage);
        // the bare-name validator surfaces those independently and
        // duplicate-of-garbage is not a useful additional signal.
        //
        // Cache `effective_path()` per child once and reject via the
        // shared `reject_reason` predicate. Calling `check_one` here
        // would re-compute `effective_path()` internally for every
        // child whose `path:` is omitted; the inline form below shares
        // the resolved string between the rejection check and the
        // bucket insert.
        let mut by_path: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for child in &pack.children {
            let effective = child.effective_path();
            if reject_reason(&effective).is_some() {
                continue;
            }
            by_path.entry(effective).or_default().push(child.url.clone());
        }
        let mut errs = Vec::new();
        for (path, urls) in by_path {
            if urls.len() >= 2 {
                errs.push(PackValidationError::ChildPathDuplicate { path, urls });
            }
        }
        errs
    }
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

// ---------------------------------------------------------------------------
// v1.2.0 Stage 1.c — boundary-preservation rejects.
//
// These checks are layered ON TOP of [`reject_reason`]. They surface via
// `TreeError::ManifestPathEscape` (in the walker's pre-clone gate) and
// catch boundary hazards that the bare-name regex by itself cannot
// distinguish: Unicode normalization collisions, Windows reserved /
// special-char segments, and FS-resident junctions or `.git`-as-file
// references.
//
// Additivity rationale: the existing `reject_reason` continues to be the
// source of truth for literal-syntax violations (separators, dots,
// charset). The boundary helpers below answer a different question —
// "does this NAME (or the on-disk thing it resolves to) re-introduce a
// parent-boundary escape on a case-insensitive or reparse-aware FS?".
// They are intentionally stricter than `reject_reason` so the walker can
// fail fast with a more diagnostic error message.
//
// See `walker.md` §boundary-preservation; the discharge maps to the V1
// Lean theorem that says a validated manifest's children must descend
// from the parent.
// ---------------------------------------------------------------------------

/// Static list of Win32 device names. Per MSDN the same names are
/// reserved with or without an extension; comparison is on the
/// case-insensitive *stem* (everything before the first `.`).
const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Boundary-preservation reject for a single child path segment.
///
/// Returns `Some(reason)` for entries that pass [`reject_reason`] (or
/// would, on a relaxed regex) but still re-open the parent-boundary
/// escape on case-insensitive / reparse-aware filesystems. Returns
/// `None` when the path is acceptable from a boundary standpoint.
///
/// Visibility: `pub(crate)` — called from the tree walker's pre-clone
/// gate and exercised directly by this module's tests.
#[must_use]
pub(crate) fn boundary_reject_reason(path: &str) -> Option<&'static str> {
    // 1. Colon — Windows drive separator (`C:`) and ADS marker
    //    (`name:stream`). Either form opens the boundary.
    if path.contains(':') {
        return Some("colon `:` is not allowed in a child path (Windows drive / ADS hazard)");
    }
    // 2. Dollar — env-var-style interpolation hazard. Forbidden so a
    //    later release can introduce expansion without re-relaxing the
    //    schema.
    if path.contains('$') {
        return Some("dollar `$` is not allowed in a child path (env-var interpolation hazard)");
    }
    // 3. Tilde-digit — Windows 8.3 short-name pattern (`FOO~1.TXT`).
    //    Two distinct long names can collapse onto the same short
    //    alias, so any `~<digit>` segment is rejected. Tilde NOT
    //    followed by a digit is left to the bare-name regex (which
    //    rejects it anyway today; a future regex relaxation that
    //    permits `~` would still need to forbid the `~\d` class).
    if has_tilde_digit_pattern(path) {
        return Some("tilde-digit (`~1`/`~9`/...) is not allowed (Windows short-name hazard)");
    }
    // 4. Windows reserved device names — case-insensitive, with or
    //    without an extension. The stem (everything before the first
    //    `.`) is compared.
    if is_windows_reserved_name(path) {
        return Some(
            "child path is a Windows reserved device name (CON/PRN/AUX/NUL/COM1-9/LPT1-9)",
        );
    }
    None
}

/// Returns `true` when `path` contains a tilde immediately followed by
/// at least one ASCII digit (e.g. `foo~1`, `bar~12`, `~9abc`).
fn has_tilde_digit_pattern(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes
        .iter()
        .enumerate()
        .any(|(i, &b)| b == b'~' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
}

/// Returns `true` when the *stem* of `path` (everything before the
/// first `.`) matches a Win32 reserved device name, case-insensitive.
fn is_windows_reserved_name(path: &str) -> bool {
    let stem = path.split('.').next().unwrap_or(path);
    WINDOWS_RESERVED.iter().any(|reserved| stem.eq_ignore_ascii_case(reserved))
}

/// NFC-duplicate detection across a manifest's `children[]`.
///
/// On case-insensitive Unicode filesystems (HFS+, APFS-default, NTFS
/// when mounted with `nocaseinsensitive`), two paths whose Unicode
/// normalization forms differ but whose NFC-collapsed forms agree will
/// land at the same on-disk slot. The first such offender's *literal*
/// path string is returned so the operator can pinpoint the duplicate.
///
/// Visibility: `pub(crate)` — called from the tree walker's pre-clone
/// gate. Returns `None` when no NFC collision exists.
#[must_use]
pub(crate) fn nfc_duplicate_path(children: &[ChildRef]) -> Option<String> {
    use std::collections::BTreeSet;
    use unicode_normalization::UnicodeNormalization;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for child in children {
        let effective = child.effective_path();
        // Skip empty / dot segments — those carry no semantic name to
        // normalise. `reject_reason` will surface them via the syntactic
        // gate; adding a "duplicate-of-empty" signal here would only
        // confuse. We intentionally do NOT skip charset / regex
        // failures — when the bare-name regex relaxes (planned for
        // Stage 1c.1), this helper must still detect Unicode-form
        // collisions.
        if effective.is_empty() || effective == "." || effective == ".." {
            continue;
        }
        let nfc: String = effective.nfc().collect();
        if !seen.insert(nfc) {
            // First collision wins — surface the offending literal so
            // the operator's error frame echoes what they wrote.
            return Some(effective);
        }
    }
    None
}

/// Filesystem-resident boundary check for a resolved child destination.
///
/// Returns `Some(reason)` when the destination *exists* and is one of:
/// * a Windows reparse point (junction or symlink)
/// * a directory whose `.git` entry is a regular file (gitfile-style
///   `gitdir:` redirect)
///
/// Returns `None` when the destination does not exist (the normal
/// pre-clone case — the walker hasn't materialised the child yet) or
/// is a plain directory with either no `.git` or a `.git/` directory.
///
/// Visibility: `pub(crate)` — called from the tree walker's pre-clone
/// gate, AFTER the destination path has been resolved against the
/// parent workspace. Pure `&Path` interface so the helper composes
/// cleanly with the resolution pipeline.
#[must_use]
pub(crate) fn boundary_fs_reject_reason(dest: &std::path::Path) -> Option<&'static str> {
    let Ok(meta) = std::fs::symlink_metadata(dest) else {
        // Pre-clone: dest doesn't exist yet. Defer junction / gitfile
        // checks to the post-clone verifier (out of scope for this
        // gate). Absence is the happy path here.
        return None;
    };
    let ft = meta.file_type();
    // Symlinks (POSIX symlink, Windows symlink_dir/symlink_file).
    if ft.is_symlink() {
        return Some(
            "child destination is a symlink — refusing to walk into it (boundary escape hazard)",
        );
    }
    // Windows-only: junctions / non-symlink reparse points. `is_symlink`
    // returns `false` for junctions on Windows, so a dedicated probe is
    // required.
    #[cfg(target_os = "windows")]
    {
        if is_windows_reparse_point(&meta) {
            return Some(
                "child destination is a Windows junction or reparse point — refusing to walk into it",
            );
        }
    }
    // Gitfile redirect: `<dest>/.git` is a regular file containing
    // `gitdir: <path>` rather than a directory. The redirect target is
    // unverified by the walker, so we refuse the whole entry.
    let git_entry = dest.join(".git");
    if let Ok(git_meta) = std::fs::symlink_metadata(&git_entry) {
        if git_meta.file_type().is_file() && file_is_gitfile(&git_entry) {
            return Some(
                "child destination's `.git` is a gitfile redirect (boundary escape hazard)",
            );
        }
    }
    None
}

/// Windows-only: detect a non-symlink reparse point (junction / mount
/// point). Reparse points carry the
/// `FILE_ATTRIBUTE_REPARSE_POINT` (0x400) attribute regardless of
/// whether the OS classifies them as symlinks.
#[cfg(target_os = "windows")]
fn is_windows_reparse_point(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

/// Returns `true` when the file at `path` looks like a git-worktree
/// gitfile redirect — a regular text file whose first non-whitespace
/// content is the literal prefix `gitdir:`.
///
/// We read at most a small prefix to bound IO; a malformed or
/// truncated gitfile is also treated as suspicious (returns `true`)
/// because the redirect intent is what we're refusing to honour.
fn file_is_gitfile(path: &std::path::Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut buf = [0u8; 32];
    let n = match f.read(&mut buf) {
        Ok(n) => n,
        Err(_) => return false,
    };
    let prefix = match std::str::from_utf8(&buf[..n]) {
        Ok(s) => s.trim_start(),
        Err(_) => return false,
    };
    prefix.starts_with("gitdir:")
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

    // ---- DupChildPathValidator ----

    fn pack_with_children(entries: &[(&str, Option<&str>)]) -> PackManifest {
        let children = entries
            .iter()
            .map(|(url, path)| ChildRef {
                url: (*url).to_string(),
                path: path.map(str::to_string),
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

    #[test]
    fn dup_validator_passes_on_distinct_paths() {
        let pack =
            pack_with_children(&[("https://x/a.git", Some("a")), ("https://x/b.git", Some("b"))]);
        assert!(DupChildPathValidator.check(&pack).is_empty());
    }

    #[test]
    fn dup_validator_flags_two_children_at_same_explicit_path() {
        let pack = pack_with_children(&[
            ("https://x/a.git", Some("foo")),
            ("https://y/b.git", Some("foo")),
        ]);
        let errs = DupChildPathValidator.check(&pack);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        match &errs[0] {
            PackValidationError::ChildPathDuplicate { path, urls } => {
                assert_eq!(path, "foo");
                assert_eq!(urls.len(), 2);
                assert!(urls.contains(&"https://x/a.git".to_string()));
                assert!(urls.contains(&"https://y/b.git".to_string()));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn dup_validator_collides_explicit_path_with_url_tail() {
        // `path: foo` collides with a sibling whose URL ends in
        // `/foo.git` and has no explicit path.
        let pack = pack_with_children(&[
            ("https://x/foo.git", None),
            ("https://y/elsewhere.git", Some("foo")),
        ]);
        let errs = DupChildPathValidator.check(&pack);
        assert_eq!(errs.len(), 1, "errs: {errs:?}");
        match &errs[0] {
            PackValidationError::ChildPathDuplicate { path, urls } => {
                assert_eq!(path, "foo");
                assert_eq!(urls.len(), 2);
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn dup_validator_skips_children_with_invalid_path() {
        // One bad path + one good. Dup validator does not flag
        // (the bare-name validator owns the bad-path error).
        let pack = pack_with_children(&[
            ("https://x/a.git", Some("../escape")),
            ("https://x/b.git", Some("good")),
        ]);
        assert!(DupChildPathValidator.check(&pack).is_empty());
    }

    // ---- v1.2.0 Stage 1.c: boundary-preservation new rejects ----
    //
    // These rejects layer ON TOP of the bare-name regex / separator /
    // dot-segment checks owned by `reject_reason`. They surface via
    // `TreeError::ManifestPathEscape` (not `ChildPathInvalid`) so the
    // walker's pre-clone gate distinguishes "literal syntax violation"
    // from "post-resolution boundary escape".
    //
    // See `walker.md` §boundary-preservation; tests here drive the
    // helpers `boundary_reject_reason`, `nfc_duplicate_path`, and
    // `boundary_fs_reject_reason`. The existing `reject_reason` is
    // unchanged (additive layering only — `min-scope` per Stage 1.c).

    #[test]
    fn test_validator_rejects_colon_in_segment() {
        let reason = boundary_reject_reason("child:foo")
            .expect("colon must be rejected as a boundary-preservation hazard");
        assert!(
            reason.to_ascii_lowercase().contains("colon"),
            "reason should mention `colon`: {reason}",
        );
    }

    #[test]
    fn test_validator_rejects_dollar_in_segment() {
        let reason = boundary_reject_reason("$home")
            .expect("dollar must be rejected as a boundary-preservation hazard");
        assert!(
            reason.contains('$') || reason.to_ascii_lowercase().contains("dollar"),
            "reason should mention `$`/dollar: {reason}",
        );
    }

    #[test]
    fn test_validator_rejects_tilde_digit_segment() {
        // `foo~1`, `bar~9`, `x~12` are Windows 8.3 short-name patterns —
        // forbidden because the resolver could collapse two distinct
        // long names onto the same short alias.
        for bad in ["foo~1", "bar~9", "x~12", "abc~3"] {
            let reason = boundary_reject_reason(bad)
                .unwrap_or_else(|| panic!("`{bad}` must be rejected (Windows 8.3 short-name)"));
            assert!(
                reason.contains('~') || reason.to_ascii_lowercase().contains("short"),
                "reason should mention `~`/short-name: {reason}",
            );
        }
    }

    #[test]
    fn test_validator_accepts_tilde_without_digit() {
        // `foo~bar` (tilde NOT followed by a digit) is not a short-name
        // pattern. The boundary check must not over-reach. The bare-name
        // regex still rejects `~` separately, but `boundary_reject_reason`
        // is layered, not replacing — it should return None here.
        assert!(boundary_reject_reason("foo~bar").is_none());
    }

    #[test]
    fn test_validator_rejects_windows_reserved_name_bare() {
        // Case-insensitive: every casing must reject. The list is the
        // Win32 device namespace per MSDN — bare or with extension.
        for variant in ["CON", "con", "Con", "PRN", "prn", "AUX", "NUL", "COM1", "com9", "LPT5"] {
            let reason = boundary_reject_reason(variant)
                .unwrap_or_else(|| panic!("`{variant}` must be rejected (Windows reserved)"));
            assert!(
                reason.to_ascii_lowercase().contains("reserved")
                    || reason.to_ascii_lowercase().contains("windows"),
                "reason should mention `reserved`/`windows` for {variant}: {reason}",
            );
        }
    }

    #[test]
    fn test_validator_rejects_windows_reserved_name_with_ext() {
        // Same list, but with an extension — also reserved per MSDN
        // (Win32 special-cases the stem regardless of suffix).
        for variant in ["con.txt", "CON.TXT", "nul.dat", "lpt1.log", "com3.bak"] {
            let reason = boundary_reject_reason(variant)
                .unwrap_or_else(|| panic!("`{variant}` must be rejected (Windows reserved + ext)"));
            assert!(
                reason.to_ascii_lowercase().contains("reserved")
                    || reason.to_ascii_lowercase().contains("windows"),
                "reason should mention `reserved`/`windows` for {variant}: {reason}",
            );
        }
    }

    #[test]
    fn test_validator_accepts_windows_reserved_name_as_substring() {
        // `concert`, `console`, `comic`, `lpton` etc. embed a reserved
        // stem but are NOT exactly the reserved name — must accept.
        for ok in ["concert", "console", "comic", "lpton", "auxiliary", "nullable"] {
            assert!(
                boundary_reject_reason(ok).is_none(),
                "`{ok}` is a normal name, must NOT be flagged as Windows-reserved",
            );
        }
    }

    #[test]
    fn test_validator_accepts_clean_paths() {
        // Sanity: clean bare names that pass the existing regex must
        // also pass the new boundary check (additive layering — no
        // regression on the happy path).
        for ok in ["foo", "a", "algo-leet", "foo-bar", "foo123", "a1-b2", "pkg-name"] {
            assert!(boundary_reject_reason(ok).is_none(), "`{ok}` should pass boundary check",);
        }
    }

    #[test]
    fn test_validator_rejects_unicode_nfc_duplicate() {
        // `café` exists in two Unicode normal forms:
        //   NFC: "caf\u{00e9}"           (é as a single precomposed code point)
        //   NFD: "cafe\u{0301}"          (e + combining acute accent)
        // On case-insensitive FAT/HFS+/APFS-default filesystems they
        // collapse to the same on-disk name. The validator must reject
        // a sibling pair that differs only by NFC form.
        let nfc = "caf\u{00e9}";
        let nfd = "cafe\u{0301}";
        let children = vec![
            ChildRef {
                url: "https://x/a.git".to_string(),
                path: Some(nfc.to_string()),
                r#ref: None,
            },
            ChildRef {
                url: "https://x/b.git".to_string(),
                path: Some(nfd.to_string()),
                r#ref: None,
            },
        ];
        let dup = nfc_duplicate_path(&children)
            .expect("NFC vs NFD siblings must be flagged as a duplicate");
        // The reported path is one of the two literal forms — either is
        // acceptable; assert it's whichever is the second occurrence (NFD
        // here, because the NFC form lands first and the second collides).
        assert!(
            dup == nfc || dup == nfd,
            "duplicate path must be one of the offending pair, got {dup:?}",
        );
    }

    #[test]
    fn test_validator_accepts_distinct_unicode_paths() {
        // Two distinct names — even with diacritics — must NOT trip the
        // NFC dup detector. `café` (NFC) and `cafe` (plain) are
        // different names regardless of normalization.
        let children = vec![
            ChildRef {
                url: "https://x/a.git".to_string(),
                path: Some("caf\u{00e9}".to_string()),
                r#ref: None,
            },
            ChildRef {
                url: "https://x/b.git".to_string(),
                path: Some("cafe".to_string()),
                r#ref: None,
            },
        ];
        assert!(nfc_duplicate_path(&children).is_none());
    }

    #[test]
    fn test_validator_fs_accepts_nonexistent_path() {
        // FS-based checks defer when the path doesn't exist — clone
        // hasn't fired yet at validation time, so absence is the
        // happy-path signal.
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("not-yet-cloned");
        assert!(boundary_fs_reject_reason(&dest).is_none());
    }

    #[test]
    fn test_validator_fs_accepts_plain_directory() {
        // A regular directory (no `.git`, no junction, no reparse) is
        // fine — the FS check is concerned only with hostile entries.
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("plain-dir");
        std::fs::create_dir(&dest).unwrap();
        assert!(boundary_fs_reject_reason(&dest).is_none());
    }

    #[test]
    fn test_validator_rejects_gitfile_reference() {
        // `.git` as a regular file containing `gitdir: ...` is git's
        // worktree-redirect mechanism. If a child path's `.git` is a
        // file (not a directory), trust is delegated to whatever path
        // the file points at — exactly the boundary escape we forbid.
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("gitfile-child");
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join(".git"), "gitdir: ../elsewhere/.git\n").unwrap();
        let reason = boundary_fs_reject_reason(&dest)
            .expect("gitfile-style `.git` reference must be rejected");
        assert!(
            reason.to_ascii_lowercase().contains("gitfile")
                || reason.to_ascii_lowercase().contains(".git"),
            "reason should mention `.git`/`gitfile`: {reason}",
        );
    }

    #[test]
    fn test_validator_accepts_gitdir_directory() {
        // `.git` AS A DIRECTORY (the normal case for a clone) is fine.
        // We only reject the gitfile-redirect form.
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("normal-clone");
        std::fs::create_dir_all(dest.join(".git")).unwrap();
        assert!(boundary_fs_reject_reason(&dest).is_none());
    }

    /// NTFS junction / Windows reparse-point rejection. Junctions are
    /// reparse points without symlink semantics — `is_symlink()` returns
    /// false on Windows for them, so a dedicated check is required.
    /// The fixture uses the `mklink /J` semantics via
    /// `std::os::windows::fs::symlink_dir` is NOT correct (that creates
    /// a real symlink); junctions are created via the `cmd /c mklink /J`
    /// shell-out — but the test would then depend on the host's `cmd`,
    /// which fights the `no shell=True` invariant. Pragmatic compromise:
    /// the test creates a real Windows symlink_dir (the closest analog
    /// available without spawning `cmd`) and asserts the rejector flags
    /// any reparse point. If symlink creation fails (no Developer Mode),
    /// the test no-ops — the protection is still in place.
    #[cfg(target_os = "windows")]
    #[test]
    fn test_validator_rejects_ntfs_reparse_point() {
        let outer = tempfile::tempdir().unwrap();
        let real = outer.path().join("real-target");
        std::fs::create_dir(&real).unwrap();
        let link = outer.path().join("via-reparse");
        if std::os::windows::fs::symlink_dir(&real, &link).is_err() {
            // Host won't let us create a reparse point — nothing to
            // exercise. The validator is still defended against the
            // attack on hosts that DO allow it.
            return;
        }
        let reason =
            boundary_fs_reject_reason(&link).expect("Windows reparse-point dest must be rejected");
        assert!(
            reason.to_ascii_lowercase().contains("reparse")
                || reason.to_ascii_lowercase().contains("symlink")
                || reason.to_ascii_lowercase().contains("junction"),
            "reason should mention reparse/symlink/junction: {reason}",
        );
    }

    /// Non-Windows stub: confirms the FS rejector compiles and runs
    /// cleanly on platforms where reparse points don't apply. Without
    /// this stub the `cfg(target_os = "windows")` test above would be
    /// the only signal of "this was tested" — and CI on Linux/macOS
    /// would silently skip it. The stub asserts a nonexistent path
    /// passes (the trivial baseline) so CI on any platform records a
    /// signal.
    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_validator_ntfs_reparse_point_stub_non_windows() {
        let outer = tempfile::tempdir().unwrap();
        let dest = outer.path().join("missing");
        assert!(boundary_fs_reject_reason(&dest).is_none());
    }
}
