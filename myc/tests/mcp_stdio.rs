//! `myc mcp` over stdio (#98), run as the real binary: stdout carries only
//! JSON-RPC frames — progress notifications included — while the per-call
//! log lines go to stderr at the default level, and `--stats-file` is written
//! when the server stops.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/sample-ts-project")
        .canonicalize()
        .expect("fixture exists")
}

#[test]
fn stdout_carries_only_json_rpc_and_logs_go_to_stderr() {
    let data = tempfile::tempdir().unwrap();
    let stats = data.path().join("stats.json");
    let mut child = Command::new(env!("CARGO_BIN_EXE_myc"))
        .args(["mcp", "--stats-file"])
        .arg(&stats)
        .env("MYCELIUMS_DATA_DIR", data.path())
        // Never the developer's home.
        .env("HOME", data.path())
        // The default log level is under test.
        .env_remove("RUST_LOG")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run myc mcp");

    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    let reader = std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "stdio-test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "analyze",
            "arguments": {"path": fixture().to_string_lossy(), "skip_embeddings": true},
            "_meta": {"progressToken": "stdio-1"}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {
            "name": "list_repos", "arguments": {}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {
            "name": "search_code", "arguments": {"query": ""}}}),
    ];
    for request in &requests {
        writeln!(stdin, "{request}").unwrap();
    }
    stdin.flush().unwrap();

    // Read until the three tool calls are answered.
    let mut frames: Vec<Value> = Vec::new();
    let mut answered = std::collections::BTreeSet::new();
    while answered.len() < 3 {
        let line = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("the server answers within 2 minutes");
        let frame: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line:?}"));
        assert_eq!(frame["jsonrpc"], "2.0", "{line}");
        assert!(
            frame.get("id").is_some() || frame.get("method").is_some(),
            "not a JSON-RPC frame: {line}"
        );
        if let Some(id) = frame["id"].as_u64().filter(|id| *id >= 2) {
            answered.insert(id);
        }
        frames.push(frame);
    }
    drop(stdin); // EOF: the server stops.
    let output = child.wait_with_output().unwrap();
    reader.join().unwrap();
    for line in rx.try_iter() {
        let frame: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON ({e}): {line:?}"));
        assert_eq!(frame["jsonrpc"], "2.0", "{line}");
    }
    assert!(output.status.success(), "{output:?}");

    let progress: Vec<&Value> = frames
        .iter()
        .filter(|f| f["method"] == "notifications/progress")
        .collect();
    assert!(!progress.is_empty(), "{frames:#?}");
    assert!(progress
        .iter()
        .all(|p| p["params"]["progressToken"] == "stdio-1"));
    let analyze = frames.iter().find(|f| f["id"] == 2).unwrap();
    assert_eq!(analyze["result"]["isError"], false, "{analyze}");
    let search = frames.iter().find(|f| f["id"] == 4).unwrap();
    assert!(search.get("error").is_some(), "empty query: {search}");

    // One log line per call on stderr, at the default level.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let calls: Vec<&str> = stderr.lines().filter(|l| l.contains("tool_call")).collect();
    assert_eq!(calls.len(), 3, "{stderr}");
    assert!(
        calls
            .iter()
            .any(|l| l.contains("tool=analyze") && l.contains("outcome=ok")),
        "{stderr}"
    );
    assert!(
        calls
            .iter()
            .any(|l| l.contains("tool=search_code") && l.contains("outcome=protocol_error")),
        "{stderr}"
    );
    assert!(!stderr.contains('\u{1b}'), "no colour codes off a terminal");

    let stats: Value = serde_json::from_slice(&std::fs::read(&stats).unwrap()).unwrap();
    assert_eq!(stats["total_calls"], 3, "{stats}");
}
