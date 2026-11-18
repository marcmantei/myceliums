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
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("not JSON ({e}): {}", stdout(o)))
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
