//! Shared fixtures + helpers for `grex-mcp` integration tests (L2 – L5).
//!
//! Owned by `feat-m7-2-mcp-test-harness`. Stage 2 lands the real
//! [`new_duplex_server`] + [`Client`] impl driving a duplex `GrexMcpServer`
//! over the same newline-delimited JSON-RPC framer production uses
//! (`rmcp` `transport-io`, see `rmcp::transport::async_rw::JsonRpcMessageCodec`).
//!
//! Layer ownership map:
//! - L2 duplex E2E         → `tests/handshake.rs`
//! - L2 real-pipe per-OS   → `tests/real_pipe_{linux,macos,windows}.rs`
//! - L3 CLI/MCP parity     → `tests/parity.rs`
//! - L4 concurrent stress  → `tests/stress.rs`
//! - L5 cancellation chaos → `tests/cancel.rs`
//!
//! Stage 4 lands `normalize`, `run_cli_json`, `run_mcp_tool`. Stage 1 / 2
//! cover the duplex transport + JSON-RPC line client only.
//!
//! Cross-test dead-code is expected during stage progression: each new
//! test file (handshake → parity → stress → cancel) brings additional
//! consumers of these helpers, so symbols added in early stages may
//! appear unused until later stages land. The `#[allow(dead_code)]` on
//! the module suppresses the noise without masking real drift.

#![allow(dead_code)]

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use grex_mcp::{GrexMcpServer, ServerState};
use serde_json::{json, Value};
use tempfile::TempDir;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf};
use tokio::task::JoinHandle;

/// MCP protocol version pinned at the wire boundary (matches
/// `.omne/cfg/mcp.md` §"Protocol version" and `feat-m7-1` Stage 5).
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Per-call response timeout. Kept low so the L2 suite fails fast under
/// regression rather than hanging the runner for the default 60 s.
const RECV_TIMEOUT: Duration = Duration::from_secs(2);

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

/// Construct a client/server duplex pair driven by the same newline-delimited
/// JSON-RPC framer production uses (`rmcp` `transport-io`), returning a
/// [`Client`] handle the test owns end-to-end.
///
/// Implementation notes:
///
/// 1. `tokio::io::duplex(4096)` hands back a pair of in-memory pipes; one
///    half (`server_io`) is fed straight into `GrexMcpServer::run` via the
///    blanket `IntoTransport for AsyncRead+AsyncWrite` impl in
///    `rmcp::transport::async_rw`. The other half (`client_io`) is split
///    into `(read, write)` so the [`Client`] can drive the JSON-RPC line
///    protocol directly without pulling in the rmcp typed-message types
///    (the L3 / L4 / L5 suites assert on raw envelopes).
/// 2. `ServerState::for_tests()` reuses the m7-1 helper that produces a
///    workspace-rooted `Scheduler::new(1)` + empty `Registry`. Stage 4
///    will swap in a fixture-aware constructor when parity tests need
///    real pack data; Stage 2's handshake suite never reaches handler
///    bodies so the default suffices.
/// 3. The server `JoinHandle` is captured inside [`Client`] so the test
///    can `await` it on `shutdown()` and surface any panic.
pub fn new_duplex_server(_fixture: &TestFixture) -> Client {
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = GrexMcpServer::new(ServerState::for_tests());
    let server_task = tokio::spawn(async move {
        // Discard the rmcp `ServerInitializeError` — the L2 tests
        // assert on transport-close behaviour via `JoinHandle`, not on
        // the rmcp error variant. m7-1 Stage 5 already covers panic-free
        // close at the typed-layer.
        let _ = server.run(server_io).await;
    });

    let (read, write) = tokio::io::split(client_io);
    Client {
        reader: BufReader::new(read),
        writer: write,
        next_id: AtomicI64::new(1),
        server_task: Some(server_task),
    }
}

/// Thin newline-delimited JSON-RPC line writer/reader over a duplex
/// transport half.
///
/// The duplex framer used by `rmcp` `transport-io` is
/// `JsonRpcMessageCodec`: each frame is one JSON object terminated by
/// `\n` (with an optional `\r` stripped on decode). This client
/// mirrors that exactly — `serde_json::to_string` + `\n`, line-buffered
/// reader on the way back. No length prefix, no Content-Length header.
///
/// Held opaque so test files only see the high-level methods; the L3
/// stress + L5 cancel suites will reuse this without leaking transport
/// internals into their assertions.
pub struct Client {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    next_id: AtomicI64,
    /// Server task — captured so [`Client::shutdown`] can join it after
    /// dropping the writer half (transport close = MCP shutdown signal).
    server_task: Option<JoinHandle<()>>,
}

impl Client {
    /// Allocate a fresh JSON-RPC request id. Monotonic, never wraps in
    /// any sane test horizon (we'd need 2^63 calls in one suite).
    fn next_request_id(&self) -> i64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Send `initialize` carrying the MCP `2025-06-18` protocol version
    /// and return the JSON-RPC envelope (so tests can assert on either
    /// `result.protocolVersion` or `error.code`).
    pub async fn initialize(&mut self) -> Value {
        let id = self.next_request_id();
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": { "name": "grex-mcp-l2-harness", "version": "0.0.1" }
            }
        });
        self.send_frame(&req).await;
        self.recv_frame_for_id(id).await
    }

    /// Send a `notifications/<method>` JSON-RPC notification (no id, no
    /// reply expected).
    ///
    /// `method` is the bare suffix (`"initialized"`, `"cancelled"`); the
    /// `notifications/` prefix is added here so tests can stay terse and
    /// match the spec's verb-only naming.
    pub async fn notify(&mut self, method: &str, params: Value) {
        debug_assert!(
            !method.starts_with("notifications/"),
            "pass bare method name (e.g. \"initialized\"); notify() prepends notifications/ — passed: {method}",
        );
        let req = json!({
            "jsonrpc": "2.0",
            "method": format!("notifications/{method}"),
            "params": params,
        });
        self.send_frame(&req).await;
    }

    /// Send a request and await the JSON-RPC envelope. Returns the full
    /// envelope so tests can inspect either `result` (success) or
    /// `error` (cancellation, init-state violation, invalid params).
    ///
    /// `method` is the JSON-RPC method as-is (`"tools/list"`,
    /// `"initialize"`, `"tools/call"`); no prefix munging.
    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_request_id();
        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        self.send_frame(&req).await;
        self.recv_frame_for_id(id).await
    }

    /// Drop the client end of the duplex transport and wait for the
    /// server task to finish. MCP 2025-06-18 has no explicit `shutdown`
    /// JSON-RPC method; transport close IS the shutdown signal (see
    /// m7-1 `tests/handshake.rs::shutdown_returns_then_closes`).
    pub async fn shutdown(mut self) {
        // Flush any pending writes before tearing down. Best-effort —
        // a closed writer just means the test already drove cleanup.
        let _ = self.writer.shutdown().await;
        // Drop both halves so the server's framed reader sees EOF and
        // unwinds its serve loop.
        drop(self.writer);
        drop(self.reader);

        if let Some(task) = self.server_task.take() {
            // 2 s budget mirrors m7-1 Stage 5's `shutdown_returns_then_closes`
            // (which uses 500 ms); we widen to 2 s here because L2 cases
            // may have an in-flight handler draining (graceful_shutdown_drains).
            let _ = tokio::time::timeout(Duration::from_secs(2), task).await;
        }
    }

    /// Encode `frame` as one newline-terminated JSON line and write it
    /// to the duplex transport. Any I/O error panics — the L2 suite
    /// treats a closed transport as a test failure.
    async fn send_frame(&mut self, frame: &Value) {
        let mut buf = serde_json::to_vec(frame).expect("serialise JSON-RPC frame");
        buf.push(b'\n');
        self.writer.write_all(&buf).await.expect("write JSON-RPC frame to duplex");
        self.writer.flush().await.expect("flush JSON-RPC frame to duplex");
    }

    /// Read frames until one whose `id` matches `expected_id` is found.
    /// Server-initiated requests / unrelated notifications are skipped
    /// (the L2 cases do not assert on them). Times out per
    /// [`RECV_TIMEOUT`].
    async fn recv_frame_for_id(&mut self, expected_id: i64) -> Value {
        let deadline = tokio::time::Instant::now() + RECV_TIMEOUT;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let frame =
                tokio::time::timeout(remaining, self.recv_frame()).await.unwrap_or_else(|_| {
                    panic!(
                        "timed out after {:?} waiting for JSON-RPC response id={expected_id}",
                        RECV_TIMEOUT
                    )
                });
            // Match either the success envelope (`{result, id}`) or the
            // error envelope (`{error, id}`). Server requests / notifications
            // (no `id` or non-matching `id`) are dropped.
            if let Some(id) = frame.get("id").and_then(Value::as_i64) {
                if id == expected_id {
                    return frame;
                }
            }
        }
    }

    /// Read one newline-delimited JSON frame from the duplex reader.
    /// EOF on a closed transport yields a panic — tests close the
    /// transport via [`shutdown`], which never expects another reply.
    async fn recv_frame(&mut self) -> Value {
        let mut line = String::new();
        let n = self.reader.read_line(&mut line).await.expect("read JSON-RPC line from duplex");
        assert!(n > 0, "duplex EOF before JSON-RPC reply arrived");
        // `read_line` retains the trailing `\n`; rmcp's encoder also
        // emits `\n` only (no `\r\n`). Trim either to be defensive.
        let trimmed = line.trim_end_matches(['\n', '\r']);
        serde_json::from_str(trimmed)
            .unwrap_or_else(|e| panic!("malformed JSON-RPC frame {trimmed:?}: {e}"))
    }
}

// ============================================================================
// L3 normaliser — Stage 4 of feat-m7-2
// ============================================================================

/// Recursively rewrite a JSON value, substituting volatile string scalars
/// with stable placeholder tokens so two structurally-equivalent payloads
/// (one from `grex --json`, one from `tools/call`) compare byte-equal.
///
/// Substitutions performed (string scalars only):
///
/// - **`<TS>`** — any string parseable as RFC3339
///   (`chrono::DateTime::parse_from_rfc3339`).
/// - **`<PATH>`** — any string shaped like an absolute filesystem path:
///     - Unix: leading `/` followed by a non-whitespace remainder.
///     - Windows: drive letter + colon + `/` or `\` followed by a
///       non-whitespace remainder.
///
/// Recurses through objects (preserving keys, normalising values) and
/// arrays (normalising elements). Non-string scalars (`Number`, `Bool`,
/// `Null`) pass through unchanged. Numeric timestamps are intentionally
/// NOT rewritten — the leaf-level normaliser cannot see field-key
/// context, and MCP envelope shapes carry timestamps as RFC3339 strings.
///
/// **Lossy by design**: distinct absolute paths collapse to the single
/// `<PATH>` token. Acceptable for parity assertions where the *shape*
/// of the response, not the literal path, is what matters. If Stage 5
/// parity tests need fixture-root-relative paths, extend with a
/// `normalize_with_root(value, root)` companion — do NOT widen this
/// helper's semantics.
///
/// Per spec §L3 (`openspec/changes/feat-m7-2-mcp-test-harness/spec.md`)
/// the placeholder set is deliberately minimal: `<TS>` + `<PATH>` only.
/// `<ID>`, `<PID>`, `<SHA>` are explicitly out-of-scope until a concrete
/// failing parity test proves the need.
pub fn normalize(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(normalize_string(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(normalize).collect()),
        Value::Object(map) => {
            let mut out = serde_json::Map::with_capacity(map.len());
            for (k, v) in map {
                out.insert(k, normalize(v));
            }
            Value::Object(out)
        }
        // Numbers, bools, null: pass-through. See doc-comment rationale.
        other => other,
    }
}

/// Classify a string scalar and return the rewritten form. Order matters:
/// timestamp check first (RFC3339 strings never look like absolute paths),
/// then absolute-path shape check, then identity.
fn normalize_string(s: &str) -> String {
    if chrono::DateTime::parse_from_rfc3339(s).is_ok() {
        return "<TS>".to_string();
    }
    if is_absolute_path_shaped(s) {
        return "<PATH>".to_string();
    }
    s.to_string()
}

/// True if `s` looks like an absolute filesystem path. Checks Unix-style
/// (`/...`) and Windows-style (`C:\...` / `c:/...`). Requires at least
/// one non-whitespace character after the prefix to avoid matching bare
/// separators or short fragments like `/` or `C:\`.
fn is_absolute_path_shaped(s: &str) -> bool {
    let bytes = s.as_bytes();
    // Unix: starts with `/`, has more non-whitespace content after.
    if bytes.first() == Some(&b'/') && bytes.len() > 1 && !bytes[1].is_ascii_whitespace() {
        return true;
    }
    // Windows: `[A-Za-z]:[/\\]<non-ws>...`
    if bytes.len() >= 4
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'/' || bytes[2] == b'\\')
        && !bytes[3].is_ascii_whitespace()
    {
        return true;
    }
    false
}

#[cfg(test)]
mod normalize_tests {
    use super::normalize;
    use serde_json::json;

    #[test]
    fn timestamp_rewrite() {
        let input = json!("2026-04-21T12:34:56Z");
        assert_eq!(normalize(input), json!("<TS>"));

        let with_offset = json!("2026-04-21T12:34:56.789+02:00");
        assert_eq!(normalize(with_offset), json!("<TS>"));
    }

    #[test]
    fn absolute_path_rewrite() {
        assert_eq!(normalize(json!("/home/user/grex/pack.toml")), json!("<PATH>"));
        assert_eq!(normalize(json!("C:\\Users\\egois\\grex\\pack.toml")), json!("<PATH>"));
        assert_eq!(normalize(json!("D:/repos/grex/pack.toml")), json!("<PATH>"));
    }

    #[test]
    fn nested_object_rewrite() {
        let input = json!({
            "pack": {
                "path": "/var/lib/grex/p1",
                "last_sync": "2026-04-21T10:00:00Z",
                "name": "p1",
            },
            "count": 3,
        });
        let expected = json!({
            "pack": {
                "path": "<PATH>",
                "last_sync": "<TS>",
                "name": "p1",
            },
            "count": 3,
        });
        assert_eq!(normalize(input), expected);
    }

    #[test]
    fn no_op_on_scalars() {
        assert_eq!(normalize(json!("plain string")), json!("plain string"));
        assert_eq!(normalize(json!(42)), json!(42));
        assert_eq!(normalize(json!(2.5)), json!(2.5));
        assert_eq!(normalize(json!(true)), json!(true));
        assert_eq!(normalize(json!(null)), json!(null));
        // Numeric epoch-shaped values are intentionally NOT rewritten —
        // see `normalize` doc-comment (no leaf-level field-key context).
        assert_eq!(normalize(json!(1_745_236_496_u64)), json!(1_745_236_496_u64));
    }

    #[test]
    fn mixed_content() {
        let input = json!({
            "results": [
                { "path": "/tmp/a", "ts": "2026-04-21T00:00:00Z", "ok": true },
                { "path": "C:\\tmp\\b", "ts": "2026-04-21T00:00:01Z", "ok": false },
                { "path": "relative/path.toml", "ts": "not-a-timestamp", "ok": true },
            ],
            "version": "1.2.3",
        });
        let expected = json!({
            "results": [
                { "path": "<PATH>", "ts": "<TS>", "ok": true },
                { "path": "<PATH>", "ts": "<TS>", "ok": false },
                { "path": "relative/path.toml", "ts": "not-a-timestamp", "ok": true },
            ],
            "version": "1.2.3",
        });
        assert_eq!(normalize(input), expected);
    }

    #[test]
    fn idempotent() {
        // normalize(normalize(x)) == normalize(x). Critical because
        // parity tests will repeatedly compare already-normalised values;
        // any second-pass drift would mask regressions.
        let input = json!({
            "a": "/usr/bin/grex",
            "b": "2026-01-01T00:00:00Z",
            "c": [
                "<PATH>",
                "<TS>",
                { "nested": "/etc/hosts", "when": "2026-04-21T00:00:00Z" },
            ],
            "d": "literal",
        });
        let once = normalize(input);
        let twice = normalize(once.clone());
        assert_eq!(once, twice);
    }
}
