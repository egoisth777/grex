//! v1.3.3 B10 — `--ref <git-ref>` parsing, folder-FA, and collision-extend
//! resolver.
//!
//! Mirrors the Lean4 spec at `proof/Grex/Ref.lean` + `proof/Grex/Types.lean`
//! (namespace `Grex.Ref`). The 8-cell finite-automaton transition table
//! (`ref_fa`), the `<refdir>` encoding (`encode_refdir`), and the
//! collision-extend invariant (OQ5: `encodeRefdir_distinct`) are reproduced
//! here as Rust functions.
//!
//! Public surface:
//! * [`Ref`] — parsed `--ref` token (mirrors Lean `RefInput`
//!   minus the dynamic `urlTracked` / `dupHit`
//!   axes, which come from the parent manifest).
//! * [`RefAction`] — five-tag FA action (mirrors Lean `RefAction`).
//! * [`AddContext`] — parent-manifest probe (URL tracked? same
//!   `(branch, commit)` already?).
//! * [`parse_ref`] — parse the `--ref <token>` string. Accepts
//!   `main`, `a3f9c1d` (7..40 hex chars), or
//!   `main@a3f9c1d`.
//! * [`encode_refdir`] — `<refdir>` folder name from a `Ref` (default
//!   7-char SHA prefix).
//! * [`encode_refdir_with_prefix`] — same as `encode_refdir` but with a
//!   caller-controlled SHA prefix length (used by
//!   the collision-extend resolver).
//! * [`resolve_unique_refdir`] — collision-extend loop. Iteratively
//!   extends the SHA prefix until the resulting
//!   folder name does not appear in `existing`.
//! * [`classify_ref_input`] — drive the 8-cell FA. Returns the
//!   `RefAction` for a `(Ref, AddContext)` pair.

use std::collections::HashSet;

/// Parsed `--ref <git-ref>` token. Mirrors `Grex.Ref.RefInput` (Lean) on
/// the static axes (branch + commit). The dynamic axes (`urlTracked`,
/// `dupHit`) live on [`AddContext`] because they come from probing the
/// parent manifest, not from parsing the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ref {
    /// Branch name as written in `--ref`, e.g. `"main"` or
    /// `"feature/foo"`. `None` when the user wrote a bare commit token.
    pub branch: Option<String>,
    /// Commit SHA (7..40 hex chars) as written in `--ref`. `None` when
    /// the user wrote a bare branch token. `Some(...)` is also produced
    /// when the user wrote `main@a3f9c1d`.
    pub commit: Option<String>,
}

/// Errors from [`parse_ref`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RefParseError {
    /// The token was empty or only whitespace.
    #[error("--ref value is empty or whitespace-only")]
    Empty,
    /// Multiple `@` separators.
    #[error("--ref value contains more than one `@` separator")]
    MultipleAt,
    /// Empty branch component before `@`.
    #[error("--ref value has empty branch component before `@`")]
    EmptyBranch,
    /// Empty commit component after `@`.
    #[error("--ref value has empty commit component after `@`")]
    EmptyCommit,
    /// Commit SHA not 7..40 hex chars.
    #[error("--ref commit `{0}` must be 7..40 hex chars (0-9 a-f A-F)")]
    InvalidCommit(String),
}

/// FA action — mirrors Lean `Grex.Ref.RefAction` (5 constructors).
///
/// | Tag             | FS mutation         | Manifest mutation | Stderr warn | Exit |
/// |---             |---                  |---                |---         |--- |
/// | `Add`           | clone + checkout    | append entry      | no         | 0  |
/// | `AddSibling`    | clone (sibling dir) | append entry      | no         | 0  |
/// | `WarnAdd`       | clone + checkout    | append entry      | yes        | 0  |
/// | `SilentReject`  | none                | none              | no         | 0  |
/// | `WarnReject`    | none                | none              | yes        | 1  |
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefAction {
    /// New entry, fresh repo (no existing `<reponame>/` parent).
    Add,
    /// New entry, existing `<reponame>/` parent — clone into a new
    /// sibling `<refdir>`.
    AddSibling,
    /// Idempotent no-op: same dup detected, no warning printed.
    /// Cells 2 / 4-dup.
    SilentReject,
    /// Dup detected with explicit user signal (commit pin) — print
    /// stderr warning, no FS or manifest mutation. Cells 6-dup / 8-dup.
    WarnReject,
    /// Distinct entry under existing repo, with explicit commit pin —
    /// print stderr warning, then Add. Cells 6-fresh / 8-fresh.
    WarnAdd,
}

impl RefAction {
    /// True iff the action mutates the world (Add-class).
    pub fn is_add(self) -> bool {
        matches!(self, Self::Add | Self::AddSibling | Self::WarnAdd)
    }

    /// True iff the action is a no-op (Reject-class).
    pub fn is_reject(self) -> bool {
        matches!(self, Self::SilentReject | Self::WarnReject)
    }

    /// True iff the action emits a stderr warning.
    pub fn warns(self) -> bool {
        matches!(self, Self::WarnAdd | Self::WarnReject)
    }
}

/// Probe of the parent manifest at FA evaluation time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AddContext {
    /// `U` axis — does the parent manifest already track a pack with
    /// this URL?
    pub url_tracked: bool,
    /// Dup hit — when `url_tracked = true` and the existing entry has
    /// the SAME `(branch, commit)` tuple as the candidate. Drives the
    /// reject vs sibling/warn-add branch in cells 4 / 6 / 8.
    pub dup_hit: bool,
}

/// Default branch token used when the user wrote no branch component
/// (cells 1, 2, 5, 6 — per OQ2/OQ3 resolution: `main` rather than a
/// `detached/` namespace).
pub const DEFAULT_BRANCH: &str = "main";

/// Minimum SHA prefix length for `<commit-short>` (per OQ5).
pub const SHA_PREFIX_MIN: usize = 7;

/// Maximum SHA prefix length (full SHA-1).
pub const SHA_PREFIX_MAX: usize = 40;

/// Parse `--ref <token>`.
///
/// Accepts:
/// * `main` → bare branch (B=1, C=0)
/// * `a3f9c1d` (7..40 hex chars) → bare commit (B=0, C=1)
/// * `main@a3f9c1d` → branch + commit pin (B=1, C=1)
///
/// `@` is the separator because it is illegal in branch names per
/// git ref-format rules (`git check-ref-format`).
pub fn parse_ref(token: &str) -> Result<Ref, RefParseError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(RefParseError::Empty);
    }
    let parts: Vec<&str> = trimmed.split('@').collect();
    match parts.as_slice() {
        [bare] => {
            // No `@` — bare branch OR bare commit. Disambiguate by
            // hex-shape check.
            if is_hex_sha(bare) {
                Ok(Ref { branch: None, commit: Some((*bare).to_string()) })
            } else {
                Ok(Ref { branch: Some((*bare).to_string()), commit: None })
            }
        }
        [branch, commit] => {
            if branch.is_empty() {
                return Err(RefParseError::EmptyBranch);
            }
            if commit.is_empty() {
                return Err(RefParseError::EmptyCommit);
            }
            if !is_hex_sha(commit) {
                return Err(RefParseError::InvalidCommit((*commit).to_string()));
            }
            Ok(Ref { branch: Some((*branch).to_string()), commit: Some((*commit).to_string()) })
        }
        _ => Err(RefParseError::MultipleAt),
    }
}

/// True iff `s` is 7..40 chars of [0-9a-fA-F]. Used to disambiguate
/// bare-branch vs bare-commit tokens in [`parse_ref`].
fn is_hex_sha(s: &str) -> bool {
    let len = s.len();
    if !(SHA_PREFIX_MIN..=SHA_PREFIX_MAX).contains(&len) {
        return false;
    }
    s.chars().all(|c| c.is_ascii_hexdigit())
}

/// Encode the `<branch>` token: replace `/` with `_` (so
/// `feature/foo` → `feature_foo`). Mirrors Lean `encodeBranchTy`.
fn encode_branch(s: &str) -> String {
    s.chars().map(|c| if c == '/' { '_' } else { c }).collect()
}

/// `<refdir>` for a [`Ref`] using the default 7-char SHA prefix.
/// Equivalent to `encode_refdir_with_prefix(r, SHA_PREFIX_MIN)`.
///
/// Mirrors Lean `Grex.Ref.encodeRefdir`.
pub fn encode_refdir(r: &Ref) -> String {
    encode_refdir_with_prefix(r, SHA_PREFIX_MIN)
}

/// `<refdir>` for a [`Ref`] using a caller-controlled SHA prefix
/// length. Used by [`resolve_unique_refdir`] to extend the prefix on
/// collision (per OQ5).
///
/// `prefix_len` is clamped to `[SHA_PREFIX_MIN..=SHA_PREFIX_MAX]`. When
/// the commit SHA is shorter than `prefix_len` (e.g. user wrote a
/// 7-char short SHA), the entire SHA is used.
pub fn encode_refdir_with_prefix(r: &Ref, prefix_len: usize) -> String {
    let prefix_len = prefix_len.clamp(SHA_PREFIX_MIN, SHA_PREFIX_MAX);
    let branch = match &r.branch {
        Some(b) => encode_branch(b),
        None => DEFAULT_BRANCH.to_string(),
    };
    match &r.commit {
        None => branch,
        Some(sha) => {
            let take = prefix_len.min(sha.len());
            let short: String = sha.chars().take(take).collect();
            format!("{branch}@{short}")
        }
    }
}

/// Collision-extend resolver (OQ5). Compute a unique `<refdir>` for
/// `r` against `existing`, extending the SHA prefix one char at a
/// time when a collision is detected.
///
/// Returns the resolved folder name. When `r.commit` is `None` (bare
/// branch — cells 1, 2, 3, 4) the folder name has no SHA component,
/// so collision-extend has nothing to extend; the function falls back
/// to [`encode_refdir`] and returns whatever name that produces (the
/// FA's reject branches handle the duplicate case at the action
/// level).
///
/// Mirrors Lean `Grex.Ref.encodeRefdir_distinct` (the OQ5 axiom).
pub fn resolve_unique_refdir(r: &Ref, existing: &HashSet<String>) -> String {
    // Bare branch or no commit — encoding is fully determined; nothing
    // to extend.
    let Some(sha) = r.commit.as_ref() else {
        return encode_refdir(r);
    };
    let max_len = sha.len().min(SHA_PREFIX_MAX);
    let mut len = SHA_PREFIX_MIN.min(max_len);
    if len < SHA_PREFIX_MIN {
        // sha shorter than 7 chars — `parse_ref` rejects this, but
        // defend against direct construction.
        len = max_len;
    }
    loop {
        let candidate = encode_refdir_with_prefix(r, len);
        if !existing.contains(&candidate) {
            return candidate;
        }
        if len >= max_len {
            // Exhausted all prefix lengths (full-SHA equality). The
            // FA's dup branches classify this case as
            // SilentReject/WarnReject; the resolver returns the full
            // candidate so callers can match against `existing`.
            return candidate;
        }
        len += 1;
    }
}

/// Drive the 8-cell FA. Mirrors Lean `Grex.Ref.ref_fa` (action axis
/// only — the `<refdir>` axis is computed by [`resolve_unique_refdir`]
/// at the call site).
///
/// The Boolean triple `(B, C, U)` is read from the inputs:
/// * `B` = `r.branch.is_some()`
/// * `C` = `r.commit.is_some()`
/// * `U` = `ctx.url_tracked`
///
/// `ctx.dup_hit` disambiguates the U=1 cells (4, 6, 8) into reject vs
/// add-sibling/warn-add sub-branches.
pub fn classify_ref_input(r: &Ref, ctx: &AddContext) -> RefAction {
    let b = r.branch.is_some();
    let c = r.commit.is_some();
    let u = ctx.url_tracked;
    let dup = ctx.dup_hit;
    match (b, c, u) {
        // cell 1: B=0 C=0 U=0 → Add (default `main`).
        (false, false, false) => RefAction::Add,
        // cell 2: B=0 C=0 U=1 → silent reject (dup of default checkout).
        (false, false, true) => RefAction::SilentReject,
        // cell 3: B=1 C=0 U=0 → Add (new branch under fresh repo).
        (true, false, false) => RefAction::Add,
        // cell 4: B=1 C=0 U=1 → silent reject if dup, else AddSibling.
        (true, false, true) => {
            if dup {
                RefAction::SilentReject
            } else {
                RefAction::AddSibling
            }
        }
        // cell 5: B=0 C=1 U=0 → Add (bare commit, default `main`).
        (false, true, false) => RefAction::Add,
        // cell 6: B=0 C=1 U=1 → warn-reject if dup, else warn-add.
        (false, true, true) => {
            if dup {
                RefAction::WarnReject
            } else {
                RefAction::WarnAdd
            }
        }
        // cell 7: B=1 C=1 U=0 → Add (branch + commit pin, fresh repo).
        (true, true, false) => RefAction::Add,
        // cell 8: B=1 C=1 U=1 → warn-reject if dup, else warn-add.
        (true, true, true) => {
            if dup {
                RefAction::WarnReject
            } else {
                RefAction::WarnAdd
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- parse_ref ---------------------------------------------------

    #[test]
    fn parse_bare_branch() {
        let r = parse_ref("main").unwrap();
        assert_eq!(r, Ref { branch: Some("main".into()), commit: None });
    }

    #[test]
    fn parse_bare_branch_with_slash() {
        let r = parse_ref("feature/foo").unwrap();
        assert_eq!(r, Ref { branch: Some("feature/foo".into()), commit: None });
    }

    #[test]
    fn parse_bare_short_sha() {
        let r = parse_ref("a3f9c1d").unwrap();
        assert_eq!(r, Ref { branch: None, commit: Some("a3f9c1d".into()) });
    }

    #[test]
    fn parse_bare_full_sha() {
        let sha = "a3f9c1d2b8e7f6a5c4d3b2a1908f7e6d5c4b3a29";
        let r = parse_ref(sha).unwrap();
        assert_eq!(r, Ref { branch: None, commit: Some(sha.to_string()) });
    }

    #[test]
    fn parse_branch_at_commit() {
        let r = parse_ref("main@a3f9c1d").unwrap();
        assert_eq!(r, Ref { branch: Some("main".into()), commit: Some("a3f9c1d".into()) });
    }

    #[test]
    fn parse_empty_rejected() {
        assert_eq!(parse_ref(""), Err(RefParseError::Empty));
        assert_eq!(parse_ref("   "), Err(RefParseError::Empty));
    }

    #[test]
    fn parse_double_at_rejected() {
        assert_eq!(parse_ref("a@b@c"), Err(RefParseError::MultipleAt));
    }

    #[test]
    fn parse_empty_branch_rejected() {
        assert_eq!(parse_ref("@a3f9c1d"), Err(RefParseError::EmptyBranch));
    }

    #[test]
    fn parse_empty_commit_rejected() {
        assert_eq!(parse_ref("main@"), Err(RefParseError::EmptyCommit));
    }

    #[test]
    fn parse_invalid_commit_rejected() {
        // 7 chars but not all hex
        assert!(matches!(parse_ref("main@notahex"), Err(RefParseError::InvalidCommit(_))));
        // too short
        assert!(matches!(parse_ref("main@abc"), Err(RefParseError::InvalidCommit(_))));
    }

    #[test]
    fn parse_short_six_char_branch_not_misread_as_sha() {
        // 6 hex chars is below `SHA_PREFIX_MIN`, so it falls through
        // to the bare-branch arm.
        let r = parse_ref("abcdef").unwrap();
        assert_eq!(r, Ref { branch: Some("abcdef".into()), commit: None });
    }

    // ----- encode_refdir -----------------------------------------------

    #[test]
    fn encode_bare_branch() {
        let r = Ref { branch: Some("main".into()), commit: None };
        assert_eq!(encode_refdir(&r), "main");
    }

    #[test]
    fn encode_branch_with_slash_replaced() {
        let r = Ref { branch: Some("feature/foo".into()), commit: None };
        assert_eq!(encode_refdir(&r), "feature_foo");
    }

    #[test]
    fn encode_no_branch_no_commit_defaults_to_main() {
        let r = Ref { branch: None, commit: None };
        assert_eq!(encode_refdir(&r), "main");
    }

    #[test]
    fn encode_bare_commit_uses_main_at_short() {
        let r = Ref { branch: None, commit: Some("a3f9c1d2b8e7".into()) };
        assert_eq!(encode_refdir(&r), "main@a3f9c1d");
    }

    #[test]
    fn encode_branch_at_commit_short_seven() {
        let r = Ref { branch: Some("main".into()), commit: Some("a3f9c1d2b8e7".into()) };
        assert_eq!(encode_refdir(&r), "main@a3f9c1d");
    }

    #[test]
    fn encode_with_extended_prefix() {
        let r = Ref { branch: Some("main".into()), commit: Some("a3f9c1d2b8e7".into()) };
        assert_eq!(encode_refdir_with_prefix(&r, 8), "main@a3f9c1d2");
        assert_eq!(encode_refdir_with_prefix(&r, 12), "main@a3f9c1d2b8e7");
    }

    #[test]
    fn encode_prefix_clamps_to_min_max() {
        let r = Ref { branch: Some("main".into()), commit: Some("a".repeat(40)) };
        // Below min clamped up to 7.
        assert_eq!(encode_refdir_with_prefix(&r, 3).len(), "main@".len() + 7);
        // Above max clamped down to 40.
        assert_eq!(encode_refdir_with_prefix(&r, 100).len(), "main@".len() + 40);
    }

    // ----- resolve_unique_refdir (collision-extend, OQ5) ---------------

    #[test]
    fn resolve_no_collision_returns_seven_char_prefix() {
        let r = Ref { branch: Some("main".into()), commit: Some("a3f9c1d2b8e7".into()) };
        let existing: HashSet<String> = HashSet::new();
        assert_eq!(resolve_unique_refdir(&r, &existing), "main@a3f9c1d");
    }

    #[test]
    fn resolve_collision_extends_one_char() {
        // Force collision on 7-char prefix; 8-char prefix is unique.
        let r = Ref { branch: Some("main".into()), commit: Some("a3f9c1d2b8e7".into()) };
        let mut existing = HashSet::new();
        existing.insert("main@a3f9c1d".to_string());
        assert_eq!(resolve_unique_refdir(&r, &existing), "main@a3f9c1d2");
    }

    #[test]
    fn resolve_collision_extends_multiple_chars() {
        // Force collision on 7-char and 8-char prefixes.
        let r = Ref { branch: Some("main".into()), commit: Some("a3f9c1d2b8e7".into()) };
        let mut existing = HashSet::new();
        existing.insert("main@a3f9c1d".to_string());
        existing.insert("main@a3f9c1d2".to_string());
        existing.insert("main@a3f9c1d2b".to_string());
        assert_eq!(resolve_unique_refdir(&r, &existing), "main@a3f9c1d2b8");
    }

    #[test]
    fn resolve_bare_branch_no_extension() {
        // No commit — nothing to extend; resolver returns the
        // un-suffixed name even on collision (FA reject branches
        // handle dup at action level).
        let r = Ref { branch: Some("main".into()), commit: None };
        let mut existing = HashSet::new();
        existing.insert("main".to_string());
        assert_eq!(resolve_unique_refdir(&r, &existing), "main");
    }

    #[test]
    fn resolve_full_sha_collision_returns_full() {
        // 12-char SHA, all prefixes collide with existing entries.
        let r = Ref { branch: Some("main".into()), commit: Some("a3f9c1d2b8e7".into()) };
        let mut existing = HashSet::new();
        for n in SHA_PREFIX_MIN..=12 {
            existing.insert(encode_refdir_with_prefix(&r, n));
        }
        // Resolver exhausts; returns the full-12-char encoding.
        assert_eq!(resolve_unique_refdir(&r, &existing), "main@a3f9c1d2b8e7");
    }

    // ----- classify_ref_input (8-cell FA) ------------------------------

    fn br(name: &str) -> Ref {
        Ref { branch: Some(name.into()), commit: None }
    }
    fn co(sha: &str) -> Ref {
        Ref { branch: None, commit: Some(sha.into()) }
    }
    fn br_co(name: &str, sha: &str) -> Ref {
        Ref { branch: Some(name.into()), commit: Some(sha.into()) }
    }

    #[test]
    fn fa_cell_1_b0_c0_u0_add() {
        let r = Ref { branch: None, commit: None };
        let ctx = AddContext { url_tracked: false, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::Add);
    }

    #[test]
    fn fa_cell_2_b0_c0_u1_silent_reject() {
        let r = Ref { branch: None, commit: None };
        let ctx = AddContext { url_tracked: true, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::SilentReject);
    }

    #[test]
    fn fa_cell_3_b1_c0_u0_add() {
        let r = br("feature/foo");
        let ctx = AddContext { url_tracked: false, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::Add);
    }

    #[test]
    fn fa_cell_4_b1_c0_u1_dup_silent_reject() {
        let r = br("main");
        let ctx = AddContext { url_tracked: true, dup_hit: true };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::SilentReject);
    }

    #[test]
    fn fa_cell_4_b1_c0_u1_no_dup_add_sibling() {
        let r = br("develop");
        let ctx = AddContext { url_tracked: true, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::AddSibling);
    }

    #[test]
    fn fa_cell_5_b0_c1_u0_add() {
        let r = co("a3f9c1d");
        let ctx = AddContext { url_tracked: false, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::Add);
    }

    #[test]
    fn fa_cell_6_b0_c1_u1_dup_warn_reject() {
        let r = co("a3f9c1d");
        let ctx = AddContext { url_tracked: true, dup_hit: true };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::WarnReject);
    }

    #[test]
    fn fa_cell_6_b0_c1_u1_no_dup_warn_add() {
        let r = co("a3f9c1d");
        let ctx = AddContext { url_tracked: true, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::WarnAdd);
    }

    #[test]
    fn fa_cell_7_b1_c1_u0_add() {
        let r = br_co("main", "a3f9c1d");
        let ctx = AddContext { url_tracked: false, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::Add);
    }

    #[test]
    fn fa_cell_8_b1_c1_u1_dup_warn_reject() {
        let r = br_co("main", "a3f9c1d");
        let ctx = AddContext { url_tracked: true, dup_hit: true };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::WarnReject);
    }

    #[test]
    fn fa_cell_8_b1_c1_u1_no_dup_warn_add() {
        let r = br_co("main", "a3f9c1d");
        let ctx = AddContext { url_tracked: true, dup_hit: false };
        assert_eq!(classify_ref_input(&r, &ctx), RefAction::WarnAdd);
    }

    // ----- predicates --------------------------------------------------

    #[test]
    fn ref_action_predicates_partition() {
        // Add-class and Reject-class are disjoint and exhaustive.
        for a in [
            RefAction::Add,
            RefAction::AddSibling,
            RefAction::WarnAdd,
            RefAction::SilentReject,
            RefAction::WarnReject,
        ] {
            assert_ne!(a.is_add(), a.is_reject(), "action {a:?}");
        }
        // Warns predicate is independent.
        assert!(RefAction::WarnAdd.warns());
        assert!(RefAction::WarnReject.warns());
        assert!(!RefAction::Add.warns());
        assert!(!RefAction::AddSibling.warns());
        assert!(!RefAction::SilentReject.warns());
    }

    // ----- dup_safe (idempotence) --------------------------------------

    #[test]
    fn dup_safe_idempotent_on_identical_re_add() {
        // Mirror Lean `dup_safe`: re-adding an identical ref produces
        // a Reject-class action, which the caller MUST translate to a
        // no-op. Cells 2, 4-dup, 6-dup, 8-dup.
        let cases: [(Ref, AddContext); 4] = [
            (Ref { branch: None, commit: None }, AddContext { url_tracked: true, dup_hit: false }), // cell 2
            (br("main"), AddContext { url_tracked: true, dup_hit: true }), // cell 4-dup
            (co("a3f9c1d"), AddContext { url_tracked: true, dup_hit: true }), // cell 6-dup
            (br_co("main", "a3f9c1d"), AddContext { url_tracked: true, dup_hit: true }), // cell 8-dup
        ];
        for (r, ctx) in &cases {
            let action = classify_ref_input(r, ctx);
            assert!(action.is_reject(), "expected reject for {r:?} {ctx:?}, got {action:?}");
        }
    }
}
