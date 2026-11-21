//! `myc mcp` exits promptly when its client goes away while a background
//! embedding runs (#157): the job is asked to stop and ends within a model
//! call, and the process never waits for it longer than its shutdown grace —
//! instead of until the embedding (or a model download) finished.
//!
//! The model is a fake OpenAI-compatible endpoint on localhost that takes two
//! seconds per request, so embedding the project would take about a minute.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

const DIM: usize = 8;

/// The slow fake model: its base URL, and how many requests reached it.
fn slow_model() -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let base_url = format!("http://{}/v1", listener.local_addr().unwrap());
    let requests = Arc::new(AtomicUsize::new(0));
    let count = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let count = count.clone();
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut length = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    if line == "\r\n" {
                        break;
                    }
                    if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                        length = v.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; length];
                if reader.read_exact(&mut body).is_err() {
                    return;
                }
                count.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_secs(2));
                let request: Value = serde_json::from_slice(&body).unwrap_or(json!({}));
                let inputs = request["input"].as_array().cloned().unwrap_or_default();
                let data: Vec<Value> = (0..inputs.len())
                    .map(|i| json!({ "index": i, "embedding": vec![1.0f32 + i as f32; DIM] }))
                    .collect();
                let body = serde_json::to_vec(&json!({ "data": data })).unwrap();
                let mut out = stream;
                let _ = write!(
                    out,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = out.write_all(&body);
            });
        }
    });
    (base_url, requests)
}

#[test]
fn the_server_exits_promptly_while_an_embedding_runs() {
    let data = tempfile::tempdir().unwrap();
    let (base_url, requests) = slow_model();
    let project = data.path().join("exit-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    let functions: String = (0..240)
        .map(|i| format!("export function f{i}(): number {{\n    return {i};\n}}\n"))
        .collect();
    std::fs::write(project.join("src/a.ts"), functions).unwrap();
    std::fs::write(
        project.join(".myceliums.toml"),
        format!(
            "[embedding]\nprovider = \"openai-compatible\"\nmodel = \"slow\"\n\
             base_url = \"{base_url}\"\ndim = {DIM}\n\n[analysis]\nembedding_batch_size = 8\n"
        ),
    )
    .unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_myc"))
        .arg("mcp")
        .env("MYCELIUMS_DATA_DIR", data.path())
        .env("HOME", data.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run myc mcp");
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    for request in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "exit-test", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {
            "name": "analyze", "arguments": {"path": project.to_string_lossy()}}}),
    ] {
        writeln!(stdin, "{request}").unwrap();
    }
    stdin.flush().unwrap();

    // analyze answers after the graph; the embedding runs on.
    let analyze = loop {
        let line = rx
            .recv_timeout(Duration::from_secs(120))
            .expect("analyze answers");
        let frame: Value = serde_json::from_str(&line).unwrap();
        if frame["id"] == 2 {
            break frame;
        }
    };
    assert_eq!(analyze["result"]["isError"], false, "{analyze}");
    assert!(
        analyze.to_string().contains("in the background"),
        "{analyze}"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while requests.load(Ordering::SeqCst) == 0 {
        assert!(Instant::now() < deadline, "the embedding never started");
        std::thread::sleep(Duration::from_millis(50));
    }

    // The client goes away.
    let gone = Instant::now();
    drop(stdin);
    let deadline = gone + Duration::from_secs(30);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("myc mcp still runs 30 s after its client left");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let took = gone.elapsed();
    assert!(status.success(), "{status:?}");
    assert!(
        took < Duration::from_secs(12),
        "exit took {took:?}, the embedding would have taken a minute"
    );
    assert!(
        requests.load(Ordering::SeqCst) < 10,
        "the embedding stopped instead of running on"
    );
}
