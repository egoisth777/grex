//! Content hash of a pack's declarative action list.
//!
//! The lockfile's `actions_hash` field pins the planned `actions:` against
//! the commit that produced them, so `grex sync` can short-circuit a pack
//! whose inputs are byte-identical to the last successful install.
//!
//! Framing: `b"grex-actions-v1\0" || canonical(actions) || b"\0" ||
//! commit_sha.as_bytes()`, digested with SHA-256 and rendered as lowercase
//! hex. The `grex-actions-v1` prefix domain-separates this hash from any
//! other SHA-256 we might emit.
//!
//! The outer [`Action`] enum is **not** `Serialize` (the Tier-1 variant set
//! grows via plugins in M4; a derived `Serialize` would couple the
//! canonical form to that enum's private shape). Instead, each action is
//! canonicalised as `name || "\0" || json(args) || 0x1e`, where the `args`
//! form is `serde_json::to_vec` over the per-variant args struct. The
//! predicate-carrying variants (`require`, `when`) now derive `Serialize`
//! on their args structs too, so the same JSON path is used for every
//! variant — no `Debug` fallback, no version-sensitive canonicalisation.
//!
//! `when` nests a `Vec<Action>` whose element type is not itself
//! `Serialize`. We handle that by serialising nested actions through the
//! same `{ name, args }` canonical shape used at the top level, so the
//! canonical form is fully recursive and self-consistent.
//!
//! Versioning: the salt is `grex-actions-v1`. Any breaking change to the
//! canonical form MUST bump this to `-v2` (see `golden_hash_v1_stability`).

use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::pack::{Action, WhenSpec};

/// Compute the content hash of a pack's resolved action list.
///
/// Returns a 64-char lowercase hex SHA-256 digest.
#[must_use]
pub fn compute_actions_hash(actions: &[Action], commit_sha: &str) -> String {
    let mut canonical: Vec<u8> = Vec::new();
    for a in actions {
        append_canonical_action(&mut canonical, a);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"grex-actions-v1\0");
    hasher.update(&canonical);
    hasher.update(b"\0");
    hasher.update(commit_sha.as_bytes());
    to_hex_lower(&hasher.finalize())
}

/// Append the canonical `name || "\0" || json(args) || 0x1e` framing for
/// one action. Uniform across every variant — no Debug fallback.
fn append_canonical_action(out: &mut Vec<u8>, a: &Action) {
    out.extend_from_slice(a.name().as_bytes());
    out.push(0);
    out.extend_from_slice(&action_args_bytes(a));
    out.push(0x1e);
}

/// Canonical JSON bytes of an action's arguments. Uses `serde_json` for
/// every variant; `when` routes through a local helper because its
/// `actions` field holds `Vec<Action>` whose element type is not
/// `Serialize`.
fn action_args_bytes(a: &Action) -> Vec<u8> {
    match a {
        Action::Symlink(s) => to_json_bytes(s, "SymlinkArgs"),
        Action::Env(e) => to_json_bytes(e, "EnvArgs"),
        Action::Mkdir(m) => to_json_bytes(m, "MkdirArgs"),
        Action::Rmdir(r) => to_json_bytes(r, "RmdirArgs"),
        Action::Exec(x) => to_json_bytes(x, "ExecSpec"),
        Action::Require(r) => to_json_bytes(r, "RequireSpec"),
        Action::When(w) => when_spec_json_bytes(w),
    }
}

/// Helper — `serde_json::to_vec` with a consistent panic message.
fn to_json_bytes<T: Serialize>(v: &T, label: &'static str) -> Vec<u8> {
    serde_json::to_vec(v).unwrap_or_else(|e| panic!("{label} Serialize: {e}"))
}

/// Canonical JSON for a [`WhenSpec`]. The `actions` vec is rendered as a
/// JSON array of `{ "name": ..., "args": <args-as-json> }` objects,
/// recursively — matching the top-level `name || args` framing in JSON
/// shape. Keys are emitted in a fixed order via `json!` so output is
/// stable.
fn when_spec_json_bytes(w: &WhenSpec) -> Vec<u8> {
    let actions: Vec<Value> = w.actions.iter().map(action_to_json).collect();
    let body = json!({
        "os": w.os,
        "all_of": w.all_of,
        "any_of": w.any_of,
        "none_of": w.none_of,
        "actions": actions,
    });
    serde_json::to_vec(&body).expect("WhenSpec canonical JSON")
}

/// Convert a single action to its canonical `{ name, args }` JSON form,
/// recursing through `when.actions`.
fn action_to_json(a: &Action) -> Value {
    let args = match a {
        Action::Symlink(s) => serde_json::to_value(s).expect("SymlinkArgs to_value"),
        Action::Env(e) => serde_json::to_value(e).expect("EnvArgs to_value"),
        Action::Mkdir(m) => serde_json::to_value(m).expect("MkdirArgs to_value"),
        Action::Rmdir(r) => serde_json::to_value(r).expect("RmdirArgs to_value"),
        Action::Exec(x) => serde_json::to_value(x).expect("ExecSpec to_value"),
        Action::Require(r) => serde_json::to_value(r).expect("RequireSpec to_value"),
        Action::When(w) => {
            let nested: Vec<Value> = w.actions.iter().map(action_to_json).collect();
            json!({
                "os": w.os,
                "all_of": w.all_of,
                "any_of": w.any_of,
                "none_of": w.none_of,
                "actions": nested,
            })
        }
    };
    json!({ "name": a.name(), "args": args })
}

/// Lowercase hex encoding of a byte slice. Inline to avoid pulling the
/// `hex` crate for one call site.
fn to_hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pack::{
        Combiner, MkdirArgs, OsKind, Predicate, RequireOnFail, RequireSpec, SymlinkArgs,
        SymlinkKind,
    };

    fn fixture_actions() -> Vec<Action> {
        vec![
            Action::Mkdir(MkdirArgs::new("~/.config/grex".into(), None)),
            Action::Symlink(SymlinkArgs::new(
                "dotfiles/vimrc".into(),
                "~/.vimrc".into(),
                false,
                true,
                SymlinkKind::Auto,
            )),
        ]
    }

    /// Fixture with a `require:` block whose combiner holds a nested
    /// `all_of` — exercises the predicate tree that previously went
    /// through the `Debug` fallback path.
    fn fixture_actions_with_require() -> Vec<Action> {
        let inner = Combiner::AllOf(vec![
            Predicate::PathExists("/usr/bin/git".into()),
            Predicate::CmdAvailable("git".into()),
        ]);
        vec![
            Action::Mkdir(MkdirArgs::new("~/.config/grex".into(), None)),
            Action::Require(RequireSpec::new(inner, RequireOnFail::Error)),
        ]
    }

    #[test]
    fn hash_is_stable_across_invocations() {
        let a = fixture_actions();
        let h1 = compute_actions_hash(&a, "deadbeef");
        let h2 = compute_actions_hash(&a, "deadbeef");
        assert_eq!(h1, h2);
    }

    #[test]
    fn hash_changes_on_reorder() {
        let mut a = fixture_actions();
        let h1 = compute_actions_hash(&a, "deadbeef");
        a.reverse();
        let h2 = compute_actions_hash(&a, "deadbeef");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_changes_on_commit_sha() {
        let a = fixture_actions();
        let h1 = compute_actions_hash(&a, "deadbeef");
        let h2 = compute_actions_hash(&a, "cafef00d");
        assert_ne!(h1, h2);
    }

    #[test]
    fn hash_is_64_lowercase_hex() {
        let h = compute_actions_hash(&fixture_actions(), "deadbeef");
        assert_eq!(h.len(), 64);
        assert!(h.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    }

    #[test]
    fn empty_actions_still_hashes() {
        // Purely defensive: zero-action packs must produce a stable,
        // well-formed hash rather than panicking or producing "".
        let h = compute_actions_hash(&[], "deadbeef");
        assert_eq!(h.len(), 64);
    }

    /// Golden digest. Pins the exact SHA-256 output for a known fixture
    /// under the `grex-actions-v1` salt. Any change to the canonical form
    /// MUST either preserve this digest or bump the salt to `-v2`.
    #[test]
    fn golden_hash_v1_stability() {
        let a = fixture_actions_with_require();
        let h = compute_actions_hash(&a, "deadbeef");
        assert_eq!(
            h, GOLDEN_HASH_V1,
            "actions_hash v1 canonical form changed; if intentional, bump the salt to -v2",
        );
    }

    /// Pin for [`fixture_actions_with_require`] + commit `deadbeef` under
    /// salt `grex-actions-v1`. Generated by running the hash fn once and
    /// copying the hex output.
    const GOLDEN_HASH_V1: &str = "4c85d79c2f49b4336a4bad06b221a802a3415d7f33400f7e96261eb87fa409c9";

    /// Reordering predicates inside a Combiner MUST change the hash —
    /// order is semantically meaningful (short-circuit evaluation order,
    /// author intent). The previous Debug-fallback path happened to
    /// preserve this, but without a test there was no guarantee.
    #[test]
    fn require_reorder_changes_hash() {
        let c1 = Combiner::AllOf(vec![
            Predicate::PathExists("/a".into()),
            Predicate::CmdAvailable("b".into()),
        ]);
        let c2 = Combiner::AllOf(vec![
            Predicate::CmdAvailable("b".into()),
            Predicate::PathExists("/a".into()),
        ]);
        let a1 = vec![Action::Require(RequireSpec::new(c1, RequireOnFail::Error))];
        let a2 = vec![Action::Require(RequireSpec::new(c2, RequireOnFail::Error))];
        let h1 = compute_actions_hash(&a1, "deadbeef");
        let h2 = compute_actions_hash(&a2, "deadbeef");
        assert_ne!(h1, h2, "predicate order must affect hash");
    }

    /// Two identical predicate trees MUST produce the same hash.
    /// Guards against nondeterministic serialisation (e.g. if someone
    /// swaps a HashMap into the predicate tree without thinking).
    #[test]
    fn require_equal_semantics_equal_hash() {
        let build = || {
            Combiner::AnyOf(vec![
                Predicate::Os(OsKind::Linux),
                Predicate::PathExists("/etc/hostname".into()),
            ])
        };
        let a1 = vec![Action::Require(RequireSpec::new(build(), RequireOnFail::Warn))];
        let a2 = vec![Action::Require(RequireSpec::new(build(), RequireOnFail::Warn))];
        let h1 = compute_actions_hash(&a1, "deadbeef");
        let h2 = compute_actions_hash(&a2, "deadbeef");
        assert_eq!(h1, h2, "identical predicate trees must hash equally");
    }
}
