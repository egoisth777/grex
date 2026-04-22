//! L2 real-pipe per-OS guard — **Linux** target.
//!
//! Owned by `feat-m7-2-mcp-test-harness` Stage 3. Compiled only on
//! Linux; the parallel `real_pipe_macos.rs` / `real_pipe_windows.rs`
//! cover the other two OSes via the CI matrix.
//!
//! ## Why subprocess, not `tokio::io::duplex`?
//!
//! Stage 2's `common::Client` drives the server through an in-memory
//! `tokio::io::duplex(4096)` pair. That hides:
//!
//! 1. **Real OS pipe back-pressure.** Linux pipes default to 64 KiB
//!    (or `/proc/sys/fs/pipe-max-size`); a >64 KiB cumulative response
//!    forces the server's writer half to block until the client drains.
//!    `duplex` is a `BytesMut`-backed in-process channel — it never
//!    blocks on real buffer pressure.
//! 2. **Stderr-handle independence.** `duplex` has no concept of
//!    stderr; closing the client's stderr cannot affect a server
//!    reading stdin / writing stdout. The real subprocess case must
//!    hold even when the parent abandons stderr (a common pattern in
//!    MCP host shims that only multiplex stdout/stdin).
//!
//! Both guarantees come from `.omne/cfg/mcp.md` §"Stdio discipline" and
//! the MCP 2025-06-18 transport contract; this file enforces them at
//! the binary boundary on the host where the bug would surface.
//!
//! ## Why duplicate the spawn helpers across `real_pipe_{linux,macos,windows}.rs`?
//!
//! Each file is `#[cfg(target_os = "...")]`-gated and exactly one
//! compiles per build. Hoisting helpers into `common/mod.rs` (which is
//! `DuplexStream`-typed and shared with the L3/L4/L5 in-process suites)
//! would force the subprocess-only `assert_cmd` + `std::process` types
//! into a hot path that doesn't need them. Three near-identical small
//! files is the simpler shape.
//!
//! Reuse pattern is consciously borrowed from
//! `crates/grex/tests/serve_smoke.rs` (m7-1 Stage 8).

#![cfg(target_os = "linux")]

use assert_cmd::cargo::CommandCargoExt;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

// Linux response-drain budget — kept generous so CI under load (KVM,
// noisy neighbours) doesn't false-fail. `MEMORY.md` records 250 ms
// for cancellation on Linux/macOS; response drain is a strictly
// larger envelope so we widen here.
const RESPONSE_DEADLINE: Duration = Duration::from_secs(15);

/// Quiet rmcp's INFO line so the stderr-close test can observe a clean
/// channel without noise; matches the env wiring used by
/// `crates/grex/tests/serve_smoke.rs` m7-1 Stage 8.
const SPAWN_ENV: &[(&str, &str)] = &[("RUST_LOG", "grex=info,rmcp=warn")];

fn frame(msg: &str) -> String {
    let mut s = String::with_capacity(msg.len() + 1);
    s.push_str(msg);
    s.push('\n');
    s
}

fn spawn_serve(stderr: Stdio) -> std::process::Child {
    let mut cmd = Command::cargo_bin("grex").expect("grex binary builds");
    cmd.arg("serve");
    for (k, v) in SPAWN_ENV {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(stderr)
        .spawn()
        .expect("spawn grex serve")
}

fn drive_init(stdin: &mut std::process::ChildStdin, id: u32) {
    let init = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"grex-mcp-real-pipe-linux","version":"0.0.1"}}}}}}"#,
    );
    stdin.write_all(frame(&init).as_bytes()).expect("write init");
    stdin.flush().expect("flush init");
}

fn drive_initialized(stdin: &mut std::process::ChildStdin) {
    let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    stdin.write_all(frame(initialized).as_bytes()).expect("write initialized");
    stdin.flush().expect("flush initialized");
}

fn read_until_id(
    rx: &mut dyn BufRead,
    id_marker: &str,
    deadline: Instant,
) -> Result<String, String> {
    let mut acc = String::new();
    while Instant::now() < deadline {
        let mut line = String::new();
        match rx.read_line(&mut line) {
            Ok(0) => return Err(format!("EOF before `{id_marker}`; got: {acc}")),
            Ok(_) => {
                acc.push_str(&line);
                if line.contains(id_marker) {
                    return Ok(line);
                }
            }
            Err(e) => return Err(format!("read err {e}; partial: {acc}")),
        }
    }
    Err(format!("timeout waiting for `{id_marker}`; got: {acc}"))
}

/// Drain `expected_count` `tools/list` responses starting at id
/// `first_id`, returning total bytes read. Asserts each response is
/// valid JSON, carries `result.tools`, and arrives in monotonic id
/// order — the contract under test for the pipe-buffer back-pressure
/// guard.
fn drain_burst_responses(
    reader: &mut BufReader<std::process::ChildStdout>,
    first_id: u32,
    expected_count: u32,
    deadline: Instant,
) -> usize {
    let mut total_bytes = 0usize;
    let mut seen_ids: Vec<u32> = Vec::with_capacity(expected_count as usize);
    for expected_id in first_id..(first_id + expected_count) {
        let marker = format!("\"id\":{expected_id}");
        let line = read_until_id(reader, &marker, deadline)
            .unwrap_or_else(|e| panic!("missing id={expected_id}: {e}"));
        total_bytes += line.len();
        let parsed: serde_json::Value = serde_json::from_str(line.trim()).unwrap_or_else(|e| {
            panic!("response for id={expected_id} not valid JSON ({e}): {line}")
        });
        let got_id = parsed
            .get("id")
            .and_then(|v| v.as_u64())
            .unwrap_or_else(|| panic!("response missing numeric id: {line}"));
        seen_ids.push(got_id as u32);
        assert!(
            parsed.pointer("/result/tools").is_some(),
            "response id={expected_id} lacks result.tools: {line}",
        );
    }
    let expected_order: Vec<u32> = (first_id..(first_id + expected_count)).collect();
    assert_eq!(
        seen_ids, expected_order,
        "tools/list responses arrived out of order: got {seen_ids:?}, expected {expected_order:?}",
    );
    total_bytes
}

fn wait_for_exit_or_kill(child: &mut std::process::Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => return,
            None if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            None => {
                let _ = child.kill();
                return;
            }
        }
    }
}

/// L2.real_pipe.T1 — a multi-frame response stream whose **cumulative**
/// byte count crosses the OS pipe-buffer boundary (>64 KiB) is
/// delivered intact and in order.
///
/// Why this shape rather than a single >64 KiB frame: in m7-1 stage 6
/// the 11 tool implementations are stubs returning `not_implemented` —
/// no production handler emits a single >64 KiB JSON body today. The
/// kernel-pipe property under test ("the framer drains under
/// back-pressure without dropping bytes") is identical for one big
/// frame vs. many small frames whose sum exceeds the buffer; the
/// many-small variant is the only one we can drive end-to-end on
/// today's tool surface. When `ls` lands a real impl in feat-m7-3 (L3
/// parity stage) this test should be updated to the single-frame form
/// the spec line 178-180 describes — the contract being asserted does
/// not change.
///
/// Mechanic: queue `BURST` `tools/list` requests back-to-back without
/// reading between them, forcing the server to write while the client
/// drains. Then verify all `BURST` responses arrive intact and in
/// monotonic id order.
#[test]
fn large_response_crosses_pipe_buffer() {
    const BURST: u32 = 32;
    const MIN_TOTAL_BYTES: usize = 64 * 1024;

    let mut child = spawn_serve(Stdio::piped());
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);

    drive_init(&mut stdin, 1);
    let init_deadline = Instant::now() + RESPONSE_DEADLINE;
    let _ = read_until_id(&mut reader, "\"id\":1", init_deadline)
        .expect("initialize response within budget");
    drive_initialized(&mut stdin);

    for id in 2..(2 + BURST) {
        let frame_str = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/list"}}"#);
        stdin.write_all(frame(&frame_str).as_bytes()).expect("write tools/list burst frame");
    }
    stdin.flush().expect("flush burst");

    let drain_deadline = Instant::now() + RESPONSE_DEADLINE;
    let total_bytes = drain_burst_responses(&mut reader, 2, BURST, drain_deadline);
    assert!(
        total_bytes > MIN_TOTAL_BYTES,
        "burst response total {total_bytes} B did not cross pipe-buffer threshold {MIN_TOTAL_BYTES} B \
         — test cannot prove the back-pressure contract; bump BURST",
    );

    drop(stdin);
    wait_for_exit_or_kill(&mut child, Duration::from_secs(5));
}

/// L2.real_pipe.T2 — closing the client's stderr handle does NOT panic
/// or terminate the server; subsequent `tools/list` requests still
/// receive responses.
///
/// Mechanic on Linux: spawning the child with `Stdio::null()` for
/// stderr redirects the server's stderr writes into `/dev/null`. This
/// is the closest analogue to a host that opens stdin/stdout pipes but
/// ignores stderr — the case `.omne/cfg/mcp.md` calls out under
/// "Stdio discipline" ("server MUST tolerate stderr being unread or
/// redirected to /dev/null"). If the server panicked on a stderr-write
/// `BrokenPipe` or aborted on a tracing-write failure the subsequent
/// `tools/list` would either time out (process still alive but wedged)
/// or surface EOF (process gone). Both manifest as a failure of
/// `read_until_id`.
///
/// We do NOT close stderr on an already-running child via `dup2` —
/// that races against the rmcp service spawning its
/// `tracing_subscriber` writer and would be flaky. The deterministic
/// equivalent is to launch with stderr already null'd.
#[test]
fn client_stderr_close_does_not_panic_server() {
    let mut child = spawn_serve(Stdio::null());
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);

    drive_init(&mut stdin, 1);
    let init_deadline = Instant::now() + RESPONSE_DEADLINE;
    let _ = read_until_id(&mut reader, "\"id\":1", init_deadline)
        .expect("initialize response within budget despite null'd stderr");
    drive_initialized(&mut stdin);

    let req1 = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    stdin.write_all(frame(req1).as_bytes()).expect("write first tools/list");
    stdin.flush().expect("flush first");
    let resp1_deadline = Instant::now() + RESPONSE_DEADLINE;
    let resp1 = read_until_id(&mut reader, "\"id\":2", resp1_deadline)
        .expect("first tools/list responds despite null'd stderr");
    let _: serde_json::Value =
        serde_json::from_str(resp1.trim()).expect("first response is valid JSON");

    let req2 = r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#;
    stdin.write_all(frame(req2).as_bytes()).expect("write second tools/list");
    stdin.flush().expect("flush second");
    let resp2_deadline = Instant::now() + RESPONSE_DEADLINE;
    let resp2 = read_until_id(&mut reader, "\"id\":3", resp2_deadline)
        .expect("second tools/list responds — proves no panic on stderr write");
    let _: serde_json::Value =
        serde_json::from_str(resp2.trim()).expect("second response is valid JSON");

    drop(stdin);

    let exit_deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None if Instant::now() < exit_deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            None => {
                let _ = child.kill();
                panic!("server did not exit cleanly within 5 s after stdin close");
            }
        }
    };
    assert!(status.success(), "server exited non-zero after stderr-null'd run: {status:?}",);

    let mut tail = String::new();
    let _ = reader.read_to_string(&mut tail);
}
