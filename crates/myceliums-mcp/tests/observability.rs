//! Observability of the MCP server (#98), end to end over JSON-RPC:
//!
//! * every tool call runs in one `tool_call` span whose fields name the
//!   tool, toolset, repository, duration, result size and outcome;
//! * `server_stats` (toolset `admin`) aggregates the calls;
//! * `analyze` sends `notifications/progress` when — and only when — the
//!   request carries a `progressToken`, all before its response.
//!
//! This binary owns its data directory (it analyzes), and
//! `MYCELIUMS_DATA_DIR` is process-wide, so the tests share one directory
//! and take turns ([`serial`]).

use myceliums_mcp::{AnalyzeParams, MyceliumsMcp, Toolsets};
use rmcp::model::{CallToolRequestParams, CallToolResult};
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::field::{Field, Visit};
use tracing::span::{Attributes, Id, Record};
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::Layer;

type Client = RunningService<RoleClient, ()>;

// ── Shared data directory and one analyzed project ──────────────────

struct Env {
    _data: tempfile::TempDir,
    _projects: tempfile::TempDir,
    project: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The data directory (set once) and a copy of the TypeScript fixture.
fn env() -> &'static Env {
    static ENV: OnceLock<Env> = OnceLock::new();
    ENV.get_or_init(|| {
        let data = tempfile::tempdir().unwrap();
        std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
        let projects = tempfile::tempdir().unwrap();
        let project = projects.path().join("observed-project");
        let fixture =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-ts-project");
        copy_dir(&fixture, &project);
        let project = project.canonicalize().unwrap();
        Env {
            _data: data,
            _projects: projects,
            project,
        }
    })
}

/// One test at a time: they share the registry file.
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
}

fn analyze_params(project: &Path) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        force: Some(true),
        skip_embeddings: Some(true),
        ..Default::default()
    }
}

/// Analyzes the project (directly, not over the wire) and returns its id.
async fn analyzed_repo() -> String {
    let project = &env().project;
    MyceliumsMcp::new()
        .analyze_for_test(analyze_params(project))
        .await
        .expect("analyze");
    myceliums_core::analyzer::repo_id_from_path(project)
}

async fn connect(toolsets: Toolsets) -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_with_toolsets_for_test(&toolsets)
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &Client, tool: &str, args: Value) -> Result<CallToolResult, String> {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
        .map_err(|e| e.to_string())
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|c| c.as_text())
        .map(|t| t.text.as_str())
        .collect()
}

// ── A subscriber that keeps the fields of closed `tool_call` spans ──

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<BTreeMap<String, String>>>>);

struct Fields(BTreeMap<String, String>);

struct Visitor<'a>(&'a mut BTreeMap<String, String>);

impl Visit for Visitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().to_string(), format!("{value:?}"));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_string(), value.to_string());
    }
}

impl<S: tracing::Subscriber + for<'a> LookupSpan<'a>> Layer<S> for Captured {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        if attrs.metadata().name() != "tool_call" {
            return;
        }
        let mut fields = BTreeMap::new();
        attrs.record(&mut Visitor(&mut fields));
        fields.insert("target".into(), attrs.metadata().target().into());
        fields.insert("level".into(), attrs.metadata().level().to_string());
        ctx.span(id)
            .unwrap()
            .extensions_mut()
            .insert(Fields(fields));
    }

    fn on_record(&self, id: &Id, values: &Record<'_>, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id) {
            if let Some(fields) = span.extensions_mut().get_mut::<Fields>() {
                values.record(&mut Visitor(&mut fields.0));
            }
        }
    }

    fn on_close(&self, id: Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(&id) {
            if let Some(fields) = span.extensions().get::<Fields>() {
                self.0.lock().unwrap().push(fields.0.clone());
            }
        }
    }
}

impl Captured {
    /// The closed spans, once there are at least `n` (spans close just
    /// after the response is sent).
    async fn wait_for(&self, n: usize) -> Vec<BTreeMap<String, String>> {
        for _ in 0..200 {
            let spans = self.0.lock().unwrap().clone();
            if spans.len() >= n {
                return spans;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        panic!(
            "expected {n} tool_call spans, got {:?}",
            self.0.lock().unwrap()
        );
    }
}

// ── Tests ────────────────────────────────────────────────────────────

/// One span per call, its fields filled in at the dispatch layer, for every
/// outcome. (The subscriber is thread-local; the current-thread test
/// runtime runs the server task on this thread.)
#[tokio::test]
async fn every_tool_call_runs_in_one_span_with_its_fields() {
    let _turn = serial().await;
    let repo_id = analyzed_repo().await;

    let captured = Captured::default();
    let subscriber = tracing_subscriber::registry().with(captured.clone());
    let _default = tracing::subscriber::set_default(subscriber);

    let client = connect(Toolsets::all()).await;
    let ok = call(
        &client,
        "find_callers",
        json!({"repo_id": repo_id, "symbol_name": "capitalize"}),
    )
    .await
    .unwrap();
    assert_ne!(ok.is_error, Some(true), "{}", text_of(&ok));
    let ok_bytes = text_of(&ok).len();

    let is_error = call(&client, "delete", json!({"repo_id": repo_id}))
        .await
        .unwrap();
    assert_eq!(is_error.is_error, Some(true));
    let is_error_bytes = text_of(&is_error).len();

    let protocol = call(&client, "search_code", json!({"query": "  "})).await;
    assert!(protocol.is_err(), "an empty query is invalid_params");

    let spans = captured.wait_for(3).await;
    assert_eq!(spans.len(), 3, "one span per call: {spans:#?}");
    let by_tool = |tool: &str| spans.iter().find(|s| s["tool"] == tool).unwrap().clone();

    let s = by_tool("find_callers");
    assert_eq!(s["target"], "myceliums_mcp::calls");
    assert_eq!(s["level"], "INFO");
    assert_eq!(s["toolset"], "core");
    assert_eq!(s["repo"], repo_id);
    assert_eq!(s["outcome"], "ok");
    assert_eq!(s["result_bytes"], ok_bytes.to_string());
    assert_eq!(s["approx_tokens"], ok_bytes.div_ceil(4).to_string());
    let duration: f64 = s["duration_ms"].parse().unwrap();
    assert!(duration >= 0.0, "{s:?}");

    let s = by_tool("delete");
    assert_eq!(s["toolset"], "admin");
    assert_eq!(s["outcome"], "is_error");
    assert_eq!(s["result_bytes"], is_error_bytes.to_string());

    let s = by_tool("search_code");
    assert_eq!(s["outcome"], "protocol_error");
    assert!(!s.contains_key("repo"), "no repo_id argument: {s:?}");
}

#[tokio::test]
async fn server_stats_aggregates_the_calls_of_this_server() {
    let _turn = serial().await;
    let repo_id = analyzed_repo().await;

    // Core only: `server_stats` is opt-in.
    let core = connect(Toolsets::core()).await;
    let hidden = call(&core, "server_stats", json!({})).await;
    assert!(hidden.is_err(), "server_stats is not a core tool");

    let client = connect("admin".parse().unwrap()).await;
    for _ in 0..3 {
        call(&client, "list_repos", json!({})).await.unwrap();
    }
    call(
        &client,
        "find_callees",
        json!({"repo_id": repo_id, "symbol_name": "nonexistentSymbolXyz"}),
    )
    .await
    .unwrap();
    let _ = call(&client, "search_code", json!({"query": ""})).await;
    let _ = call(&client, "no_such_tool", json!({})).await;

    let stats = call(&client, "server_stats", json!({})).await.unwrap();
    let text = text_of(&stats);
    assert!(text.contains("local only"), "{text}");
    let row = |tool: &str| {
        text.lines()
            .find(|l| l.starts_with(&format!("{tool} |")))
            .unwrap_or_else(|| panic!("no row for {tool}: {text}"))
            .split(" | ")
            .map(str::to_string)
            .collect::<Vec<_>>()
    };
    // tool | toolset | calls | isError | protocol errors | cancelled | ...
    let list = row("list_repos");
    assert_eq!(&list[1..6], ["core", "3", "0", "0", "0"]);
    let bytes: u64 = list[9].parse().unwrap();
    assert!(bytes > 0, "{text}");
    assert_eq!(list[10], bytes.div_ceil(4).to_string());
    assert_eq!(&row("find_callees")[2..5], ["1", "1", "0"]);
    assert_eq!(&row("search_code")[2..5], ["1", "0", "1"]);
    assert_eq!(&row("(unknown tool)")[2..5], ["1", "0", "1"]);
    // The stats call itself is not finished while it renders.
    assert!(!text.contains("server_stats |"), "{text}");
}

/// A JSON-RPC session over raw lines, so the request can omit or set the
/// `progressToken` itself (rmcp's client always sets one).
async fn raw_analyze(meta: Option<Value>) -> (Value, Vec<Value>) {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_with_toolsets_for_test(&Toolsets::core())
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    let (read, mut write) = tokio::io::split(client_io);
    let mut lines = BufReader::new(read).lines();
    let line = |msg: Value| format!("{msg}\n");
    for msg in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "raw", "version": "0"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ] {
        write.write_all(line(msg).as_bytes()).await.unwrap();
    }
    let mut params = json!({
        "name": "analyze",
        "arguments": {
            "path": env().project.to_string_lossy(),
            "force": true,
            "skip_embeddings": true
        }
    });
    if let Some(meta) = meta {
        params["_meta"] = meta;
    }
    let request = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": params});
    write.write_all(line(request).as_bytes()).await.unwrap();

    let mut progress = Vec::new();
    loop {
        let line = lines.next_line().await.unwrap().expect("server answers");
        let msg: Value = serde_json::from_str(&line).unwrap();
        if msg["method"] == "notifications/progress" {
            progress.push(msg["params"].clone());
        } else if msg["id"] == 2 {
            return (msg, progress);
        }
    }
}

#[tokio::test]
async fn analyze_reports_progress_only_when_the_client_asks() {
    let _turn = serial().await;
    env();

    let (response, progress) = raw_analyze(None).await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert!(
        progress.is_empty(),
        "no token, no notifications: {progress:?}"
    );

    let (response, progress) = raw_analyze(Some(json!({"progressToken": "tok-98"}))).await;
    assert_eq!(response["result"]["isError"], false, "{response}");
    assert!(progress.len() >= 4, "{progress:#?}");
    let values: Vec<f64> = progress
        .iter()
        .map(|p| {
            assert_eq!(p["progressToken"], "tok-98");
            assert_eq!(p["total"], 100.0);
            assert!(p["message"].as_str().is_some_and(|m| !m.is_empty()), "{p}");
            p["progress"].as_f64().unwrap()
        })
        .collect();
    assert!(
        values.windows(2).all(|w| w[0] < w[1]),
        "progress increases: {values:?}"
    );
    assert_eq!(values.last(), Some(&100.0), "the last one says done");
    let messages: Vec<&str> = progress
        .iter()
        .map(|p| p["message"].as_str().unwrap())
        .collect();
    assert!(
        messages.contains(&"replacing the previous index"),
        "{messages:?}"
    );
}
