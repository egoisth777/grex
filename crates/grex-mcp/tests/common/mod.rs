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

// ============================================================================
// L3 parity helpers — Stage 5 of feat-m7-2
// ============================================================================
//
// Pragmatic parity contract (see `tests/parity.rs` module-doc for the long
// form). Spec line 98 calls for `assert_eq!(normalize(cli_json),
// normalize(mcp_json))` — strict byte-equal of two normalised JSON
// payloads. That cannot be satisfied today because `crates/grex/src/cli/
// args.rs::GlobalFlags::json` is parsed but not consumed by any verb's
// `run()`. None of the 11 CLI verbs emits JSON; 9 print
// `"grex <verb>: unimplemented (M1 scaffold)"`, `sync` (the one real
// impl) prints `[ok]/[would]/[skipped] pack=... action=... idx=...` text.
//
// Until CLI `--json` wiring lands (post-m7-4), parity asserts the
// **structural shape** both surfaces DO carry: each verb is observably
// in an "M7-1-stub" state — CLI text contains `unimplemented` or fails
// non-zero; MCP returns `CallToolResult { isError: true }` whose body
// parses as a JSON envelope with `data.kind` ∈ {`not_implemented`,
// `pack_op`}. Call sites stay unchanged when `assert_parity` flips to
// the spec-shaped strict byte-equal in m7-4.

use std::process::Stdio;

use assert_cmd::cargo::CommandCargoExt as _;

/// Per-surface parity signal observed in the wild today.
///
/// Both surfaces emit one of these for every verb in `VERBS_EXPOSED`.
/// Strict byte-equal of CLI JSON vs MCP JSON awaits CLI `--json` wiring;
/// see module-level comment block above for rationale.
#[derive(Debug, PartialEq, Eq)]
pub enum ParitySignal {
    /// Verb is an M7-1 stub. CLI prints `"unimplemented"` text; MCP
    /// returns the spec-shaped `not_implemented` envelope.
    Unimplemented,
    /// Verb dispatched into `grex_core` and surfaced a domain-level
    /// error (e.g. missing pack root for `sync`). CLI exits non-zero
    /// with stderr text; MCP returns `packop_error(...)`.
    PackOpError,
    /// Verb dispatched into `grex_core`, completed cleanly, and emitted
    /// a structured report. CLI exits zero; MCP returns a
    /// `CallToolResult::success`. Used by the M8-7 `doctor` wiring
    /// where an empty-workspace run is the happy path.
    Success,
}

/// CLI argv for every verb that drives `assert_parity` into a
/// **deterministic** outcome (no flake on host filesystem state).
///
/// `sync` gets a `fixture`-rooted absolute path that does not exist as a
/// `.grex/pack.yaml` — both surfaces dispatch into `sync::run`, both
/// surface a structural error. Every other verb is a stub that prints
/// `"unimplemented"` from `cli/verbs/<verb>.rs::run()` (search the file
/// for the exact string) regardless of args. Passing the fixture in
/// (instead of a bare relative path) keeps the CLI subprocess from
/// creating stray dirs in the test runner's cwd — a real footgun
/// observed during Stage 5 first run (`crates/grex-mcp/grex-mcp-parity-
/// nonexistent/` was being created next to `Cargo.toml`).
///
/// Returns owned `Vec<String>` so the caller can extend at the call
/// site without lifetime gymnastics. Empty `Vec` = no extra args.
#[must_use]
pub fn default_args_for(verb: &str, fixture: &TestFixture) -> Vec<String> {
    match verb {
        // `add` requires a positional URL; pass a deterministic dummy.
        "add" => vec!["https://example.invalid/parity-fixture.git".to_string()],
        // `rm` requires a positional path.
        "rm" => vec!["nonexistent-pack".to_string()],
        // `run` requires a positional action name.
        "run" => vec!["parity-fixture-action".to_string()],
        // `exec` collects trailing args; pass one so clap-parse succeeds.
        "exec" => vec!["true".to_string()],
        // `sync` needs an absolute path inside the per-test tempdir so
        // the CLI's `pack_root.is_none()` legacy-stub branch is not
        // taken AND the runner cwd never gets polluted. The path
        // intentionally points at a sub-dir of the fixture that does
        // NOT exist (no `.grex/pack.yaml` seeded), so `sync::run`
        // surfaces a structural error on both surfaces.
        "sync" => vec![sync_pack_root(fixture).to_string_lossy().into_owned()],
        // Every other verb is a bare stub — no args needed for the parse
        // path or the print path.
        _ => Vec::new(),
    }
}

/// MCP `tools/call` arguments matching [`default_args_for`] in shape so
/// both surfaces dispatch through equivalent code paths.
///
/// The MCP-side `Params` structs (in `crates/grex-mcp/src/tools/<verb>.rs`)
/// use `serde(deny_unknown_fields)`. Required fields per verb are pulled
/// from those `Params` structs verbatim — over-supplying yields
/// `-32602 Invalid params` and breaks the parity signal.
#[must_use]
pub fn default_mcp_params_for(verb: &str, fixture: &TestFixture) -> Value {
    match verb {
        "add" => json!({ "url": "https://example.invalid/parity-fixture.git" }),
        "rm" => json!({ "path": "nonexistent-pack" }),
        "run" => json!({ "action": "parity-fixture-action" }),
        "exec" => json!({ "cmd": ["true"] }),
        // `sync` requires `pack_root`. Use the same absolute fixture-
        // rooted path the CLI side gets so both surfaces hit
        // `sync::run` against the identical (non-existent) target and
        // produce the matching structural error envelope.
        "sync" => json!({
            "packRoot": sync_pack_root(fixture),
            "dryRun": true,
            "noValidate": true,
        }),
        // `doctor` + `import` parity drive through the field-level
        // helpers (`assert_parity_doctor_report`,
        // `assert_parity_import_plan`) — not `assert_parity`. These
        // entries stay bare so the `default_mcp_params_cover_all_verbs`
        // helper-test remains exhaustive.
        "doctor" => json!({}),
        "import" => json!({}),
        // `update` accepts an optional pack name; bare is fine.
        // All other stubs accept no required params.
        _ => json!({}),
    }
}

/// Per-fixture pack-root path used by both surfaces' `sync` parity
/// case. Sub-dir name is fixed (not random) so the CLI subprocess
/// argv and the MCP params dict remain string-equal — important
/// because the spec's `<PATH>` normaliser (Stage 4) collapses both to
/// `<PATH>` only after stage-5 byte-equal flips on; today the path
/// is just an input to `sync::run` and never appears in either output.
fn sync_pack_root(fixture: &TestFixture) -> std::path::PathBuf {
    fixture.workspace.path().join("parity-sync-pack-root")
}

/// Drive `grex <verb> [args...]` as a subprocess against the freshly-
/// built workspace `grex` binary; capture stdout + stderr + exit code
/// and classify into a [`ParitySignal`].
///
/// Today every verb either prints `"unimplemented"` to stdout (9 stubs +
/// `sync` without `pack_root`) OR exits non-zero with a structural error
/// to stderr (`sync` with `pack_root` pointing at a non-`.grex` path).
/// When CLI `--json` wiring lands the body parses as JSON; until then we
/// classify on text shape.
async fn drive_cli(verb: &str, args: &[String], fixture: &TestFixture) -> ParitySignal {
    // `cargo_bin` resolves to `target/debug/grex[.exe]`. `assert_cmd`
    // ensures it is built before the test runs (same wiring m7-1
    // serve_smoke uses).
    let mut cmd = std::process::Command::cargo_bin("grex").expect("grex binary builds");
    cmd.arg(verb);
    for a in args {
        cmd.arg(a);
    }
    // Pin the subprocess cwd to the fixture tempdir so verbs that
    // resolve paths relative to `current_dir()` (e.g. `doctor`,
    // `import` with the default manifest) stay deterministic and
    // cannot see the harness's real working tree. `sync` already
    // takes an absolute path argument so the cwd change is harmless.
    cmd.current_dir(fixture.workspace.path());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    // tokio-async wrapper around `std::process::Command::output()` keeps
    // the test future-friendly (parity runs are independent so no
    // throughput win, but the helper is `async fn` for symmetry with
    // `drive_mcp` and to leave room for cancellation budgets in m7-3).
    let output = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("spawn_blocking joins")
        .expect("CLI subprocess runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let _stderr = String::from_utf8_lossy(&output.stderr);

    if stdout.contains("unimplemented") {
        ParitySignal::Unimplemented
    } else if !output.status.success() {
        // Non-zero exit with no `"unimplemented"` marker → real domain
        // error path. `sync` (missing pack) and `import` (missing
        // `--from-repos-json`) reach here.
        ParitySignal::PackOpError
    } else {
        // Clean zero-exit with no "unimplemented" marker = real wired
        // verb that completed. Only `doctor` currently reaches here
        // (empty fixture workspace → all-OK report, exit 0). Future
        // wired verbs will follow suit.
        ParitySignal::Success
    }
}

/// Drive an MCP `tools/call` for `verb` with `params` against an
/// in-process duplex server; classify the result envelope into a
/// [`ParitySignal`].
///
/// Mirrors `drive_cli`'s classification: `not_implemented` envelope →
/// [`ParitySignal::Unimplemented`]; `pack_op` envelope →
/// [`ParitySignal::PackOpError`]. Anything else is a parity-helper
/// assumption violation worth a panic so the contract update lands in
/// the same diff as the surface change.
async fn drive_mcp(verb: &str, fixture: &TestFixture, params: Value) -> ParitySignal {
    let mut client = new_duplex_server(fixture);
    let _ = client.initialize().await;
    client.notify("initialized", json!({})).await;

    let resp = client.call("tools/call", json!({ "name": verb, "arguments": params })).await;
    client.shutdown().await;

    // Preflight from spec §"Tool enumeration": tools/list must advertise
    // `>= VERBS_EXPOSED.len()`. Keeping it inside the call path (as
    // tasks 5.4 directs) means a registry shrink fails `parity_*` first,
    // before the per-verb assertion even runs. Cheap (~one JSON
    // walk) and the diagnostic localises to the verb that tripped.
    {
        let mut probe = new_duplex_server(fixture);
        let _ = probe.initialize().await;
        probe.notify("initialized", json!({})).await;
        let tools = probe.call("tools/list", json!({})).await;
        probe.shutdown().await;
        let len = tools["result"]["tools"].as_array().map(Vec::len).unwrap_or(0);
        assert!(
            len >= grex_mcp::VERBS_EXPOSED.len(),
            "tools/list must expose at least {} tools, got {} (verb under test: {verb})",
            grex_mcp::VERBS_EXPOSED.len(),
            len,
        );
    }

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("MCP tools/call for `{verb}` returned no `result`: {resp:?}"));
    let is_error = result.get("isError").and_then(Value::as_bool).unwrap_or(false);

    // A wired verb that completed cleanly returns `isError: false` (or
    // the flag absent). We classify that as `Success` and let the
    // per-verb parity test assert against its CLI counterpart.
    if !is_error {
        return ParitySignal::Success;
    }

    let body_text =
        result.pointer("/content/0/text").and_then(Value::as_str).unwrap_or_else(|| {
            panic!("MCP isError envelope for `{verb}` lacks content[0].text: {result:?}")
        });
    let envelope: Value = serde_json::from_str(body_text)
        .unwrap_or_else(|e| panic!("MCP `{verb}` envelope is not JSON: {e}; body: {body_text:?}"));
    let kind = envelope
        .pointer("/data/kind")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("MCP `{verb}` envelope missing data.kind: {envelope:?}"));

    match kind {
        "not_implemented" => ParitySignal::Unimplemented,
        "pack_op" => ParitySignal::PackOpError,
        other => panic!(
            "MCP `{verb}` returned unknown data.kind={other:?} — parity-helper \
             assumption violated; extend `ParitySignal`. envelope: {envelope:?}"
        ),
    }
}

/// Drive both surfaces for `verb` and assert they signal the same
/// parity outcome.
///
/// **Today (m7-2)**: asserts both surfaces produce the same
/// [`ParitySignal`] (`Unimplemented` for 10 stubs, `PackOpError` for
/// `sync` against a missing pack root). This is the strongest contract
/// reachable without CLI `--json` wiring (see spec §"Known limitations"
/// entry 5 for the gap).
///
/// **Tomorrow (post-m7-4)**: flip the assertion to
/// `assert_eq!(normalize(cli_json), normalize(mcp_json))` per spec §L3.
/// Call sites in `tests/parity.rs` stay unchanged.
///
/// Owns the per-test [`TestFixture`] for the call's lifetime so the
/// `sync` case (which needs an absolute path inside the fixture
/// tempdir) cannot leak the workspace root past the assertion.
pub async fn assert_parity(verb: &str) {
    let fixture = TestFixture::new();
    let args = default_args_for(verb, &fixture);
    let mcp_params = default_mcp_params_for(verb, &fixture);
    let cli_signal = drive_cli(verb, &args, &fixture).await;
    let mcp_signal = drive_mcp(verb, &fixture, mcp_params).await;
    assert_eq!(
        cli_signal, mcp_signal,
        "CLI and MCP surfaces disagree on `{verb}` outcome — \
         CLI: {cli_signal:?}, MCP: {mcp_signal:?}"
    );
}

// ============================================================================
// L3 field-level parity — feat-m8-release blocker fix
// ============================================================================
//
// The `ParitySignal` scheme above only asserts outcome-class equivalence
// ("both surfaces signalled PackOpError" etc.). That is too weak for
// wired verbs like `doctor` + `import` — a schema divergence (renamed
// field, dropped wrapper, different label set) would still signal
// `Success` on both sides.
//
// These helpers drive both surfaces against a concrete fixture, parse
// both JSON outputs, and assert structural equivalence on the
// verb-specific shape. Timestamps + absolute paths are normalised via
// [`normalize`] so host-specific scalars collapse.

/// Drive CLI `grex <verb> --json <args...>` with cwd pinned to
/// `fixture.workspace` and return the parsed JSON stdout.
///
/// Panics if the subprocess does not produce valid JSON on stdout —
/// tests that expect failure should go through `drive_cli` instead.
async fn run_cli_json(verb: &str, args: &[String], fixture: &TestFixture) -> Value {
    let mut cmd = std::process::Command::cargo_bin("grex").expect("grex binary builds");
    cmd.arg("--json");
    cmd.arg(verb);
    for a in args {
        cmd.arg(a);
    }
    cmd.current_dir(fixture.workspace.path());
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());

    let output = tokio::task::spawn_blocking(move || cmd.output())
        .await
        .expect("spawn_blocking joins")
        .expect("CLI subprocess runs");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    serde_json::from_str(&stdout).unwrap_or_else(|e| {
        panic!(
            "CLI `{verb} --json` stdout is not JSON: {e}; status={:?}; stdout=<<{stdout}>>",
            output.status.code()
        )
    })
}

/// Drive MCP `tools/call` for `verb` against an in-process duplex
/// server rooted at `fixture.workspace`, return the parsed JSON body
/// carried inside `content[0].text`.
async fn run_mcp_tool_json(verb: &str, fixture: &TestFixture, params: Value) -> Value {
    // Build a server whose `ServerState.workspace` points at the fixture
    // tempdir (not cwd). Mirrors the real `grex serve --workspace ...`.
    let workspace = fixture.workspace.path().to_path_buf();
    let state = grex_mcp::ServerState::new(
        grex_core::Scheduler::new(1),
        grex_core::Registry::default(),
        workspace.join("grex.jsonl"),
        workspace,
    );
    let (server_io, client_io) = tokio::io::duplex(4096);
    let server = grex_mcp::GrexMcpServer::new(state);
    let server_task = tokio::spawn(async move {
        let _ = server.run(server_io).await;
    });
    let (read, write) = tokio::io::split(client_io);
    let mut client = Client {
        reader: BufReader::new(read),
        writer: write,
        next_id: AtomicI64::new(1),
        server_task: Some(server_task),
    };
    let _ = client.initialize().await;
    client.notify("initialized", json!({})).await;
    let resp = client.call("tools/call", json!({ "name": verb, "arguments": params })).await;
    client.shutdown().await;

    let result = resp
        .get("result")
        .unwrap_or_else(|| panic!("MCP tools/call for `{verb}` returned no `result`: {resp:?}"));
    let text = result
        .pointer("/content/0/text")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("MCP `{verb}` envelope has no content[0].text: {result:?}"));
    serde_json::from_str(text)
        .unwrap_or_else(|e| panic!("MCP `{verb}` body is not JSON: {e}; body: {text:?}"))
}

/// Seed an empty workspace → all-OK report. Both surfaces must emit the
/// same `{exit_code, worst_severity, findings}` shape.
///
/// We do NOT seed drift: introducing a .gitignore-missing warning would
/// require fragile text matching on its detail strings, and empty-
/// workspace already exercises the full rendering pipeline (findings
/// array, severity label set, exit-code roll-up).
pub async fn assert_parity_doctor_report() {
    let fixture = TestFixture::new();
    let cli_raw = run_cli_json("doctor", &[], &fixture).await;
    let mcp_raw = run_mcp_tool_json("doctor", &fixture, json!({})).await;
    let cli = normalize(cli_raw);
    let mcp = normalize(mcp_raw);

    // Canonical doctor shape pins:
    //   {exit_code: number, worst_severity: string, findings: [...]}
    // No wrapper keys. Any divergence is a schema contract break.
    assert_eq!(cli, mcp, "doctor CLI/MCP JSON bodies must be byte-equal after normalise");
    for (k, expected_type) in
        [("exit_code", "number"), ("worst_severity", "string"), ("findings", "array")]
    {
        let got = &cli[k];
        let ok = match expected_type {
            "number" => got.is_number(),
            "string" => got.is_string(),
            "array" => got.is_array(),
            _ => unreachable!(),
        };
        assert!(
            ok,
            "doctor JSON missing or wrong-typed field `{k}` (expected {expected_type}): {cli:?}"
        );
    }
}

/// Seed a `REPOS.json` with two entries — one scripted (non-empty URL),
/// one declarative (empty URL) — and assert both surfaces produce the
/// same `ImportPlan` shape under `--dry-run`.
pub async fn assert_parity_import_plan() {
    let fixture = TestFixture::new();
    let repos_path = fixture.workspace.path().join("REPOS.json");
    std::fs::write(
        &repos_path,
        r#"[
            {"url": "https://example.invalid/a.git", "path": "a"},
            {"url": "", "path": "b"}
        ]"#,
    )
    .expect("seed REPOS.json fixture");

    // CLI gets the absolute path via --from-repos-json; MCP gets the
    // workspace-relative form (the MCP handler resolves against
    // state.workspace + canonicalises).
    let cli_args = vec![
        "--from-repos-json".to_string(),
        repos_path.to_string_lossy().into_owned(),
        "--dry-run".to_string(),
    ];
    let cli_raw = run_cli_json("import", &cli_args, &fixture).await;
    let mcp_raw = run_mcp_tool_json(
        "import",
        &fixture,
        json!({ "fromReposJson": "REPOS.json", "dryRun": true }),
    )
    .await;

    let cli = normalize(cli_raw);
    let mcp = normalize(mcp_raw);

    // Canonical import shape pins:
    //   {dry_run: bool, imported: [...], skipped: [...], failed: [...]}
    // No `summary` wrapper — counts are derived from array lengths.
    assert_eq!(cli, mcp, "import CLI/MCP JSON bodies must be byte-equal after normalise");
    for k in ["dry_run", "imported", "skipped", "failed"] {
        assert!(cli.get(k).is_some(), "import JSON missing field `{k}`: {cli:?}");
    }
    assert!(
        cli.get("summary").is_none(),
        "import JSON must not carry a `summary` wrapper: {cli:?}"
    );
    assert_eq!(cli["dry_run"], json!(true));
    assert_eq!(cli["imported"].as_array().map(Vec::len), Some(2));
    assert_eq!(cli["skipped"].as_array().map(Vec::len), Some(0));
    assert_eq!(cli["failed"].as_array().map(Vec::len), Some(0));
}

#[cfg(test)]
mod parity_helper_tests {
    use super::*;

    /// `default_args_for` returns args every CLI verb's clap parser
    /// will accept. Catches a future contributor who renames
    /// `RmArgs::path` → `RmArgs::pack_path` etc. without updating the
    /// fixture map.
    #[test]
    fn default_args_cover_required_positionals() {
        let fixture = TestFixture::new();
        for verb in grex_mcp::VERBS_EXPOSED {
            // Every verb in VERBS_EXPOSED gets an entry — `_` arm is
            // empty, which is correct for verbs with no required
            // positionals. Probe for the five verbs with required
            // positionals (add, rm, run, exec, sync) to lock the contract.
            let args = default_args_for(verb, &fixture);
            if matches!(*verb, "add" | "rm" | "run" | "exec" | "sync") {
                assert!(
                    !args.is_empty(),
                    "verb `{verb}` requires a positional but default_args_for returned empty",
                );
            }
        }
    }

    /// `default_mcp_params_for` returns params every MCP `Params`
    /// struct will deserialise. `deny_unknown_fields` plus
    /// missing-required-field both yield `-32602`; this test pins the
    /// contract at compile-test time — but since we don't `use` the
    /// `Params` types here, the actual deser check happens at the
    /// per-verb `parity_*` integration tests in `tests/parity.rs`.
    #[test]
    fn default_mcp_params_cover_all_verbs() {
        let fixture = TestFixture::new();
        for verb in grex_mcp::VERBS_EXPOSED {
            let _ = default_mcp_params_for(verb, &fixture);
        }
    }
}
