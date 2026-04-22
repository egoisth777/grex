//! Stage 8 subprocess smoke tests for `grex serve`.
//!
//! Spawns the actual `grex` binary as a child process, pipes a hand-rolled
//! JSON-RPC handshake over stdio, and asserts:
//!
//! * 8.T1 — `tools/list` returns the 11 spec-mandated verbs and stdout
//!   carries only well-formed JSON-RPC frames.
//! * 8.T2 — closing stdin (the MCP "shutdown" handshake — no `shutdown`
//!   method exists in 2025-06-18) drives the child to exit 0 within 500 ms.
//! * 8.T3 — `tracing` lines appear on stderr and never on stdout, proving
//!   the stdout-discipline contract of the stdio transport survives a
//!   real subprocess (no surprise `print!` regressions in the binary).
//!
//! These are intentionally separate from the in-process duplex tests in
//! `crates/grex-mcp/tests/{handshake,tools_list_empty}.rs`: they exercise
//! the binary boundary, not the framework wiring.

use assert_cmd::cargo::CommandCargoExt;
use std::{
    io::{BufRead, BufReader, Read, Write},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

const SPAWN_ENV: &[(&str, &str)] = &[
    // Quiet rmcp's "Service initialized as server" INFO line so 8.T3's
    // stderr assertion only matches our own `grex` target. The default
    // (`grex=info,rmcp=warn`) is set by `grex serve` when no env var is
    // present; we override here only to make the test deterministic
    // across CI envs that may pre-set RUST_LOG.
    ("RUST_LOG", "grex=info,rmcp=warn"),
];

/// Build the JSON-RPC framing the rmcp stdio transport expects.
/// rmcp speaks line-delimited JSON (`\n` per message) over stdio for
/// MCP 2025-06-18 — no Content-Length headers, no length prefix.
fn frame(msg: &str) -> String {
    let mut s = String::with_capacity(msg.len() + 1);
    s.push_str(msg);
    s.push('\n');
    s
}

fn spawn_serve() -> std::process::Child {
    let mut cmd = Command::cargo_bin("grex").expect("grex binary builds");
    cmd.arg("serve");
    for (k, v) in SPAWN_ENV {
        cmd.env(k, v);
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn grex serve")
}

fn read_until_contains(
    rx: &mut dyn BufRead,
    needle: &str,
    deadline: Instant,
) -> Result<String, String> {
    let mut acc = String::new();
    while Instant::now() < deadline {
        let mut line = String::new();
        match rx.read_line(&mut line) {
            Ok(0) => return Err(format!("EOF before `{needle}`; got: {acc}")),
            Ok(_) => {
                acc.push_str(&line);
                if line.contains(needle) {
                    return Ok(acc);
                }
            }
            Err(e) => return Err(format!("read err {e}; partial: {acc}")),
        }
    }
    Err(format!("timeout waiting for `{needle}`; got: {acc}"))
}

fn drive_init(stdin: &mut std::process::ChildStdin, client_name: &str) {
    let init = format!(
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"{client_name}","version":"0.0.1"}}}}}}"#,
    );
    stdin.write_all(frame(&init).as_bytes()).expect("write init");
    stdin.flush().expect("flush init");
}

fn drive_initialized(stdin: &mut std::process::ChildStdin) {
    let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    stdin
        .write_all(frame(initialized).as_bytes())
        .expect("write initialized");
}

fn wait_for_exit(child: &mut std::process::Child, timeout: Duration, label: &str) {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait().expect("try_wait") {
            Some(_) => return,
            None if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            None => {
                let _ = child.kill();
                panic!("child did not exit within {timeout:?}: {label}");
            }
        }
    }
}

/// 8.T1 — drive a real `grex serve` subprocess through initialize +
/// notifications/initialized + tools/list and assert the 11-tool surface.
#[test]
fn grex_serve_subprocess_responds_to_tools_list() {
    let mut child = spawn_serve();
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);

    drive_init(&mut stdin, "serve-smoke");
    let deadline = Instant::now() + Duration::from_secs(10);
    let init_resp = read_until_contains(&mut reader, "\"id\":1", deadline)
        .expect("initialize response within 10s");
    let init_line = init_resp
        .lines()
        .find(|l| l.contains("\"id\":1"))
        .expect("init id=1 line present");
    let _: serde_json::Value =
        serde_json::from_str(init_line).expect("init response is valid JSON");

    drive_initialized(&mut stdin);
    let list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
    stdin.write_all(frame(list).as_bytes()).expect("write tools/list");
    stdin.flush().expect("flush stdin");

    let list_resp = read_until_contains(&mut reader, "\"id\":2", deadline)
        .expect("tools/list response within 10s");
    let list_line = list_resp
        .lines()
        .find(|l| l.contains("\"id\":2"))
        .expect("tools/list id=2 line present");
    let parsed: serde_json::Value =
        serde_json::from_str(list_line).expect("tools/list response is valid JSON");
    let tools = parsed
        .pointer("/result/tools")
        .and_then(|v| v.as_array())
        .expect("result.tools is an array");
    assert_eq!(
        tools.len(),
        11,
        "expected 11 tools, got {}: {:?}",
        tools.len(),
        tools.iter().map(|t| t.get("name")).collect::<Vec<_>>(),
    );

    drop(stdin);
    wait_for_exit(&mut child, Duration::from_millis(2000), "after stdin close");
}

/// 8.T2 — closing stdin must drive the child to exit cleanly within
/// 500 ms. MCP 2025-06-18 has no `shutdown` JSON-RPC method; transport
/// close IS the shutdown handshake.
#[test]
fn grex_serve_shutdown_exits_cleanly() {
    let mut child = spawn_serve();
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut reader = BufReader::new(stdout);

    drive_init(&mut stdin, "serve-smoke-shutdown");
    let deadline = Instant::now() + Duration::from_secs(10);
    let _ = read_until_contains(&mut reader, "\"id\":1", deadline)
        .expect("initialize response within 10s");

    drop(stdin);

    let exit_deadline = Instant::now() + Duration::from_millis(500);
    let status = loop {
        match child.try_wait().expect("try_wait") {
            Some(s) => break s,
            None if Instant::now() < exit_deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            None => {
                let _ = child.kill();
                panic!("grex serve did not exit within 500 ms of stdin close");
            }
        }
    };
    assert!(
        status.success(),
        "grex serve exited non-zero on clean shutdown: {status:?}",
    );
}

/// 8.T3 — `tracing` output lands on stderr; stdout carries only
/// JSON-RPC. Proves the binary boundary respects the same stdio
/// discipline the in-process `tests/stdout_discipline.rs` exercises.
#[test]
fn grex_serve_stderr_carries_tracing() {
    let mut child = spawn_serve();
    let mut stdin = child.stdin.take().expect("stdin pipe");
    let stdout = child.stdout.take().expect("stdout pipe");
    let mut stderr = child.stderr.take().expect("stderr pipe");
    let mut stdout_reader = BufReader::new(stdout);

    drive_init(&mut stdin, "serve-smoke-tracing");
    drive_initialized(&mut stdin);

    let deadline = Instant::now() + Duration::from_secs(10);
    let stdout_acc =
        read_until_contains(&mut stdout_reader, "\"id\":1", deadline).expect("init response");

    drop(stdin);
    let mut stdout_tail = String::new();
    let _ = stdout_reader.read_to_string(&mut stdout_tail);
    let full_stdout = format!("{stdout_acc}{stdout_tail}");

    let _ = child.wait().expect("child waits");
    let mut stderr_buf = String::new();
    stderr
        .read_to_string(&mut stderr_buf)
        .expect("read stderr");

    // stdout: every non-empty line must parse as JSON.
    for (i, line) in full_stdout.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        serde_json::from_str::<serde_json::Value>(trimmed).unwrap_or_else(|e| {
            panic!("stdout line {i} is not valid JSON ({e}): {trimmed}\nfull stdout:\n{full_stdout}")
        });
    }

    // stderr: must contain at least one tracing-format line.
    let has_tracing = stderr_buf.contains("INFO")
        || stderr_buf.contains("WARN")
        || stderr_buf.contains("DEBUG");
    assert!(
        has_tracing,
        "stderr lacks any tracing level marker (INFO/WARN/DEBUG); got:\n{stderr_buf}",
    );
}
