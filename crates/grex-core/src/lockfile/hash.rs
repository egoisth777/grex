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
//! The outer [`Action`] enum is **not** `Serialize`; only its variant-args
//! structs are. Each action is therefore canonicalised as
//! `name || "\0" || json(args) || 0x1e`. The two predicate-carrying
//! variants (`require`, `when`) have no pure serde path and fall back to
//! `Debug` output — deterministic for the in-memory layout produced by
//! the YAML parser, which is what we actually hash over.

use sha2::{Digest, Sha256};

use crate::pack::Action;

/// Compute the content hash of a pack's resolved action list.
///
/// Returns a 64-char lowercase hex SHA-256 digest.
#[must_use]
pub fn compute_actions_hash(actions: &[Action], commit_sha: &str) -> String {
    let mut canonical: Vec<u8> = Vec::new();
    for a in actions {
        canonical.extend_from_slice(a.name().as_bytes());
        canonical.push(0);
        canonical.extend_from_slice(&action_args_bytes(a));
        canonical.push(0x1e);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"grex-actions-v1\0");
    hasher.update(&canonical);
    hasher.update(b"\0");
    hasher.update(commit_sha.as_bytes());
    to_hex_lower(&hasher.finalize())
}

/// Canonical byte form of an action's arguments. Variants whose args
/// structs derive `Serialize` use JSON; `require` / `when` fall back to
/// `Debug` (their predicate trees are not `Serialize`).
fn action_args_bytes(a: &Action) -> Vec<u8> {
    match a {
        Action::Symlink(s) => serde_json::to_vec(s).expect("SymlinkArgs Serialize"),
        Action::Env(e) => serde_json::to_vec(e).expect("EnvArgs Serialize"),
        Action::Mkdir(m) => serde_json::to_vec(m).expect("MkdirArgs Serialize"),
        Action::Rmdir(r) => serde_json::to_vec(r).expect("RmdirArgs Serialize"),
        Action::Exec(x) => serde_json::to_vec(x).expect("ExecSpec Serialize"),
        Action::Require(r) => format!("{r:?}").into_bytes(),
        Action::When(w) => format!("{w:?}").into_bytes(),
    }
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
    use crate::pack::{MkdirArgs, SymlinkArgs, SymlinkKind};

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
}
