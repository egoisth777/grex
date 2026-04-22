//! Shared fixtures + helpers for `grex-mcp` integration tests (L2 – L5).
//!
//! Owned by `feat-m7-2-mcp-test-harness`. **Stage 1 = scaffolding only** —
//! every public symbol here is intentionally `unimplemented!()` so the
//! L2 handshake tests in `tests/handshake.rs` go RED in a useful way
//! (compile-clean, panic at the helper boundary, not in the test body).
//!
//! Layer ownership map:
//! - L2 duplex E2E         → `tests/handshake.rs`
//! - L2 real-pipe per-OS   → `tests/real_pipe_{linux,macos,windows}.rs`
//! - L3 CLI/MCP parity     → `tests/parity.rs`
//! - L4 concurrent stress  → `tests/stress.rs`
//! - L5 cancellation chaos → `tests/cancel.rs`
//!
//! Stage 2 lands the real `new_duplex_server` + `Client` impl; Stage 4
//! lands `normalize`, `run_cli_json`, `run_mcp_tool`. Stage 1 only proves
//! the API shape compiles against the rest of the harness.
//!
//! Cross-test dead-code is expected during stage progression: each new
//! test file (handshake → parity → stress → cancel) brings additional
//! consumers of these helpers, so symbols added in early stages may
//! appear unused until later stages land. The `#[allow(dead_code)]` on
//! the module suppresses the noise without masking real drift.

#![allow(dead_code)]

use tempfile::TempDir;

/// A scratch workspace + manifest path pair, isolated per-test so destructive
/// verbs (`rm`, `run`, `exec`) cannot collide across parallel cargo-test
/// threads.
///
/// Stage 1: empty struct holding only the tempdir handle. Stage 2 wires
/// the manifest seed + workspace root into [`new_duplex_server`]. Stage 4
/// extends this with the seeded-pack count needed by [`run_cli_json`] /
/// [`run_mcp_tool`].
pub struct TestFixture {
    /// Held to keep the temp directory alive for the lifetime of the test.
    /// The drop order (fixture last) matters because [`Client`] in Stage 2
    /// will hold an `Arc<Path>` rooted here.
    pub workspace: TempDir,
}

impl TestFixture {
    /// Build a fresh, empty workspace tempdir. Each test gets its own.
    pub fn new() -> Self {
        let workspace = tempfile::tempdir().expect("create test workspace tempdir");
        Self { workspace }
    }
}

impl Default for TestFixture {
    fn default() -> Self {
        Self::new()
    }
}

/// Construct a client/server duplex pair driven by the same `rmcp`
/// `transport-io` framer production uses, returning a [`Client`] handle
/// the test owns end-to-end.
///
/// **Stage 1 stub.** Stage 2 (per `tasks.md` §2.1) replaces this with
/// the real implementation that:
///
/// 1. Pairs `tokio::io::duplex(4096)` into `(server_io, client_io)`.
/// 2. Spawns `GrexMcpServer::new(ServerState::for_tests()).run(server_io)`
///    on the current Tokio runtime.
/// 3. Wraps `client_io` in a thin JSON-RPC line writer/reader exposed
///    via [`Client`].
///
/// The intentional `unimplemented!()` is what drives the 5 L2 handshake
/// tests RED in Stage 1; Stage 2 closing the implementation flips them
/// GREEN with no test-body churn.
pub fn new_duplex_server(_fixture: &TestFixture) -> Client {
    unimplemented!(
        "stage-1 stub — `new_duplex_server` lands in feat-m7-2 stage 2 \
         (see openspec/changes/feat-m7-2-mcp-test-harness/tasks.md §2.1)"
    )
}

/// Thin JSON-RPC line writer/reader over a duplex transport half.
///
/// **Stage 1 stub.** Stage 2 (per `tasks.md` §2.2) implements
/// `initialize`, `notify`, `call`, `shutdown` against `serde_json::Value`
/// payloads. The struct stays empty here so the L2 handshake tests can
/// reference the type without dragging in the rmcp wire types directly.
pub struct Client {
    // Stage 2 fields land here: a duplex half + a request-id counter.
    // Kept opaque so test files only see the high-level methods.
    _stage_2_placeholder: (),
}

impl Client {
    /// Send `initialize` carrying the MCP `2025-06-18` protocol version
    /// and return the framework's `result` payload (the inner JSON-RPC
    /// `result` object, not the envelope).
    pub async fn initialize(&mut self) -> serde_json::Value {
        unimplemented!("stage-1 stub — `Client::initialize` lands in stage 2 §2.2")
    }

    /// Send a `notifications/<method>` JSON-RPC notification (no id, no
    /// reply expected).
    pub async fn notify(&mut self, _method: &str, _params: serde_json::Value) {
        unimplemented!("stage-1 stub — `Client::notify` lands in stage 2 §2.2")
    }

    /// Send a `tools/call` (or arbitrary `method`) request and await the
    /// JSON-RPC envelope. Returns the full envelope so tests can inspect
    /// either `result` (success) or `error` (cancellation, invalid params).
    pub async fn call(&mut self, _method: &str, _params: serde_json::Value) -> serde_json::Value {
        unimplemented!("stage-1 stub — `Client::call` lands in stage 2 §2.2")
    }

    /// Drop the client end of the duplex transport. MCP 2025-06-18 has
    /// no explicit `shutdown` JSON-RPC method; transport close IS the
    /// shutdown signal (see `tests/handshake.rs` Stage 5 m7-1 docs).
    pub async fn shutdown(self) {
        unimplemented!("stage-1 stub — `Client::shutdown` lands in stage 2 §2.2")
    }
}
