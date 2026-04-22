//! Error mapping from grex-core failure types to MCP error envelopes.
//!
//! Two distinct error surfaces co-exist:
//!
//! 1. **JSON-RPC envelope errors** — returned as `Err(ErrorData)` from a
//!    handler. The dispatcher emits a top-level JSON-RPC error response.
//!    Used for: cancellation (-32800), bad params (-32602), and rmcp-state
//!    errors like "not initialized" (-32002, kind=`init_state`).
//!
//! 2. **Tool-level failures inside `CallToolResult`** — `Ok(CallToolResult
//!    { isError: Some(true), content: [...] })`. The handler completed; the
//!    domain operation failed. Used for grex pack-op failures (-32002,
//!    kind=`pack_op`) and the spec's manifest / lock / drift / plugin-
//!    missing codes.
//!
//! Per `.omne/cfg/mcp.md` §"Error codes" `-32002` is dual-use; the `data.kind`
//! discriminator disambiguates. Splitting into two codes is a documented
//! future item — not in this change.

use grex_core::Cancelled;
use rmcp::{
    model::{CallToolResult, Content, ErrorCode},
    ErrorData,
};
use serde_json::{json, Value};

// ── JSON-RPC reserved codes ─────────────────────────────────────────────

/// MCP "Request cancelled" per the 2025-06-18 specification.
pub const REQUEST_CANCELLED: i32 = -32800;

/// JSON-RPC "Invalid params" per the 2.0 spec.
pub const INVALID_PARAMS: i32 = -32602;

// ── grex error-code surface (spec §"Error codes") ───────────────────────

/// Manifest read / parse / write failure.
pub const MANIFEST_ERROR: i32 = -32001;

/// Pack-op or init-state failure (dual-use, see file header).
pub const POLICY_ERROR: i32 = -32002;

/// Pack-lock acquisition failure.
pub const LOCK_ERROR: i32 = -32003;

/// Lockfile drift detected.
pub const DRIFT_ERROR: i32 = -32004;

/// Plugin-missing failure (action or pack-type).
pub const PLUGIN_MISSING: i32 = -32005;

// ── Cancellation → -32800 (envelope) ────────────────────────────────────

/// Convert grex-core's [`Cancelled`] sentinel into an MCP envelope.
impl From<CancelledExt> for ErrorData {
    fn from(_: CancelledExt) -> Self {
        ErrorData::new(ErrorCode(REQUEST_CANCELLED), "request cancelled", None)
    }
}

/// New-type wrapper because `Cancelled` is foreign and we cannot
/// `impl From<Cancelled> for ErrorData` directly (orphan rule).
#[derive(Debug, Clone, Copy, Default)]
pub struct CancelledExt;

impl From<Cancelled> for CancelledExt {
    fn from(_: Cancelled) -> Self {
        CancelledExt
    }
}

// ── Pack-op failure → -32002 inside CallToolResult ──────────────────────

/// Build a `CallToolResult { isError: true }` carrying a structured
/// error description with `data.kind = "pack_op"` and code `-32002`.
///
/// The MCP spec puts tool-domain failures inside the result envelope
/// rather than at the JSON-RPC layer. We attach the code + kind in
/// the `Content::text` body as serialised JSON so agents can parse
/// without an out-of-band channel.
pub fn packop_error(message: &str) -> CallToolResult {
    let body = json!({
        "code": POLICY_ERROR,
        "data": { "kind": "pack_op" },
        "message": message,
    });
    CallToolResult::error(vec![Content::text(body.to_string())])
}

/// Build an `ErrorData` for an init-state failure (-32002, kind=
/// `init_state`). Used at the JSON-RPC envelope layer when a request
/// arrives before `initialize` has completed.
pub fn init_state_error(message: impl Into<String>) -> ErrorData {
    let data: Value = json!({ "kind": "init_state" });
    ErrorData::new(ErrorCode(POLICY_ERROR), message.into(), Some(data))
}

/// Build a `CallToolResult { isError: true }` for the "verb not yet
/// implemented" stub path used by Stage 6's nine stub verbs.
///
/// Choice (per `feat-m7-1-mcp-server/tasks.md` Stage 6 Option A):
/// stub verbs are advertised in `tools/list` so agents can discover the
/// surface, but every call returns an isError envelope with code
/// `-32601 Method Not Implemented` and a human-readable hint. Swap to
/// real impls is local in M7-4.
pub fn not_implemented_result(verb: &str) -> CallToolResult {
    let body = json!({
        "code": -32601,
        "data": { "kind": "not_implemented" },
        "message": format!("verb `{verb}` not yet implemented in M7-1; planned for M7-4"),
    });
    CallToolResult::error(vec![Content::text(body.to_string())])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 6.T7 — preserved from Stage 5; cancellation still maps to -32800.
    #[test]
    fn cancelled_maps_to_minus_32800() {
        let err: ErrorData = CancelledExt::from(Cancelled).into();
        assert_eq!(err.code.0, REQUEST_CANCELLED);
        assert!(err.message.contains("cancel"));
    }

    /// 6.T5 — pack-op failures go inside `CallToolResult { isError }`
    /// with code `-32002` and `data.kind = "pack_op"`.
    #[test]
    fn packop_failure_maps_to_minus_32002_with_kind_pack_op() {
        let r = packop_error("disk full");
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().expect("content").as_text().expect("text").text.clone();
        let v: Value = serde_json::from_str(&text).expect("payload is JSON");
        assert_eq!(v["code"], json!(POLICY_ERROR));
        assert_eq!(v["data"]["kind"], json!("pack_op"));
        assert!(v["message"].as_str().unwrap().contains("disk full"));
    }

    /// 6.T6 — init-state failures live at the JSON-RPC envelope layer
    /// (`Err(ErrorData)`) with code `-32002` and `data.kind = "init_state"`.
    #[test]
    fn init_state_failure_maps_to_minus_32002_with_kind_init_state() {
        let err = init_state_error("not initialised");
        assert_eq!(err.code.0, POLICY_ERROR);
        let data = err.data.as_ref().expect("data attached");
        assert_eq!(data["kind"], json!("init_state"));
    }

    #[test]
    fn not_implemented_envelope_carries_minus_32601_and_kind() {
        let r = not_implemented_result("ls");
        assert_eq!(r.is_error, Some(true));
        let text = r.content.first().expect("content").as_text().expect("text").text.clone();
        let v: Value = serde_json::from_str(&text).expect("payload is JSON");
        assert_eq!(v["code"], json!(-32601));
        assert_eq!(v["data"]["kind"], json!("not_implemented"));
        assert!(v["message"].as_str().unwrap().contains("ls"));
    }
}
