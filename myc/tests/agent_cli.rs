//! End-to-end checks of the agent-facing commands (#126) against a real index.
//!
//! The `sample-ts-project` fixture is indexed once (embeddings off, offline)
//! into a throwaway data directory; every test then runs the built `myc`
//! binary with `MYCELIUMS_DATA_DIR` pointing there and the fixture as working
//! directory, the way an agent or a Claude Code hook runs it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use myceliums_core::Analyzer;
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use tempfile::TempDir;
use tokio::sync::OnceCell;

const REPO_ID: &str = "sample-ts-project";

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../tests/fixtures/sample-ts-project")
        .canonicalize()
        .expect("fixture exists")
}

static DATA: OnceCell<TempDir> = OnceCell::const_new();

async fn data_dir() -> &'static Path {
    DATA.get_or_init(|| async {
        let tmp = tempfile::tempdir().expect("tempdir");
        let store = Store::open(&RepoRegistry::repo_db_path(tmp.path(), REPO_ID), REPO_ID)
            .await
            .expect("open store");
        let result = Analyzer::new(store, fixture())
            .set_skip_embeddings(true)
            .analyze()
            .await
            .expect("analyze fixture");
        let mut registry = RepoRegistry::load(&tmp.path().join("repos.json")).unwrap();
        registry.register(RepoInfo {
            id: REPO_ID.into(),
            name: REPO_ID.into(),
            path: fixture().to_string_lossy().into_owned(),
            analyzed_at: "2026-01-01T00:00:00Z".into(),
            symbol_count: result.symbol_count as u32,
            file_count: result.file_count as u32,
            analyzed_commit: None,
            vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
            classification_version: myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
        });
        registry.save().unwrap();
        tmp
    })
    .await
    .path()
}

fn myc(data: &Path, cwd: &Path, args: &[&str], stdin: Option<&str>) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_myc"))
        .args(args)
        .current_dir(cwd)
        .env("MYCELIUMS_DATA_DIR", data)
        // Never the developer's home: some code paths still derive paths
        // from HOME.
        .env("HOME", data)
        // Debug builds on shared CI runners can be slower than the hook's
        // 400 ms production deadline on a cold index; the deadline itself is
        // unit-tested. MYC_HOOK_DEBUG puts the reason for silence on stderr.
        .env("MYC_HOOK_DEADLINE_MS", "30000")
        .env("MYC_HOOK_DEBUG", "1")
        .env_remove("CLAUDE_PROJECT_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run myc");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.unwrap_or("").as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "not JSON ({e}): {}\nstderr: {}",
            stdout(o),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

#[tokio::test]
async fn callers_and_callees_answer_with_locations_and_freshness() {
    let data = data_dir().await;
    let out = myc(data, &fixture(), &["callees", "processRequest"], None);
    assert!(out.status.success(), "{out:?}");
    let text = stdout(&out);
    assert!(
        text.starts_with("callees of processRequest (Function) src/index.ts:"),
        "{text}"
    );
    assert!(text.contains("parseBody  src/index.ts:"), "{text}");
    assert!(text.contains("validateInput  src/index.ts:"), "{text}");
    assert!(
        text.trim_end()
            .lines()
            .last()
            .unwrap()
            .starts_with("index:"),
        "{text}"
    );

    let out = myc(data, &fixture(), &["callers", "parseBody", "--json"], None);
    assert!(out.status.success(), "{out:?}");
    let v = json(&out);
    assert_eq!(v["direction"], "callers");
    assert_eq!(v["symbol"]["name"], "parseBody");
    assert_eq!(v["symbol"]["file"], "src/index.ts");
    assert!(v["freshness"]["state"].is_string(), "{v}");
    let callers: Vec<&str> = v["results"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert!(callers.contains(&"processRequest"), "{v}");
    assert_eq!(v["total"].as_u64().unwrap() as usize, callers.len());
}

#[tokio::test]
async fn unknown_symbols_exit_1_and_unindexed_directories_are_refused() {
    let data = data_dir().await;
    let out = myc(data, &fixture(), &["callers", "noSuchSymbolAnywhere"], None);
    assert_eq!(out.status.code(), Some(1), "{out:?}");
    assert!(stdout(&out).contains("No definition named 'noSuchSymbolAnywhere'"));

    // Outside every indexed project there is no fallback to another repo.
    let elsewhere = tempfile::tempdir().unwrap();
    let out = myc(data, elsewhere.path(), &["callers", "parseBody"], None);
    assert!(!out.status.success());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("No myceliums index covers"),
        "{out:?}"
    );
}

/// The read commands take their tables from a `GraphService` snapshot (#101):
/// they answer from a real index, and a registered repository whose index is
/// missing is an error, never a freshly created empty database.
#[tokio::test]
async fn read_commands_use_the_index_and_never_create_one() {
    let data = data_dir().await;
    for args in [
        &["stats", REPO_ID][..],
        &["communities", REPO_ID],
        &[
            "query",
            "MATCH (n:CodeSymbol) RETURN n.name LIMIT 1",
            "--repo",
            REPO_ID,
        ],
    ] {
        let out = myc(data, &fixture(), args, None);
        assert!(out.status.success(), "{args:?}: {out:?}");
    }
    assert!(stdout(&myc(data, &fixture(), &["stats", REPO_ID], None)).contains("Symbols:"));

    let empty = tempfile::tempdir().unwrap();
    let mut registry = RepoRegistry::load(&empty.path().join("repos.json")).unwrap();
    registry.register(RepoInfo {
        id: "ghost".into(),
        name: "ghost".into(),
        path: empty.path().to_string_lossy().into_owned(),
        analyzed_at: String::new(),
        symbol_count: 0,
        file_count: 0,
        analyzed_commit: None,
        vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
        classification_version: myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
    });
    registry.save().unwrap();
    for args in [
        &["stats", "ghost"][..],
        &["export", "ghost", "--format", "json"],
    ] {
        let out = myc(empty.path(), empty.path(), args, None);
        assert!(!out.status.success(), "{args:?}: {out:?}");
    }
    assert!(
        !RepoRegistry::repo_db_path(empty.path(), "ghost").exists(),
        "a read must never create the index directory"
    );
}

#[tokio::test]
async fn impact_of_a_file_lists_dependents_and_files() {
    let data = data_dir().await;
    let out = myc(
        data,
        &fixture(),
        &["impact", "src/services/user.ts", "--json"],
        None,
    );
    assert!(out.status.success(), "{out:?}");
    let v = json(&out);
    let files: Vec<&str> = v["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect();
    assert!(files.contains(&"src/services/user.ts"), "{v}");
    assert!(!v["roots"].as_array().unwrap().is_empty(), "{v}");
    assert!(
        v["dependents"].is_array() && v["omitted"].is_number(),
        "{v}"
    );
}

#[tokio::test]
async fn context_pack_json_names_the_mentioned_symbol_first() {
    let data = data_dir().await;
    let out = myc(
        data,
        &fixture(),
        &[
            "context",
            "--json",
            "--budget",
            "800",
            "how",
            "does",
            "processRequest",
            "validate",
            "input",
        ],
        None,
    );
    assert!(out.status.success(), "{out:?}");
    let v = json(&out);
    assert_eq!(v["symbols"][0]["name"], "processRequest");
    assert_eq!(v["symbols"][0]["reason"], "named");
    assert_eq!(v["repo"], REPO_ID);
    assert!(v["estimated_tokens"].as_u64().unwrap() <= 800, "{v}");
}

fn hook_input(tool: &str, tool_input: serde_json::Value) -> String {
    serde_json::json!({
        "session_id": "s",
        "transcript_path": "/tmp/t.jsonl",
        "cwd": fixture(),
        "permission_mode": "default",
        "hook_event_name": "PreToolUse",
        "tool_name": tool,
        "tool_input": tool_input,
        "tool_use_id": "toolu_1",
    })
    .to_string()
}

#[tokio::test]
async fn grep_hook_suggests_myc_for_known_symbols_and_is_silent_otherwise() {
    let data = data_dir().await;
    let tmp = tempfile::tempdir().unwrap();
    // The hook takes the project from the input's cwd, not its own.
    let run = |stdin: &str| myc(data, tmp.path(), &["hook-grep"], Some(stdin));

    let out = run(&hook_input(
        "Grep",
        serde_json::json!({"pattern": "processRequest\\("}),
    ));
    assert!(out.status.success());
    let v = json(&out);
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    let hint = v["hookSpecificOutput"]["additionalContext"]
        .as_str()
        .unwrap();
    assert!(hint.contains("`myc callers processRequest`"), "{hint}");
    assert!(hint.contains("src/index.ts:"), "{hint}");
    assert!(v["hookSpecificOutput"].get("permissionDecision").is_none());

    let bash = hook_input(
        "Bash",
        serde_json::json!({"command": "grep -rn 'function parseBody' src | head -5"}),
    );
    assert!(stdout(&run(&bash)).contains("myc callers parseBody"));

    for silent in [
        hook_input("Grep", serde_json::json!({"pattern": "Invalid input"})),
        hook_input(
            "Grep",
            serde_json::json!({"pattern": "noSuchSymbolAnywhere"}),
        ),
        hook_input(
            "Grep",
            serde_json::json!({"pattern": "processRequest", "glob": "*.json"}),
        ),
        hook_input("Bash", serde_json::json!({"command": "ls -la"})),
        "not json at all".to_string(),
        String::new(),
    ] {
        let out = run(&silent);
        assert!(out.status.success(), "{silent}: {out:?}");
        assert!(out.stdout.is_empty(), "{silent}: {}", stdout(&out));
    }
}

/// One `myc mcp` stdio session in `cwd`: `initialize`, then each
/// `tools/call`; returns each call's `result`.
fn mcp_calls(
    data: &Path,
    cwd: &Path,
    calls: &[(&str, serde_json::Value)],
) -> Vec<serde_json::Value> {
    use std::io::{BufRead as _, BufReader};
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
    let results = {
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
        "clientInfo": {"name": "agent_cli", "version": "1"}}}),
        );
        send(serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        calls
            .iter()
            .enumerate()
            .map(|(i, (tool, args))| {
                send(
                    serde_json::json!({"jsonrpc": "2.0", "id": i + 1, "method": "tools/call",
                "params": {"name": tool, "arguments": args}}),
                )
            })
            .collect()
    };
    // Closing stdin ends the session.
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    results
}

/// `(qualified_name, file, line, depth, via)` of each listed symbol.
fn hits(
    list: &serde_json::Value,
    file: &str,
    line: &str,
) -> Vec<(String, String, u64, u64, String)> {
    list.as_array()
        .unwrap_or_else(|| panic!("a list: {list}"))
        .iter()
        .map(|h| {
            (
                h["qualified_name"].as_str().unwrap().to_string(),
                h[file].as_str().unwrap().to_string(),
                h[line].as_u64().unwrap(),
                h["depth"].as_u64().unwrap(),
                h["via"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

#[tokio::test]
async fn mcp_tools_answer_like_the_cli() {
    let data = data_dir().await;
    let cases = [
        ("callers", "find_callers", "validateInput", 3),
        ("callers", "find_callers", "findById", 2),
        ("callees", "find_callees", "handler", 3),
        ("callees", "find_callees", "main", 1),
    ];
    // No repo_id: the server resolves the repository from its working
    // directory, as the CLI does.
    let calls: Vec<(&str, serde_json::Value)> = cases
        .iter()
        .map(|(_, tool, symbol, depth)| {
            (
                *tool,
                serde_json::json!({"symbol_name": symbol, "depth": depth, "limit": 200}),
            )
        })
        .collect();
    let results = mcp_calls(data, &fixture(), &calls);
    for ((command, tool, symbol, depth), result) in cases.iter().zip(&results) {
        assert_ne!(result["isError"], true, "{tool} {symbol}: {result}");
        let mcp = &result["structuredContent"];
        let key = if *command == "callers" {
            "callers"
        } else {
            "callees"
        };
        let depth_arg = depth.to_string();
        let out = myc(
            data,
            &fixture(),
            &[
                command, symbol, "--depth", &depth_arg, "--json", "--budget", "100000",
            ],
            None,
        );
        assert!(out.status.success(), "{out:?}");
        let cli = json(&out);
        let cli_hits = hits(&cli["results"], "file", "line");
        assert!(!cli_hits.is_empty(), "{command} {symbol}: {cli}");
        assert_eq!(
            hits(&mcp[key], "file_path", "start_line"),
            cli_hits,
            "{tool} {symbol} --depth {depth}"
        );
        assert_eq!(mcp["total_count"], cli["total"], "{tool} {symbol}");
    }

    // Outside every indexed directory there is no answer about another repo.
    let elsewhere = tempfile::tempdir().unwrap();
    let results = mcp_calls(
        data,
        elsewhere.path(),
        &[(
            "find_callers",
            serde_json::json!({"symbol_name": "findById"}),
        )],
    );
    assert_eq!(results[0]["isError"], true, "{}", results[0]);
    let text = results[0]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("No index covers"), "{text}");
    assert!(text.contains("Next: run analyze with path="), "{text}");
}
