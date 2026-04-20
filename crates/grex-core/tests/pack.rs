//! Integration: `grex_core::pack::parse` round-trip, validation, and error
//! coverage for M3 Stage A. Each test maps onto one acceptance item in
//! `openspec/feat-grex/spec.md`.

#![allow(clippy::too_many_lines)]

use grex_core::pack::{
    parse, Action, ChildRef, Combiner, EnvScope, ExecOnFail, OsKind, PackParseError, PackType,
    Predicate, RequireOnFail, SymlinkKind, MAX_REQUIRE_DEPTH,
};

// ---------- fixtures ----------

const F_DECLARATIVE: &str = include_str!("fixtures/pack-declarative.yaml");
const F_META: &str = include_str!("fixtures/pack-meta.yaml");
const F_SCRIPTED: &str = include_str!("fixtures/pack-scripted.yaml");
const F_REQUIRE_NESTED: &str = include_str!("fixtures/pack-require-nested.yaml");
const F_EXEC_SHELL: &str = include_str!("fixtures/pack-exec-shell.yaml");

// ---------- round-trip + meta ----------

#[test]
fn parse_declarative_round_trip() {
    let pack = parse(F_DECLARATIVE).expect("declarative fixture must parse");
    assert_eq!(pack.schema_version.as_str(), "1");
    assert_eq!(pack.name, "warp-cfg");
    assert_eq!(pack.r#type, PackType::Declarative);
    assert_eq!(pack.version.as_deref(), Some("0.2.0"));
    assert!(pack.children.is_empty());
    assert_eq!(pack.actions.len(), 3);
    matches!(pack.actions[0], Action::Require(_));
    matches!(pack.actions[1], Action::When(_));
    matches!(pack.actions[2], Action::Exec(_));
    let teardown = pack.teardown.as_ref().expect("explicit teardown expected");
    assert_eq!(teardown.len(), 1);
    matches!(teardown[0], Action::Rmdir(_));
}

#[test]
fn parse_meta_children_effective_path() {
    let pack = parse(F_META).expect("meta fixture must parse");
    assert_eq!(pack.r#type, PackType::Meta);
    assert_eq!(pack.children.len(), 3);
    // `.git` suffix stripped for default.
    assert_eq!(pack.children[0].effective_path(), "foo");
    assert!(pack.children[0].path.is_none());
    // Explicit override wins.
    assert_eq!(pack.children[1].effective_path(), "bar-override");
    assert_eq!(pack.children[1].r#ref.as_deref(), Some("v1.2.0"));
    // HTTPS URL with no `.git` suffix and no override.
    assert_eq!(pack.children[2].effective_path(), "baz");
}

#[test]
fn parse_scripted_minimal() {
    let pack = parse(F_SCRIPTED).expect("scripted fixture must parse");
    assert_eq!(pack.r#type, PackType::Scripted);
    assert!(pack.actions.is_empty());
    assert!(pack.children.is_empty());
    assert!(pack.teardown.is_none());
}

// ---------- schema_version ----------

#[test]
fn schema_version_must_be_quoted_string_one() {
    assert!(parse("schema_version: \"1\"\nname: ok\ntype: meta\n").is_ok());
    let err = parse("schema_version: 1\nname: ok\ntype: meta\n").unwrap_err();
    // Stringified bare int fails via the custom Deserialize → serde_yaml::Error.
    assert!(
        matches!(err, PackParseError::Inner(_)),
        "bare int must fail at SchemaVersion deserialize, got {err:?}"
    );
    let err = parse("schema_version: \"2\"\nname: ok\ntype: meta\n").unwrap_err();
    assert!(matches!(err, PackParseError::InvalidSchemaVersion { .. }), "{err:?}");
}

// ---------- name regex ----------

#[test]
fn invalid_name_rejected() {
    // Per pack-spec.md §Validation, the regex is ^[a-z][a-z0-9-]*$:
    // the first character must be a lowercase letter (digits allowed only
    // in later positions).
    for bad in ["NAME", "with_underscore", "", "-leading", "has space", "123-bad", "9to5"] {
        let yaml = format!("schema_version: \"1\"\nname: {bad:?}\ntype: meta\n");
        let err = parse(&yaml).unwrap_err();
        assert!(matches!(err, PackParseError::InvalidName { .. }), "name {bad:?} must fail");
    }
    // Positive controls: each exercises a boundary of the regex.
    for good in ["ok", "a", "warp-cfg", "a9", "pack-2"] {
        let yaml = format!("schema_version: \"1\"\nname: {good}\ntype: meta\n");
        parse(&yaml).unwrap_or_else(|e| panic!("good name {good:?} must parse, got {e:?}"));
    }
}

// ---------- empty collections ----------

#[test]
fn empty_actions_valid() {
    let yaml = "schema_version: \"1\"\nname: ok\ntype: declarative\nactions: []\n";
    let pack = parse(yaml).expect("empty actions must parse");
    assert!(pack.actions.is_empty());
}

#[test]
fn empty_children_valid() {
    let yaml = "schema_version: \"1\"\nname: ok\ntype: meta\nchildren: []\n";
    let pack = parse(yaml).expect("empty children must parse");
    assert!(pack.children.is_empty());
}

#[test]
fn teardown_omitted_vs_empty() {
    let omitted =
        parse("schema_version: \"1\"\nname: ok\ntype: declarative\n").expect("omitted teardown");
    assert!(omitted.teardown.is_none());
    let empty =
        parse("schema_version: \"1\"\nname: ok\ntype: declarative\nteardown: []\n").unwrap();
    assert_eq!(empty.teardown.as_deref(), Some(&[][..]));
}

// ---------- ordering + serialize ----------

#[test]
fn action_order_preserved() {
    let pack = parse(F_DECLARATIVE).unwrap();
    assert!(matches!(pack.actions[0], Action::Require(_)));
    assert!(matches!(pack.actions[1], Action::When(_)));
    assert!(matches!(pack.actions[2], Action::Exec(_)));
}

// ---------- YAML anchor/alias rejection ----------

#[test]
fn yaml_anchors_rejected() {
    let yaml = "
schema_version: \"1\"
name: ok
type: declarative
actions:
  - &sym
    symlink: { src: a, dst: b }
  - *sym
";
    let err = parse(yaml).unwrap_err();
    assert!(matches!(err, PackParseError::YamlAliasRejected), "{err:?}");
}

// ---------- unknown top-level ----------

#[test]
fn unknown_top_level_key_x_prefix_allowed() {
    let yaml = "schema_version: \"1\"\nname: ok\ntype: meta\nx-custom: whatever\n";
    let pack = parse(yaml).expect("x-* must be accepted");
    assert!(pack.extensions.contains_key("x-custom"));
    let yaml_bad = "schema_version: \"1\"\nname: ok\ntype: meta\ncustom: whatever\n";
    let err = parse(yaml_bad).unwrap_err();
    assert!(matches!(err, PackParseError::UnknownTopLevelKey { ref key } if key == "custom"));
}

// ---------- action-entry shape errors ----------

#[test]
fn unknown_action_key_rejected() {
    let yaml = "schema_version: \"1\"\nname: ok\ntype: declarative\nactions:\n  - notavar: {}\n";
    let err = parse(yaml).unwrap_err();
    match err {
        PackParseError::UnknownActionKey { key } => assert_eq!(key, "notavar"),
        other => panic!("expected UnknownActionKey, got {other:?}"),
    }
}

#[test]
fn empty_action_entry_rejected() {
    let yaml = "schema_version: \"1\"\nname: ok\ntype: declarative\nactions:\n  - {}\n";
    let err = parse(yaml).unwrap_err();
    assert!(matches!(err, PackParseError::EmptyActionEntry), "{err:?}");
}

#[test]
fn multiple_action_keys_rejected() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: a, dst: b }
    env: { name: N, value: V }
";
    let err = parse(yaml).unwrap_err();
    match err {
        PackParseError::MultipleActionKeys { keys } => {
            assert!(keys.contains(&"symlink".to_string()));
            assert!(keys.contains(&"env".to_string()));
        }
        other => panic!("expected MultipleActionKeys, got {other:?}"),
    }
}

// ---------- exec XOR ----------

#[test]
fn exec_cmd_shell_mutex_false() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - exec:
      shell: false
      cmd_shell: \"oops\"
";
    let err = parse(yaml).unwrap_err();
    assert!(matches!(err, PackParseError::ExecCmdMutex { shell: false, .. }), "{err:?}");
}

#[test]
fn exec_cmd_shell_mutex_true() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - exec:
      shell: true
      cmd: [\"echo\", \"x\"]
";
    let err = parse(yaml).unwrap_err();
    assert!(matches!(err, PackParseError::ExecCmdMutex { shell: true, .. }), "{err:?}");
}

#[test]
fn exec_cmd_shell_positive_both_paths() {
    // Array form.
    let array = parse(
        "schema_version: \"1\"
name: ok
type: declarative
actions:
  - exec: { cmd: [\"git\", \"status\"], shell: false }
",
    )
    .unwrap();
    match &array.actions[0] {
        Action::Exec(e) => {
            assert!(!e.shell);
            assert_eq!(e.cmd.as_ref().unwrap().len(), 2);
            assert!(e.cmd_shell.is_none());
        }
        _ => panic!("expected exec"),
    }
    // Shell form fixture.
    let shell = parse(F_EXEC_SHELL).unwrap();
    match &shell.actions[0] {
        Action::Exec(e) => {
            assert!(e.shell);
            assert_eq!(e.cmd_shell.as_deref(), Some("echo hello world"));
            assert_eq!(e.on_fail, ExecOnFail::Ignore);
        }
        _ => panic!("expected exec"),
    }
}

// ---------- on_fail set split (BUG 1) ----------

#[test]
fn require_on_fail_skip_accepted() {
    let ok = parse(
        "schema_version: \"1\"
name: ok
type: declarative
actions:
  - require:
      all_of: [{ os: windows }]
      on_fail: skip
",
    )
    .unwrap();
    match &ok.actions[0] {
        Action::Require(r) => assert_eq!(r.on_fail, RequireOnFail::Skip),
        _ => panic!("expected require"),
    }
    let err = parse(
        "schema_version: \"1\"
name: ok
type: declarative
actions:
  - require:
      all_of: [{ os: windows }]
      on_fail: ignore
",
    )
    .unwrap_err();
    assert!(matches!(err, PackParseError::Inner(_)), "ignore must reject on require: {err:?}");
}

#[test]
fn exec_on_fail_ignore_accepted() {
    let ok = parse(F_EXEC_SHELL).unwrap();
    match &ok.actions[0] {
        Action::Exec(e) => assert_eq!(e.on_fail, ExecOnFail::Ignore),
        _ => panic!("expected exec"),
    }
    let err = parse(
        "schema_version: \"1\"
name: ok
type: declarative
actions:
  - exec:
      shell: true
      cmd_shell: \"ls\"
      on_fail: skip
",
    )
    .unwrap_err();
    assert!(matches!(err, PackParseError::Inner(_)), "skip must reject on exec: {err:?}");
}

// ---------- predicate nesting ----------

#[test]
fn require_nested_depth2() {
    let pack = parse(F_REQUIRE_NESTED).expect("nested require fixture must parse");
    let Action::Require(req) = &pack.actions[0] else {
        panic!("expected require action");
    };
    assert_eq!(req.on_fail, RequireOnFail::Warn);
    let Combiner::AllOf(outer) = &req.combiner else {
        panic!("expected all_of top combiner");
    };
    // Outer has 2 preds: a nested any_of and os=windows.
    assert_eq!(outer.len(), 2);
    let Predicate::AnyOf(inner) = &outer[0] else {
        panic!("expected any_of nested");
    };
    // Inner any_of has cmd_available and a nested none_of (depth 2 reachable).
    assert_eq!(inner.len(), 2);
    assert!(matches!(inner[0], Predicate::CmdAvailable(_)));
    assert!(matches!(inner[1], Predicate::NoneOf(_)));
    assert!(matches!(outer[1], Predicate::Os(OsKind::Windows)));
}

#[test]
fn require_depth_exceeded() {
    // Build a predicate tree where the top-level combiner already sits at
    // depth 1 and subsequent all_of nests dig deeper. We want the inner
    // predicate depth to exceed MAX_REQUIRE_DEPTH (32).
    let inner_leaf = "{ os: windows }";
    let mut nested = inner_leaf.to_string();
    for _ in 0..(MAX_REQUIRE_DEPTH + 2) {
        nested = format!("{{ all_of: [{nested}] }}");
    }
    let yaml = format!(
        "schema_version: \"1\"
name: ok
type: declarative
actions:
  - require:
      all_of: [{nested}]
",
    );
    let err = parse(&yaml).unwrap_err();
    assert!(
        matches!(err, PackParseError::RequireDepthExceeded { .. }),
        "expected RequireDepthExceeded, got {err:?}"
    );
}

// ---------- reg_key BUG 3 ----------

#[test]
fn reg_key_legacy_form_rejected() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - require:
      all_of:
        - reg_key: \"HKLM\\\\Software\\\\X!Val\"
      on_fail: error
";
    let err = parse(yaml).unwrap_err();
    match err {
        PackParseError::InvalidPredicate { detail } => {
            assert!(detail.contains("reg_key"), "error should cite reg_key, got {detail:?}");
        }
        other => panic!("expected InvalidPredicate, got {other:?}"),
    }
}

#[test]
fn reg_key_map_form_accepted() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - require:
      all_of:
        - reg_key: { path: \"HKLM\\\\Software\\\\X\", name: \"Val\" }
      on_fail: error
";
    let pack = parse(yaml).unwrap();
    let Action::Require(req) = &pack.actions[0] else { panic!("require") };
    let Combiner::AllOf(preds) = &req.combiner else { panic!("all_of") };
    match &preds[0] {
        Predicate::RegKey { path, name } => {
            assert_eq!(path, "HKLM\\Software\\X");
            assert_eq!(name.as_deref(), Some("Val"));
        }
        other => panic!("expected RegKey, got {other:?}"),
    }
}

// ---------- action default fields ----------

#[test]
fn symlink_defaults_applied() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { src: files/a, dst: \"$HOME/a\" }
";
    let pack = parse(yaml).unwrap();
    let Action::Symlink(s) = &pack.actions[0] else { panic!("symlink") };
    assert!(!s.backup);
    assert!(s.normalize);
    assert_eq!(s.kind, SymlinkKind::Auto);
}

#[test]
fn env_defaults_applied() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - env: { name: FOO, value: bar }
";
    let pack = parse(yaml).unwrap();
    let Action::Env(e) = &pack.actions[0] else { panic!("env") };
    assert_eq!(e.scope, EnvScope::User);
}

// ---------- missing required ----------

#[test]
fn missing_required_field_src_in_symlink() {
    let yaml = "schema_version: \"1\"
name: ok
type: declarative
actions:
  - symlink: { dst: \"$HOME/a\" }
";
    let err = parse(yaml).unwrap_err();
    match err {
        PackParseError::Inner(e) => {
            let msg = e.to_string();
            assert!(msg.contains("src"), "error should cite `src` field, got {msg:?}");
        }
        other => panic!("expected serde inner error, got {other:?}"),
    }
}

// ---------- pack_type surface ----------

#[test]
fn pack_type_variants_all_parse() {
    for (ty, expect) in [
        ("meta", PackType::Meta),
        ("declarative", PackType::Declarative),
        ("scripted", PackType::Scripted),
    ] {
        let yaml = format!("schema_version: \"1\"\nname: ok\ntype: {ty}\n");
        let pack = parse(&yaml).unwrap_or_else(|e| panic!("type {ty} must parse: {e:?}"));
        assert_eq!(pack.r#type, expect);
    }
}

#[test]
fn pack_type_uppercase_rejected() {
    let err = parse("schema_version: \"1\"\nname: ok\ntype: Meta\n").unwrap_err();
    assert!(matches!(err, PackParseError::Inner(_)), "{err:?}");
}

// ---------- round-trip via re-serialize ----------

#[test]
fn child_ref_round_trip() {
    // Separate from the whole-manifest round-trip: ChildRef is the one
    // shape we round-trip structurally (manifest has parse-only top-level).
    let pack = parse(F_META).unwrap();
    let s = serde_yaml::to_string(&pack.children).unwrap();
    let back: Vec<ChildRef> = serde_yaml::from_str(&s).unwrap();
    assert_eq!(back, pack.children);
}
