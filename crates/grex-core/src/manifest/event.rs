//! Event types for the manifest log.
//!
//! Events are JSON objects with a `"op"` discriminant. Unknown fields are
//! **intentionally accepted** (no `#[serde(deny_unknown_fields)]`) so that
//! older grex binaries can still read newer logs as long as the operation
//! type is known.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Logical identifier of a pack. Stable across updates, unique per workspace.
pub type PackId = String;

/// Current manifest schema version. Bumped whenever event shapes change
/// incompatibly.
///
/// # v1.3.1 schema v2 hard-cut (B8)
///
/// Bumped from `"1"` to `"2"` as part of the v1.3.1 dogfood-finding
/// remediation:
///
/// * The action-audit variants ([`Event::ActionStarted`],
///   [`Event::ActionCompleted`], [`Event::ActionHalted`]) renamed their
///   pack-id field from `pack` to `id` so every event variant uses the
///   same `id` discriminant per `.omne/cfg/manifest.md` §"events schema
///   v2" reader contract. Each of those three variants now also carries
///   a `schema_version: String` field so consumers can disambiguate v1
///   vs v2 records line-by-line during the migration window (no field
///   deployments per maintainer 2026-05-02; this is the only writer
///   shape from now on).
/// * A new variant [`Event::DryRunWouldClone`] is added for B4 — emitted
///   by the walker when `dry_run = true` instead of firing a real clone
///   subprocess + lockfile write + events.jsonl mutation. Carries the
///   would-be-cloned child's id (= folder name = repo name), declared
///   `ref`, and `url` so a downstream caller can reconstruct the plan.
///
/// Per the maintainer's 2026-05-02 directive, no v1 writers ever shipped
/// the legacy `pack` field to a customer environment; the rename is a
/// hard cut with no back-compat shim. Old logs (if any internal dogfood
/// log carried v1 records) still decode through the existing
/// `serde(rename_all = "snake_case")` + `Unknown` fallback because the
/// variant tags themselves did not change — only the field names within
/// each variant.
pub const SCHEMA_VERSION: &str = "2";

/// One entry in the manifest log.
///
/// Serialized form uses a lowercase `"op"` tag:
/// ```json
/// {"op":"add","ts":"...","id":"...","url":"...","path":"...","type":"...","schema_version":"2"}
/// ```
///
/// # Action audit variants (PR E, schema v2 since v1.3.1)
///
/// [`Event::ActionStarted`] is appended **before** the executor runs an
/// action. [`Event::ActionCompleted`] is appended **after** success;
/// [`Event::ActionHalted`] is appended **after** failure. A dangling
/// `ActionStarted` with no matching completed/halted peer is a crash
/// candidate — see [`crate::sync::scan_recovery`].
///
/// As of v1.3.1 (schema v2) the pack-id field on these three variants
/// is named `id` (was `pack` in v1) and each carries a `schema_version`
/// string field — see the [`SCHEMA_VERSION`] doc comment for the
/// rationale. The runtime invariant that `id` equals the on-disk folder
/// name (= repo name) for both meta-packs and single packs is enforced
/// by writers; deserialization preserves whatever string the writer
/// emitted (the schema is shape-only, not a folder-name validator).
///
/// These variants are ignored by [`crate::manifest::fold::fold`] (they do
/// not mutate pack state) so the folded projection is unchanged from the
/// pre-PR-E schema; old readers decoding a log that contains them still
/// parse successfully because the `op` discriminants are known lowercase
/// tags with plain fields (unknown fields are tolerated per module docs).
#[non_exhaustive]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Event {
    /// Register a new pack in the workspace.
    Add {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Upstream source URL (git remote, file path, etc).
        url: String,
        /// Workspace-relative checkout path.
        path: String,
        /// Pack flavor (e.g. `"declarative"`, `"imperative"`).
        #[serde(rename = "type")]
        pack_type: String,
        /// Schema version at time of write.
        schema_version: String,
    },
    /// Update a single field on an existing pack.
    ///
    /// `field` must be one of `"url"`, `"ref"`, `"path"`. Unknown field
    /// names are ignored by [`crate::manifest::fold::fold`] to keep forward
    /// compatibility.
    Update {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Field name being updated.
        field: String,
        /// New value (string or other JSON scalar).
        value: serde_json::Value,
    },
    /// Remove a pack from the workspace.
    Rm {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
    },
    /// Record a completed action step. Emitted on the success path of
    /// [`crate::sync::run`] along with [`Event::ActionCompleted`]; the
    /// `sha` field carries a short human summary of the step outcome
    /// (kept for backward-compat with M2 readers that folded only on this
    /// variant).
    Sync {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier.
        id: PackId,
        /// Resolved commit SHA or short action summary.
        sha: String,
    },
    /// An executor is **about to run** an action. Written before
    /// `executor.execute` so a crash mid-action leaves a discoverable
    /// trace. A dangling `ActionStarted` with no matching completed/halted
    /// peer signals a crashed run — see [`crate::sync::scan_recovery`].
    ///
    /// v1.3.1 (schema v2): pack-id field renamed from `pack` to `id`;
    /// `schema_version` field added. See [`SCHEMA_VERSION`].
    ActionStarted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier owning the action. Runtime invariant: equals
        /// the on-disk folder name (= repo name) for both meta-packs
        /// and single packs.
        id: PackId,
        /// 0-based index into the pack's top-level `actions` vector.
        action_idx: usize,
        /// Short action kind tag (e.g. `"symlink"`, `"mkdir"`).
        action_name: String,
        /// Schema version at time of write.
        schema_version: String,
    },
    /// The executor returned `Ok`. Paired with a preceding
    /// [`Event::ActionStarted`]. `result_summary` is a short
    /// human-readable string (e.g. `"performed_change"`).
    ///
    /// v1.3.1 (schema v2): pack-id field renamed from `pack` to `id`;
    /// `schema_version` field added. See [`SCHEMA_VERSION`].
    ActionCompleted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier owning the action. Same folder-name invariant
        /// as [`Event::ActionStarted::id`].
        id: PackId,
        /// 0-based index into the pack's top-level `actions` vector.
        action_idx: usize,
        /// Short outcome summary tag.
        result_summary: String,
        /// Schema version at time of write.
        schema_version: String,
    },
    /// The executor returned `Err`. Paired with a preceding
    /// [`Event::ActionStarted`]. `error_summary` is the error's `Display`
    /// output truncated to a small limit so an audit trail line stays
    /// single-event-sized.
    ///
    /// v1.3.1 (schema v2): pack-id field renamed from `pack` to `id`;
    /// `schema_version` field added. See [`SCHEMA_VERSION`].
    ActionHalted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Pack identifier owning the action. Same folder-name invariant
        /// as [`Event::ActionStarted::id`].
        id: PackId,
        /// 0-based index into the pack's top-level `actions` vector.
        action_idx: usize,
        /// Short action kind tag.
        action_name: String,
        /// Truncated error message (at most
        /// [`ACTION_ERROR_SUMMARY_MAX`] bytes).
        error_summary: String,
        /// Schema version at time of write.
        schema_version: String,
    },
    /// v1.3.1 (B4) — emitted by the walker when `dry_run = true` instead
    /// of firing a real clone subprocess. Records what WOULD have been
    /// cloned so a downstream caller can reconstruct the plan without
    /// any network call, FS write, or lockfile mutation having happened.
    ///
    /// `id` is the would-be-cloned child's folder name (= repo name);
    /// `ref_` mirrors the manifest-declared ref (or the global
    /// `--ref` override when set); `url` is the upstream source URL.
    /// The variant carries no `path`/`pack_type` because dry-run does
    /// not load the child's pack.yaml — only what the parent manifest
    /// declares is observable.
    ///
    /// `op` discriminator: `"dry_run_would_clone"` (snake_case).
    /// `ref_` serializes as `"ref"` to dodge the Rust keyword while
    /// keeping the JSONL field name aligned with manifest authorship
    /// conventions.
    DryRunWouldClone {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Folder name (= repo name) the child would land at if a real
        /// sync ran.
        id: PackId,
        /// Upstream source URL. Mirrors the manifest `url` field.
        url: String,
        /// Effective ref (manifest-declared or `--ref` override).
        /// `None` when neither is set; downstream callers display this
        /// as the backend's default branch.
        #[serde(rename = "ref")]
        ref_: Option<String>,
        /// Schema version at time of write.
        schema_version: String,
    },
    /// v1.2.0 Stage 1.l — A walker Phase 2 prune fired against a
    /// non-Clean consent verdict because the operator requested
    /// `--force-prune` (or the stronger `--force-prune-with-ignored`).
    /// Postmortem-only: emitted ONLY when the override flags actually
    /// consumed a non-Clean verdict; clean-consent prunes do not write
    /// this event. Tracks the dest path, the refusal kind that was
    /// overridden, and whether the stronger ignored-content override
    /// was in effect.
    ForcePruneExecuted {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Absolute path of the dest that was pruned (display form).
        path: String,
        /// Stable lowercase tag for the refusal kind that was
        /// overridden (`"dirty_tree"`, `"dirty_tree_with_ignored"`,
        /// `"sub_meta_with_dirty_children"`). `GitInProgress` is never
        /// overridable so it never appears here.
        kind: String,
        /// `true` when the stronger `--force-prune-with-ignored` was
        /// in effect at the time of the override; `false` when only
        /// the base `--force-prune` was set.
        force_prune_with_ignored: bool,
    },
    /// v1.2.1 Item 5b — `--quarantine` lifecycle: append-and-fsync'd
    /// BEFORE any byte of the recursive snapshot is written. The audit
    /// event must precede the snapshot per Lean theorem
    /// `quarantine_snapshot_precedes_delete` (proof/Grex/Quarantine.lean):
    /// `delete_licensed` only holds when this entry is durable in the
    /// log. If the audit append fails the prune aborts with no FS
    /// mutation; if the subsequent snapshot fails the prune still
    /// aborts (no `unlink(dest)`) and a [`Event::QuarantineFailed`]
    /// follow-up is logged.
    QuarantineStart {
        /// Event timestamp (matches the `ts` segment in `trash_path`).
        ts: DateTime<Utc>,
        /// Absolute path of the dest about to be snapshot-then-pruned
        /// (display form).
        src: String,
        /// Absolute path of the trash bucket the snapshot will be
        /// copied to: `<meta>/.grex/trash/<ISO8601>/<basename>/`.
        trash: String,
    },
    /// v1.2.1 Item 5b — `--quarantine` lifecycle: appended after the
    /// snapshot succeeded AND `unlink(dest)` succeeded. Pairs 1:1 with
    /// a preceding [`Event::QuarantineStart`]. Absence of this event
    /// after a Start is a forensic signal: either the snapshot or the
    /// unlink failed (look for [`Event::QuarantineFailed`] or a
    /// partial trash dir at `trash`).
    QuarantineComplete {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Absolute path of the dest that was successfully pruned
        /// after snapshot.
        src: String,
        /// Absolute path of the snapshot bucket holding the recursive
        /// copy of the pre-prune subtree.
        trash: String,
    },
    /// v1.2.1 Item 5b — `--quarantine` lifecycle: appended when the
    /// recursive snapshot or the audited unlink failed AFTER the
    /// preceding [`Event::QuarantineStart`] entry was already on disk.
    /// The original `src` MUST remain intact — the unlink either never
    /// fired (snapshot failure) or is the failure being reported here.
    /// A partial trash dir may remain at `trash` for forensics.
    QuarantineFailed {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Absolute path of the dest the prune attempted (and aborted
        /// on).
        src: String,
        /// Absolute path of the trash bucket. May exist as a partial
        /// snapshot for operator inspection; never auto-cleaned.
        trash: String,
        /// Truncated failure message (display form of the underlying
        /// I/O error). Bounded to keep one event on one line.
        error: String,
    },
    /// v1.2.5 — operator restored a quarantined snapshot back into the
    /// workspace via `grex doctor --restore-quarantine <ts>
    /// [<basename>]`. Records both the source snapshot path the operator
    /// pulled from and the dest the bytes were restored to. Fully
    /// additive variant — older readers tolerate it via the
    /// [`Event::Unknown`] fallback below (forward-compat per JSONL
    /// policy).
    QuarantineRestored {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Absolute path of the trash snapshot the bytes came from
        /// (`<meta>/.grex/trash/<ts>/<basename>/`).
        src: String,
        /// Absolute path of the dest the snapshot was restored to.
        dest: String,
    },
    /// v1.2.5 — quarantine GC sweep removed an aged trash entry. Emitted
    /// per pruned entry by [`crate::tree::quarantine::prune_quarantine`]
    /// (best-effort; failures log via tracing instead of producing this
    /// event). Fully additive — older readers tolerate via
    /// [`Event::Unknown`].
    QuarantineGcSwept {
        /// Event timestamp.
        ts: DateTime<Utc>,
        /// Absolute path of the trash entry that was deleted
        /// (`<meta>/.grex/trash/<ts>/`).
        entry: String,
        /// Age in whole days of the entry at the time of the sweep
        /// (cutoff = `now - retain_days`; entries with age > cutoff
        /// are pruned).
        age_days: u64,
    },
    /// v1.2.5 — forward-compat fallback for unknown `op` discriminants.
    /// Exists so an OLDER `grex` binary reading a NEWER log (e.g.
    /// containing variants added after this binary was built) decodes
    /// the unknown line as `Unknown` instead of failing the whole
    /// `read_all` with a `Corruption` error. The variant is unit (no
    /// fields) by design: any payload the writer included is dropped on
    /// the read side. Folding logic ([`crate::manifest::fold::fold`])
    /// already ignores audit-only variants, and accessor methods
    /// ([`Event::id`], [`Event::ts`]) return placeholder values so call
    /// sites don't need to special-case the variant. Per JSONL forward-
    /// compat policy in design.md.
    ///
    /// # `#[serde(other)]` semantics
    ///
    /// Catches unknown `op` discriminator values for forward-compat.
    /// Filtered out by [`crate::manifest::read_all`] (silently dropped
    /// regardless of position so older readers can still consume newer
    /// logs without erroring on the rest of the stream). Cannot be
    /// written back: the write-side guard in
    /// [`crate::manifest::append_event`] panics in debug builds and
    /// no-ops with a `tracing::error!` in release builds.
    #[serde(other)]
    Unknown,
}

/// Lazy-initialised empty [`PackId`] returned by [`Event::id`] for the
/// [`Event::Unknown`] forward-compat variant. Avoids a `'static` literal
/// so the return type stays `&PackId` (= `&String`) without the variant
/// having to carry a synthetic id field.
fn empty_pack_id() -> &'static PackId {
    static EMPTY: std::sync::OnceLock<PackId> = std::sync::OnceLock::new();
    EMPTY.get_or_init(String::new)
}

/// Max bytes retained in [`Event::ActionHalted::error_summary`].
///
/// Truncation keeps one halt record on one JSONL line without
/// pathological blowup when an executor surfaces a multi-KB error
/// (e.g. captured stderr from an exec failure).
pub const ACTION_ERROR_SUMMARY_MAX: usize = 2048;

impl Event {
    /// Return the stable snake_case op tag for this variant.
    ///
    /// This is the same string that serde emits as the `"op"` field when
    /// the event is serialized (driven by
    /// `#[serde(tag = "op", rename_all = "snake_case")]` on the enum).
    /// It is intended as a Display-stable name for use in tracing
    /// fields (`op = %ev.op_name()`) so trace output renders human-
    /// readable tags (e.g. `op="sync"`) instead of the opaque
    /// `op=Discriminant(N)` produced by `Debug`-formatting
    /// [`std::mem::discriminant`].
    ///
    /// v1.3.1 fix-sweep B7: replaces the `op = ?std::mem::discriminant(ev)`
    /// site in [`crate::manifest::append::emit_semantic_warnings`].
    pub fn op_name(&self) -> &'static str {
        match self {
            Event::Add { .. } => "add",
            Event::Update { .. } => "update",
            Event::Rm { .. } => "rm",
            Event::Sync { .. } => "sync",
            Event::ActionStarted { .. } => "action_started",
            Event::ActionCompleted { .. } => "action_completed",
            Event::ActionHalted { .. } => "action_halted",
            Event::DryRunWouldClone { .. } => "dry_run_would_clone",
            Event::ForcePruneExecuted { .. } => "force_prune_executed",
            Event::QuarantineStart { .. } => "quarantine_start",
            Event::QuarantineComplete { .. } => "quarantine_complete",
            Event::QuarantineFailed { .. } => "quarantine_failed",
            Event::QuarantineRestored { .. } => "quarantine_restored",
            Event::QuarantineGcSwept { .. } => "quarantine_gc_swept",
            Event::Unknown => "unknown",
        }
    }

    /// Return the pack id the event applies to.
    ///
    /// As of v1.3.1 (schema v2) every pack-scoped variant uses the same
    /// `id` field name — see [`SCHEMA_VERSION`] for the rename rationale.
    /// Workspace-scoped variants ([`Event::ForcePruneExecuted`]) return
    /// the dest `path` as their identifier — there is no single owning
    /// pack for an audit-only override record.
    pub fn id(&self) -> &PackId {
        match self {
            Event::Add { id, .. }
            | Event::Update { id, .. }
            | Event::Rm { id, .. }
            | Event::Sync { id, .. }
            | Event::ActionStarted { id, .. }
            | Event::ActionCompleted { id, .. }
            | Event::ActionHalted { id, .. }
            | Event::DryRunWouldClone { id, .. } => id,
            Event::ForcePruneExecuted { path, .. } => path,
            // v1.2.1 Item 5b — quarantine variants carry the dest as
            // their identifier (`src`); same audit-only pattern as
            // `ForcePruneExecuted` (no owning pack id).
            Event::QuarantineStart { src, .. }
            | Event::QuarantineComplete { src, .. }
            | Event::QuarantineFailed { src, .. }
            | Event::QuarantineRestored { src, .. } => src,
            // v1.2.5 — GC sweep variant carries the trash entry path
            // as its identifier (no owning pack id; same audit-only
            // pattern).
            Event::QuarantineGcSwept { entry, .. } => entry,
            // v1.2.5 — forward-compat fallback. No payload to
            // surface; return a stable empty id so legacy callers
            // that group by `id()` see the line as a non-pack-scoped
            // entry.
            Event::Unknown => empty_pack_id(),
        }
    }

    /// Return the event timestamp.
    pub fn ts(&self) -> DateTime<Utc> {
        match self {
            Event::Add { ts, .. }
            | Event::Update { ts, .. }
            | Event::Rm { ts, .. }
            | Event::Sync { ts, .. }
            | Event::ActionStarted { ts, .. }
            | Event::ActionCompleted { ts, .. }
            | Event::ActionHalted { ts, .. }
            | Event::DryRunWouldClone { ts, .. }
            | Event::ForcePruneExecuted { ts, .. }
            | Event::QuarantineStart { ts, .. }
            | Event::QuarantineComplete { ts, .. }
            | Event::QuarantineFailed { ts, .. }
            | Event::QuarantineRestored { ts, .. }
            | Event::QuarantineGcSwept { ts, .. } => *ts,
            // v1.2.5 — `Unknown` carries no payload; return the Unix
            // epoch as the stable placeholder so consumers that order
            // by `ts()` still get a well-defined value.
            Event::Unknown => DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_default(),
        }
    }
}

/// Current resolved state of a single pack.
///
/// Produced by folding the manifest log. **Not serialized** — this is an
/// in-memory projection; the lockfile has its own on-disk shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackState {
    /// Pack identifier.
    pub id: PackId,
    /// Upstream URL.
    pub url: String,
    /// Workspace-relative checkout path.
    pub path: String,
    /// Pack flavor.
    pub pack_type: String,
    /// Optional ref spec (branch, tag) — `None` until first `update ref`.
    pub ref_spec: Option<String>,
    /// Last synced SHA — `None` until first `sync`.
    pub last_sync_sha: Option<String>,
    /// Timestamp of the originating `add` event.
    pub added_at: DateTime<Utc>,
    /// Timestamp of the most recent event that touched this pack.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 19, 10, 0, 0).unwrap()
    }

    #[test]
    fn add_roundtrip() {
        let e = Event::Add {
            ts: ts(),
            id: "warp-cfg".into(),
            url: "git@example:warp".into(),
            path: "warp-cfg".into(),
            pack_type: "declarative".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        let back: Event = serde_json::from_str(&s).unwrap();
        assert_eq!(e, back);
        assert!(s.contains(r#""op":"add""#));
        assert!(s.contains(r#""type":"declarative""#));
    }

    #[test]
    fn update_roundtrip() {
        let e = Event::Update {
            ts: ts(),
            id: "warp-cfg".into(),
            field: "ref".into(),
            value: serde_json::json!("v0.2.0"),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn rm_roundtrip() {
        let e = Event::Rm { ts: ts(), id: "warp-cfg".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn sync_roundtrip() {
        let e = Event::Sync { ts: ts(), id: "warp-cfg".into(), sha: "abc123".into() };
        let s = serde_json::to_string(&e).unwrap();
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn unknown_fields_are_accepted() {
        // Forward-compat: a newer writer may add fields the old reader
        // doesn't know about. Parse must still succeed.
        let raw = r#"{"op":"rm","ts":"2026-04-19T10:00:00Z","id":"x","future_field":true}"#;
        let e: Event = serde_json::from_str(raw).unwrap();
        assert_eq!(e.id(), "x");
    }

    #[test]
    fn id_and_ts_accessors() {
        let e = Event::Sync { ts: ts(), id: "a".into(), sha: "s".into() };
        assert_eq!(e.id(), "a");
        assert_eq!(e.ts(), ts());
    }

    #[test]
    fn action_started_roundtrip() {
        let e = Event::ActionStarted {
            ts: ts(),
            id: "warp".into(),
            action_idx: 3,
            action_name: "symlink".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"action_started""#));
        // v1.3.1 schema v2 hard-cut: field is `id`, not `pack`.
        assert!(s.contains(r#""id":"warp""#));
        assert!(!s.contains(r#""pack":"warp""#));
        assert!(s.contains(r#""schema_version":"2""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn action_completed_roundtrip() {
        let e = Event::ActionCompleted {
            ts: ts(),
            id: "warp".into(),
            action_idx: 1,
            result_summary: "performed_change".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"action_completed""#));
        assert!(s.contains(r#""id":"warp""#));
        assert!(!s.contains(r#""pack":"warp""#));
        assert!(s.contains(r#""schema_version":"2""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn action_halted_roundtrip() {
        let e = Event::ActionHalted {
            ts: ts(),
            id: "warp".into(),
            action_idx: 2,
            action_name: "exec".into(),
            error_summary: "non-zero exit 3".into(),
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"action_halted""#));
        assert!(s.contains(r#""id":"warp""#));
        assert!(!s.contains(r#""pack":"warp""#));
        assert!(s.contains(r#""schema_version":"2""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn dry_run_would_clone_roundtrip() {
        let e = Event::DryRunWouldClone {
            ts: ts(),
            id: "warp-cfgs".into(),
            url: "https://example.com/warp-cfgs.git".into(),
            ref_: Some("main".into()),
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""op":"dry_run_would_clone""#));
        assert!(s.contains(r#""id":"warp-cfgs""#));
        // Rust keyword `ref` is exposed via `#[serde(rename = "ref")]`.
        assert!(s.contains(r#""ref":"main""#));
        assert!(s.contains(r#""schema_version":"2""#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn dry_run_would_clone_no_ref_serializes_null() {
        let e = Event::DryRunWouldClone {
            ts: ts(),
            id: "warp-cfgs".into(),
            url: "https://example.com/warp-cfgs.git".into(),
            ref_: None,
            schema_version: SCHEMA_VERSION.into(),
        };
        let s = serde_json::to_string(&e).unwrap();
        assert!(s.contains(r#""ref":null"#));
        assert_eq!(serde_json::from_str::<Event>(&s).unwrap(), e);
    }

    #[test]
    fn legacy_lowercase_tags_still_parse() {
        // Historical writers used `rename_all = "lowercase"`. snake_case
        // and lowercase are identical for the single-word legacy tags, so
        // old logs must still decode. The Add/Sync variants are unchanged
        // by the v1.3.1 schema v2 hard-cut so a v1 record decoded today
        // remains valid; only the Action* variants gained `id`/dropped
        // `pack`/added `schema_version`.
        let raw = r#"{"op":"add","ts":"2026-04-19T10:00:00Z","id":"a","url":"u","path":"a","type":"declarative","schema_version":"1"}"#;
        let _: Event = serde_json::from_str(raw).unwrap();
        let raw = r#"{"op":"sync","ts":"2026-04-19T10:00:00Z","id":"a","sha":"deadbeef"}"#;
        let _: Event = serde_json::from_str(raw).unwrap();
    }
}
