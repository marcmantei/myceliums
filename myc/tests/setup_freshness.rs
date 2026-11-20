//! The documented order — `myc analyze`, then `myc setup-claude --scope
//! project` — leaves an index the MCP server calls up to date (#151).
//!
//! `setup-claude` writes `.mcp.json`, `.claude/` and the routing block in
//! `CLAUDE.md` after the index was built. None of it changes what the
//! analysis indexes, so the `Index:` line must not call the index stale and
//! the `analyze` tool must answer from the cache instead of re-indexing
//! (which once ran into the client's 300 s timeout). An edit to CLAUDE.md
//! outside the block is still a change.

use std::io::{BufRead as _, BufReader, Write as _};
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn myc(data: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_myc"))
        .args(args)
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        // Never the developer's home: setup-claude and some code paths
        // derive paths from HOME.
        .env("HOME", data)
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::null())
        .output()
        .expect("run myc")
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "commit.gpgsign=false",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git runs");
    assert!(status.success(), "git {args:?}");
}

/// One `myc mcp` stdio session in `cwd`; returns each `tools/call` result.
fn mcp_calls(
    data: &Path,
    cwd: &Path,
    calls: &[(&str, serde_json::Value)],
) -> Vec<serde_json::Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_myc"))
        .arg("mcp")
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        .env("HOME", data)
        .env("RUST_LOG", "error")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("run myc mcp");
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut send = |msg: serde_json::Value| {
        writeln!(stdin, "{msg}").unwrap();
        stdin.flush().unwrap();
        let Some(id) = msg.get("id").cloned() else {
            return serde_json::Value::Null;
        };
        loop {
            let line = lines.next().expect("myc mcp answers").unwrap();
            let response: serde_json::Value = serde_json::from_str(&line).unwrap();
            if response["id"] == id {
                return response["result"].clone();
            }
        }
    };
    send(
        serde_json::json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {
        "protocolVersion": "2025-06-18", "capabilities": {},
        "clientInfo": {"name": "setup_freshness", "version": "1"}}}),
    );
    send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    let results = calls
        .iter()
        .enumerate()
        .map(|(i, (tool, args))| {
            send(
                serde_json::json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                "params": {"name": tool, "arguments": args}}),
            )
        })
        .collect();
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    results
}

/// The `Index:` line of a graph answer, and the `analyze` answer, from one
/// MCP session.
fn index_line_and_analyze(data: &Path, project: &Path) -> (String, String) {
    let results = mcp_calls(
        data,
        project,
        &[
            ("find_callers", serde_json::json!({"symbol_name": "helper"})),
            (
                "analyze",
                serde_json::json!({"path": project, "skip_embeddings": true}),
            ),
        ],
    );
    assert_ne!(results[0]["isError"], true, "{}", results[0]);
    let line = results[0]["structuredContent"]["index_status"]
        .as_str()
        .unwrap_or_else(|| panic!("an index line: {}", results[0]))
        .to_string();
    assert_ne!(results[1]["isError"], true, "{}", results[1]);
    let analyze = results[1]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_string();
    (line, analyze)
}

#[test]
fn setup_claude_after_analyze_leaves_the_index_up_to_date() {
    let data = tempfile::tempdir().unwrap();
    let projects = tempfile::tempdir().unwrap();
    let project = projects.path().join("setup-project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(
        project.join("src/app.ts"),
        "export function helper(): number {\n    return 1;\n}\n\
         export function start(): number {\n    return helper();\n}\n",
    )
    .unwrap();
    std::fs::write(project.join("CLAUDE.md"), "# Project\n\nTeam notes.\n").unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "init"]);
    let project = project.canonicalize().unwrap();

    let out = myc(
        data.path(),
        &project,
        &["analyze", ".", "--skip-embeddings"],
    );
    assert!(out.status.success(), "{out:?}");
    let out = myc(
        data.path(),
        &project,
        &["setup-claude", "--scope", "project"],
    );
    assert!(out.status.success(), "{out:?}");
    for written in [
        ".mcp.json",
        ".claude/settings.json",
        ".claude/agents/code-explorer.md",
    ] {
        assert!(project.join(written).is_file(), "{written}");
    }
    let memory = std::fs::read_to_string(project.join("CLAUDE.md")).unwrap();
    assert!(memory.contains("<!-- BEGIN myceliums -->"), "{memory}");

    let (line, analyze) = index_line_and_analyze(data.path(), &project);
    assert!(line.ends_with("= HEAD, up to date."), "{line}");
    assert!(analyze.contains("(cached)"), "{analyze}");

    // CLAUDE.md is indexed: an edit outside the block is a change, and
    // analyze re-indexes.
    std::fs::write(
        project.join("CLAUDE.md"),
        format!("{memory}\nMore notes.\n"),
    )
    .unwrap();
    let (line, analyze) = index_line_and_analyze(data.path(), &project);
    assert!(line.contains("1 file changed since indexing"), "{line}");
    assert!(analyze.contains("(fresh)"), "{analyze}");
}
