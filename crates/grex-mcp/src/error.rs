//! Error mapping from grex-core failure types to MCP `ErrorData` envelopes.
//!
//! Stage 5 lands only the `Cancelled → -32800` mapping; Stages 6/7 extend this
//! file with pack-op (-32002), init-state (-32002), manifest (-32001), lock
//! (-32003), drift (-32004), and plugin-missing (-32005) codes per the spec.
//!
//! Code `-32800` is the MCP-reserved "request cancelled" code (per the
//! 2025-06-18 spec); it MUST be returned for any request aborted because the
//! caller (or the transport close) fired the request's cancellation token.

use grex_core::Cancelled;
use rmcp::ErrorData;

/// MCP "Request cancelled" error code per the 2025-06-18 specification.
pub const REQUEST_CANCELLED: i32 = -32800;

/// Convert grex-core's [`Cancelled`] sentinel into an MCP error envelope.
///
/// Used by Stage-7 cancellation wiring; Stage 5 ships the conversion so the
/// public surface is stable from the start.
impl From<CancelledExt> for ErrorData {
    fn from(_: CancelledExt) -> Self {
        ErrorData::new(
            rmcp::model::ErrorCode(REQUEST_CANCELLED),
            "request cancelled",
            None,
        )
    }
}

/// New-type wrapper because `Cancelled` is a foreign type and we cannot
/// `impl From<Cancelled> for ErrorData` directly (orphan rule). Stage 7
/// uses this at the tool-handler boundary.
#[derive(Debug, Clone, Copy, Default)]
pub struct CancelledExt;

impl From<Cancelled> for CancelledExt {
    fn from(_: Cancelled) -> Self {
        CancelledExt
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Stage 5: pre-wire the conversion so Stage 7 can pull it in unchanged.
    #[test]
    fn cancelled_converts_to_minus_32800() {
        let err: ErrorData = CancelledExt::from(Cancelled).into();
        assert_eq!(err.code.0, REQUEST_CANCELLED);
        assert!(err.message.contains("cancel"));
    }
}
