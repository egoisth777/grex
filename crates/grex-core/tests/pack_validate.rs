//! Integration: plan-phase validator framework + `DuplicateSymlinkValidator`.
//!
//! Covers M3 Stage B slice 2 acceptance items: duplicate `dst` detection
//! (flat, across `when` wrappers, inside a single `when`), non-symlink
//! path cross-checks, validator aggregation, and error-message quality.

#![allow(clippy::too_many_lines)]

use grex_core::pack::{parse, validate::Validator, PackManifest, PackValidationError};

// ---------- helpers ----------

fn assert_dup(
    err: &PackValidationError,
    expect_dst: &str,
    expect_first: usize,
    expect_second: usize,
) {
    match err {
        PackValidationError::DuplicateSymlinkDst { dst, first, second } => {
            assert_eq!(dst, expect_dst, "dst mismatch in {err:?}");
            assert_eq!(*first, expect_first, "first index mismatch in {err:?}");
            assert_eq!(*second, expect_second, "second index mismatch in {err:?}");
        }
        other => panic!("expected DuplicateSymlinkDst, got {other:?}"),
    }
}

// ---------- positive cases ----------

#[test]
fn no_symlinks_ok() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - mkdir: { path: /tmp/x }
  - env: { name: FOO, value: bar }
";
    let pack = parse(yaml).unwrap();
    pack.validate_plan().expect("no symlinks => no validation errors");
}

#[test]
fn distinct_dsts_ok() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: /tmp/a }
  - symlink: { src: b, dst: /tmp/b }
  - symlink: { src: c, dst: /tmp/c }
";
    let pack = parse(yaml).unwrap();
    pack.validate_plan().expect("distinct dsts must pass");
}

#[test]
fn non_symlink_same_path_ignored() {
    // mkdir.path and symlink.dst share the same string — validator only
    // cross-checks symlinks against each other, so this must pass.
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - mkdir: { path: /x }
  - symlink: { src: s, dst: /x }
";
    let pack = parse(yaml).unwrap();
    pack.validate_plan().expect("mkdir.path vs symlink.dst must not collide");
}

// ---------- duplicate detection ----------

#[test]
fn dup_dst_flat_detected() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: /tmp/dup }
  - symlink: { src: b, dst: /tmp/dup }
";
    let pack = parse(yaml).unwrap();
    let errs = pack.validate_plan().expect_err("duplicate must be flagged");
    assert_eq!(errs.len(), 1, "one pair => one error, got {errs:?}");
    assert_dup(&errs[0], "/tmp/dup", 0, 1);
}

#[test]
fn dup_dst_three_way() {
    // 3 symlinks sharing one dst => C(3,2) = 3 unordered pairs.
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: /tmp/dup }
  - symlink: { src: b, dst: /tmp/dup }
  - symlink: { src: c, dst: /tmp/dup }
";
    let pack = parse(yaml).unwrap();
    let errs = pack.validate_plan().expect_err("3-way dup must flag");
    assert_eq!(errs.len(), 3, "C(3,2)=3 pairs expected, got {errs:?}");
    assert_dup(&errs[0], "/tmp/dup", 0, 1);
    assert_dup(&errs[1], "/tmp/dup", 0, 2);
    assert_dup(&errs[2], "/tmp/dup", 1, 2);
}

#[test]
fn dup_dst_across_when_wrapper() {
    // One top-level symlink (global index 0) + one inside a `when` block
    // (global index 1). Indices come from the flattened walk.
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: /tmp/shared }
  - when:
      os: windows
      actions:
        - symlink: { src: b, dst: /tmp/shared }
";
    let pack = parse(yaml).unwrap();
    let errs = pack.validate_plan().expect_err("cross-when dup must flag");
    assert_eq!(errs.len(), 1);
    assert_dup(&errs[0], "/tmp/shared", 0, 1);
}

#[test]
fn dup_inside_same_when() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - when:
      os: linux
      actions:
        - symlink: { src: a, dst: /tmp/inner }
        - symlink: { src: b, dst: /tmp/inner }
";
    let pack = parse(yaml).unwrap();
    let errs = pack.validate_plan().expect_err("within-when dup must flag");
    assert_eq!(errs.len(), 1);
    assert_dup(&errs[0], "/tmp/inner", 0, 1);
}

// ---------- diagnostic quality ----------

#[test]
fn validator_error_message_quality() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: /tmp/same }
  - symlink: { src: b, dst: /tmp/same }
";
    let pack = parse(yaml).unwrap();
    let errs = pack.validate_plan().expect_err("dup expected");
    let msg = errs[0].to_string();
    assert!(msg.contains("/tmp/same"), "message must cite dst, got {msg:?}");
    assert!(msg.contains('0') && msg.contains('1'), "message must cite both indices, got {msg:?}");
    assert!(msg.contains("duplicate"), "message must be self-describing, got {msg:?}");
}

// ---------- framework plumbing ----------

/// Trivial second validator proving [`run_all`]'s aggregation point is
/// trait-based. Not added to the default set — invoked manually here to
/// assert that plugging in new validators does not require touching
/// [`PackManifest`] or parse logic.
struct AlwaysFailsValidator;

impl Validator for AlwaysFailsValidator {
    fn name(&self) -> &'static str {
        "always_fails_test_only"
    }

    fn check(&self, _pack: &PackManifest) -> Vec<PackValidationError> {
        vec![PackValidationError::DuplicateSymlinkDst {
            dst: "<sentinel>".to_string(),
            first: 99,
            second: 100,
        }]
    }
}

#[test]
fn multiple_validators_future_proofing() {
    // Manually compose the default + a test validator. This mirrors what
    // a future slice will do when registering its own check: it only
    // needs the `Validator` trait — nothing else in the crate changes.
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: /tmp/dup }
  - symlink: { src: b, dst: /tmp/dup }
";
    let pack = parse(yaml).unwrap();

    let default_errs = grex_core::pack::run_all(&pack);
    let extra = AlwaysFailsValidator.check(&pack);
    let mut combined = default_errs.clone();
    combined.extend(extra.clone());

    assert_eq!(default_errs.len(), 1, "default set surfaces the dup");
    assert_eq!(extra.len(), 1, "test validator surfaces its sentinel");
    assert_eq!(combined.len(), 2, "aggregation concatenates");
    // Name is part of the diagnostic surface.
    assert_eq!(AlwaysFailsValidator.name(), "always_fails_test_only");
}
