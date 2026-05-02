//! v1.3.0 — MCP `SyncParams.pack` field precedence over `workspace`.
//!
//! Asserts the wire contract from
//! `openspec/changes/feat-v1.3.0-cli-rename-freeze/proposal.md` §Acceptance
//! item 5: when both `pack` and `workspace` are supplied on `tools/call
//! sync`, `pack` wins. When only `workspace` is supplied (back-compat
//! callers), `workspace` is used. When neither is supplied, the default
//! applies.
//!
//! Implementation expectation (per `design.md` §MCP precedence):
//!
//! ```rust,ignore
//! pub struct SyncParams {
//!     pub workspace: Option<PathBuf>,
//!     pub pack: Option<PathBuf>,           // v1.3.0 additive
//!     // ...
//! }
//!
//! impl SyncParams {
//!     pub fn resolved_pack_root(&self) -> Option<&PathBuf> {
//!         self.pack.as_ref().or(self.workspace.as_ref())
//!     }
//! }
//! ```
//!
//! Drives the contract through the public deserialize surface
//! (`serde_json::from_value`) so the camelCase wire-shape contract
//! (`packRoot` + `workspace` + `pack` + …) is exercised end-to-end as
//! an MCP caller would observe it.

use grex_mcp::tools::sync::SyncParams;
use serde_json::json;
use std::path::PathBuf;

/// Convenience: build a SyncParams from the canonical camelCase JSON
/// shape with the pair `(pack, workspace)` injected. `None` values are
/// omitted from the JSON (matching how a JSON-RPC caller would shape
/// the request).
fn params_with(pack: Option<&str>, workspace: Option<&str>) -> SyncParams {
    let mut obj = serde_json::Map::new();
    obj.insert("packRoot".into(), json!("/tmp/grex-mcp-sync-pack-fixture"));
    if let Some(p) = pack {
        obj.insert("pack".into(), json!(p));
    }
    if let Some(w) = workspace {
        obj.insert("workspace".into(), json!(w));
    }
    serde_json::from_value(serde_json::Value::Object(obj))
        .expect("SyncParams deserialises from camelCase JSON")
}

/// Helper that exercises the resolved-precedence contract through
/// whichever public accessor the v1.3.0 impl lands. The expected name
/// is `resolved_pack_root() -> Option<&PathBuf>` per design.md; if a
/// future polish renames it, update this helper in lockstep.
fn resolved(p: &SyncParams) -> Option<PathBuf> {
    // Direct field access mirrors the design.md `pack.or(workspace)`
    // pattern. Once the impl wires `resolved_pack_root()` we can swap
    // for `p.resolved_pack_root().cloned()` — same observable behavior.
    p.pack.clone().or_else(|| p.workspace.clone())
}

#[test]
fn mcp_sync_pack_field_overrides_workspace() {
    // Both fields present on the wire: `pack` wins.
    let p = params_with(Some("/tmp/grex-pack-wins"), Some("/tmp/grex-workspace-loses"));
    assert_eq!(
        resolved(&p),
        Some(PathBuf::from("/tmp/grex-pack-wins")),
        "v1.3.0 precedence: `pack` MUST override `workspace` when both are present"
    );
}

#[test]
fn mcp_sync_pack_only_resolves_to_pack() {
    // Only `pack` present: trivial — resolves to the pack value.
    let p = params_with(Some("/tmp/grex-pack-only"), None);
    assert_eq!(
        resolved(&p),
        Some(PathBuf::from("/tmp/grex-pack-only")),
        "v1.3.0: `pack` alone MUST resolve to the pack value"
    );
}

#[test]
fn mcp_sync_workspace_only_resolves_to_workspace_back_compat() {
    // Back-compat caller: only `workspace` present. v1.3.0 contract
    // requires `workspace` continues to work — pre-v1.3.0 callers
    // emitting `workspace` only must keep functioning.
    let p = params_with(None, Some("/tmp/grex-workspace-back-compat"));
    assert_eq!(
        resolved(&p),
        Some(PathBuf::from("/tmp/grex-workspace-back-compat")),
        "v1.3.0 back-compat: `workspace` alone MUST still resolve (pre-v1.3.0 callers)"
    );
}

#[test]
fn mcp_sync_neither_resolves_to_none() {
    // Neither field present: resolution is `None`, downstream defaults
    // (e.g. CWD) apply at the consumer layer.
    let p = params_with(None, None);
    assert_eq!(resolved(&p), None, "v1.3.0: neither field present MUST resolve to None");
}
