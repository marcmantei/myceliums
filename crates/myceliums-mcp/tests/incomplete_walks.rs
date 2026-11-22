//! Graph walks that stop early say so (#169, on top of the #96 contract).
//!
//! The #163 outcome study lost sessions where an agent answered from one
//! `trace_flow` result that was partial without saying so. Here every walk
//! that can stop early — `trace_flow`, `find_callers`/`find_callees` with a
//! depth, `impact_of_change` — is driven over a real MCP session against a
//! project built for it: a chain longer than the default window, a call
//! through a callback parameter, and a method name three classes share (the
//! #139 cap links at most two). Each cut must be named in an `Incomplete`
//! block (text) or `incomplete` object (JSON) with the exact call that
//! continues, and following that call must lift the cut it named.
//!
//! This binary owns its data directory, so it may analyze.

use myceliums_mcp::{AnalyzeParams, MyceliumsMcp, Toolsets};
use rmcp::model::CallToolRequestParams;
use rmcp::service::{RoleClient, RunningService};
use rmcp::ServiceExt;
use serde_json::{json, Value};
use std::path::Path;
use std::process::{Command, Stdio};

type Client = RunningService<RoleClient, ()>;

const FLOW: &str = r#"export function entry(cb: () => void): void {
  step1();
  finish();
  cb();
}

function step1(): void {
  step2();
}

function step2(): void {
  step3();
}

function step3(): void {
  finish();
}

export function finish(): void {}

export function poll(worker): void {
  worker.run();
}
"#;

const JOBS: &str = r#"export class Job {
  run(): void {}
}

export class Task {
  run(): void {}
}

export class Cron {
  run(): void {}
}
"#;

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

async fn connect() -> Client {
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_with_toolsets_for_test(&Toolsets::all())
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &Client, tool: &str, args: Value) -> (String, Option<Value>) {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    let result = client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
        .expect("no protocol error");
    let text = result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default();
    assert_ne!(result.is_error, Some(true), "{tool}: {text}");
    (text, result.structured_content)
}

/// The text after the `Index:` line and the blank line under it.
fn body(text: &str) -> &str {
    assert!(text.starts_with("Index: "), "{text}");
    text.split_once("\n\n").map_or(text, |(_, rest)| rest)
}

/// The `Next:` line of an `Incomplete` block.
fn next_line(text: &str) -> &str {
    text.lines()
        .find_map(|l| l.strip_prefix("Next: "))
        .unwrap_or_else(|| panic!("no Next line: {text}"))
}

/// The arguments of the first `tool(...)` call in a `Next` step, as JSON.
fn next_args(next: &str, tool: &str) -> Value {
    let start = next.find(&format!("{tool}(")).expect("names the tool") + tool.len() + 1;
    let inner = &next[start..start + next[start..].find(')').expect("closed")];
    let mut args = serde_json::Map::new();
    for part in inner.split(", ") {
        let (key, value) = part.split_once('=').expect("key=value");
        let value: Value = serde_json::from_str(value).expect("a JSON value");
        args.insert(key.to_string(), value);
    }
    Value::Object(args)
}

#[tokio::test]
async fn walks_that_stop_early_say_why_and_how_to_continue() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());
    std::env::set_var("HOME", data.path());
    let projects = tempfile::tempdir().unwrap();
    let project = projects.path().join("flows");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("src/flow.ts"), FLOW).unwrap();
    std::fs::write(project.join("src/jobs.ts"), JOBS).unwrap();
    git(&project, &["init", "-q"]);
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "init"]);
    let project = project.canonicalize().unwrap();
    let repo = myceliums_core::analyzer::repo_id_from_path(&project);
    MyceliumsMcp::new()
        .analyze_for_test(AnalyzeParams {
            path: project.to_string_lossy().into_owned(),
            skip_embeddings: Some(true),
            ..Default::default()
        })
        .await
        .expect("analyze");
    let client = connect().await;

    // ── trace_flow: a chain found, longer chains left out ────────────────
    let (flow, _) = call(
        &client,
        "trace_flow",
        json!({"repo_id": repo, "from_symbol": "entry", "to_symbol": "finish"}),
    )
    .await;
    let lines: Vec<&str> = body(&flow).lines().collect();
    assert!(
        lines[0].starts_with("Shortest call chain") && lines[0].ends_with(": 1 calls."),
        "{flow}"
    );
    // The cut is stated right under the first line, before the chain.
    assert!(lines[2].starts_with("Incomplete ("), "{flow}");
    assert!(
        flow.contains("3 more symbol(s) lie on longer chains of 4-4 calls"),
        "{flow}"
    );
    // The callback call made on the chain is named with its line.
    assert!(
        flow.contains("through a variable or callback") && flow.contains("`entry` -> `cb` ("),
        "{flow}"
    );
    let next = next_line(&flow);
    let mut args = next_args(next, "trace_flow");
    assert_eq!(args["extra_calls"], 3, "{next}");
    args["repo_id"] = json!(repo);
    let (wider, _) = call(&client, "trace_flow", args).await;
    assert!(!wider.contains("longer chains"), "{wider}");
    for step in ["step1", "step2", "step3"] {
        assert!(wider.contains(&format!("* {step} (Function)")), "{wider}");
    }
    // The callback is still a place the flow may go on unseen.
    assert!(wider.contains("`entry` -> `cb` ("), "{wider}");

    // ── trace_flow: no chain within max_depth ────────────────────────────
    let (none, _) = call(
        &client,
        "trace_flow",
        json!({"repo_id": repo, "from_symbol": "entry", "to_symbol": "step3", "max_depth": 2}),
    )
    .await;
    assert!(body(&none).starts_with("No call chain"), "{none}");
    assert!(
        none.contains("depth limit 2: 1 more symbols one step further, through 1 symbol(s) at depth 2: `step2`"),
        "{none}"
    );
    let mut args = next_args(next_line(&none), "trace_flow");
    assert_eq!(args["max_depth"], 4);
    args["repo_id"] = json!(repo);
    let (found, _) = call(&client, "trace_flow", args).await;
    assert!(body(&found).starts_with("Shortest call chain"), "{found}");
    assert!(found.contains(": 3 calls."), "{found}");

    // ── find_callees: depth limit and unlinked calls ─────────────────────
    let (_, callees) = call(
        &client,
        "find_callees",
        json!({"repo_id": repo, "symbol_name": "entry", "depth": 2}),
    )
    .await;
    let callees = callees.expect("structured");
    assert_eq!(callees["depth_limited"], true, "{callees}");
    let incomplete = &callees["incomplete"];
    let reasons = incomplete["reasons"].as_array().unwrap();
    assert!(reasons[0]
        .as_str()
        .unwrap()
        .starts_with("depth limit 2: 1 more callees"));
    assert!(reasons[1].as_str().unwrap().contains("`entry` -> `cb`"));
    assert_eq!(incomplete["branches_cut"], 2);
    let next = incomplete["next"].as_str().unwrap();
    assert!(
        next.starts_with(r#"find_callees(symbol_name="entry", depth=3)"#),
        "{next}"
    );
    let (_, deeper) = call(
        &client,
        "find_callees",
        json!({"repo_id": repo, "symbol_name": "entry", "depth": 3}),
    )
    .await;
    let deeper = deeper.unwrap();
    assert_eq!(deeper["depth_limited"], false);
    assert!(!deeper["incomplete"]["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r.as_str().unwrap().starts_with("depth limit")));

    // A page cut is a reason too, and its call fetches the rest.
    let (_, first) = call(
        &client,
        "find_callees",
        json!({"repo_id": repo, "symbol_name": "entry", "depth": 3, "limit": 2}),
    )
    .await;
    let first = first.unwrap();
    let next = first["incomplete"]["next"].as_str().unwrap();
    assert!(
        next.starts_with(r#"find_callees(symbol_name="entry", depth=3, offset=2)"#),
        "{next}"
    );

    // Direct callees: the depth limit is the question, not a cut; the
    // ambiguous method call (three `run` methods) is one.
    let (_, poll) = call(
        &client,
        "find_callees",
        json!({"repo_id": repo, "symbol_name": "poll"}),
    )
    .await;
    let poll = poll.unwrap();
    assert_eq!(poll["total_count"], 0, "{poll}");
    let reasons = poll["incomplete"]["reasons"].as_array().unwrap();
    assert_eq!(reasons.len(), 1, "{poll}");
    assert!(
        reasons[0]
            .as_str()
            .unwrap()
            .contains("1 more than 2 candidates"),
        "{poll}"
    );

    // ── find_callers: nothing cut says nothing; unlinked sites say so ────
    let (_, callers) = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "finish"}),
    )
    .await;
    let callers = callers.unwrap();
    assert_eq!(callers["total_count"], 2);
    assert!(callers.get("incomplete").is_none(), "{callers}");
    let (_, run) = call(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": "Job.run"}),
    )
    .await;
    let run = run.unwrap();
    let incomplete = &run["incomplete"];
    assert!(
        incomplete["reasons"][0]
            .as_str()
            .unwrap()
            .starts_with("1 call site(s) named like a symbol here got no CALLS edge"),
        "{run}"
    );
    assert!(incomplete["next"].as_str().unwrap().contains("`run(`"));

    // ── impact_of_change: the depth limit ────────────────────────────────
    let (impact, _) = call(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "symbol_name": "finish", "depth": 1}),
    )
    .await;
    let lines: Vec<&str> = body(&impact).lines().collect();
    assert!(
        lines[0].starts_with("Impact of changing finish"),
        "{impact}"
    );
    assert!(lines[2].starts_with("Incomplete ("), "{impact}");
    assert!(
        impact.contains("depth limit 1: 1 more dependents"),
        "{impact}"
    );
    let mut args = next_args(next_line(&impact), "impact_of_change");
    assert_eq!(args["depth"], 2);
    args["repo_id"] = json!(repo);
    let (deeper, _) = call(&client, "impact_of_change", args).await;
    assert!(
        deeper.contains("step2") && !deeper.contains("depth limit 1"),
        "{deeper}"
    );
}
