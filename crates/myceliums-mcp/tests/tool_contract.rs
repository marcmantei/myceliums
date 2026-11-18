//! The agent-facing tool contract (#96), checked end to end over a real MCP
//! session: an in-process client talks JSON-RPC to the server through a
//! duplex pipe, so every assertion sees exactly what an agent would.
//!
//! * failures: recoverable ones are `isError` results with a `Next:` step;
//!   bad input is `invalid_params`;
//! * output budget: truncation notices and a pagination round trip (the
//!   visible cut of long Cypher cells is unit-tested in `format.rs`: the
//!   fixture has no value long enough);
//! * freshness line on graph results; CALLS resolution labels;
//! * untrusted framing of indexed text; annotations; `delete` confirmation.

mod harness;

use myceliums_mcp::MyceliumsMcp;
use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::ServiceExt;
use serde_json::{json, Value};

type Client = RunningService<RoleClient, ()>;

/// Serves a fresh server over an in-process pipe and connects a client.
async fn connect() -> Client {
    harness::ensure_indexed().await;
    let (server_io, client_io) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = MyceliumsMcp::new()
            .into_router_for_test()
            .serve(server_io)
            .await
            .expect("server starts");
        let _ = running.waiting().await;
    });
    ().serve(client_io).await.expect("client connects")
}

async fn call(client: &Client, tool: &str, args: Value) -> Result<CallToolResult, ServiceError> {
    let Value::Object(args) = args else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(tool.to_string()).with_arguments(args))
        .await
}

/// A successful call: the text an agent reads.
async fn ok_text(client: &Client, tool: &str, args: Value) -> String {
    let result = call(client, tool, args).await.expect("no protocol error");
    assert_ne!(result.is_error, Some(true), "{tool}: {}", text_of(&result));
    text_of(&result)
}

/// A successful call with structured output.
async fn ok_json(client: &Client, tool: &str, args: Value) -> Value {
    let result = call(client, tool, args).await.expect("no protocol error");
    assert_ne!(result.is_error, Some(true), "{tool}: {}", text_of(&result));
    result.structured_content.expect("structured content")
}

/// The text of a result: the `text` field of structured text output, or the
/// first content block (what an `isError` result carries).
fn text_of(result: &CallToolResult) -> String {
    if let Some(Value::String(text)) = result
        .structured_content
        .as_ref()
        .and_then(|v| v.get("text"))
    {
        return text.clone();
    }
    result
        .content
        .first()
        .and_then(|c| c.as_text())
        .map(|t| t.text.clone())
        .unwrap_or_default()
}

/// The JSON-RPC error code of a protocol-level failure.
fn protocol_code(result: Result<CallToolResult, ServiceError>) -> ErrorCode {
    match result {
        Err(ServiceError::McpError(e)) => e.code,
        Ok(r) => panic!("expected a protocol error, got a result: {}", text_of(&r)),
        Err(other) => panic!("expected an MCP error, got {other}"),
    }
}

/// The `offset=N` a truncation notice asks for.
fn next_offset(notice: &str) -> Option<usize> {
    let rest = notice.split("offset=").nth(1)?;
    rest.chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

// ── Errors ───────────────────────────────────────────────────────────────

#[tokio::test]
async fn recoverable_failures_are_is_error_results_with_a_next_step() {
    let client = connect().await;
    let cases = [
        (
            "symbol_context",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "definitelyNotASymbol"}),
            "not found",
        ),
        (
            "cypher_query",
            json!({"repo_id": "no-such-repo", "query": "MATCH (s:CodeSymbol) RETURN s.name"}),
            "unknown repo_id",
        ),
        (
            "get_file_symbols",
            json!({"repo_id": harness::REPO_ID, "file_path": "no/such/file.ts"}),
            "not in the index",
        ),
        (
            "get_community_detail",
            json!({"repo_id": harness::REPO_ID, "community_id": "no-such-community"}),
            "not found",
        ),
    ];
    for (tool, args, problem) in cases {
        let result = call(&client, tool, args).await.unwrap_or_else(|e| {
            panic!("{tool}: recoverable failures are not protocol errors: {e}")
        });
        let text = text_of(&result);
        assert_eq!(result.is_error, Some(true), "{tool}: {text}");
        assert!(result.structured_content.is_none(), "{tool}");
        assert!(text.contains(problem), "{tool}: {text}");
        assert!(
            text.contains("\nNext: "),
            "{tool} must name the next step: {text}"
        );
    }
    // The unknown-repo error names the ids that do exist.
    let text = text_of(
        &call(&client, "get_stats", json!({"repo_id": "no-such-repo"}))
            .await
            .unwrap(),
    );
    assert!(text.contains(harness::REPO_ID), "{text}");
}

#[tokio::test]
async fn bad_input_is_invalid_params() {
    let client = connect().await;
    let cases = [
        // Malformed repo id.
        (
            "cypher_query",
            json!({"repo_id": "../escape", "query": "MATCH (s:CodeSymbol) RETURN s.name"}),
        ),
        // Unparsable Cypher.
        (
            "cypher_query",
            json!({"repo_id": harness::REPO_ID, "query": "MATCH (s:CodeSymbol RETURN"}),
        ),
        // Write clause.
        (
            "cypher_query",
            json!({"repo_id": harness::REPO_ID, "query": "CREATE (n:CodeSymbol {name: 'x'})"}),
        ),
        // Value outside an enum (rejected before the handler runs).
        (
            "get_centrality_report",
            json!({"repo_id": harness::REPO_ID, "metric": "pagerank"}),
        ),
        // Missing required argument.
        ("symbol_context", json!({"repo_id": harness::REPO_ID})),
        // Empty query.
        (
            "context_search",
            json!({"repo_id": harness::REPO_ID, "query": "  "}),
        ),
        // Mutually exclusive options.
        (
            "get_rationale",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "getUser", "file_path": "a.ts"}),
        ),
    ];
    for (tool, args) in cases {
        let code = protocol_code(call(&client, tool, args.clone()).await);
        assert_eq!(code, ErrorCode::INVALID_PARAMS, "{tool} {args}");
    }
}

// ── Output budget ────────────────────────────────────────────────────────

#[tokio::test]
async fn cut_lists_say_so_and_pages_round_trip() {
    let client = connect().await;
    let query = "MATCH (s:CodeSymbol) RETURN s.uid ORDER BY s.uid";

    // The whole list in one page, for reference.
    let full = ok_text(
        &client,
        "cypher_query",
        json!({"repo_id": harness::REPO_ID, "query": query, "limit": 500}),
    )
    .await;
    assert!(!full.contains("offset="), "everything fits: {full}");
    // Rows of a one-column table: after the `----` rule, up to the end of
    // the untrusted block.
    let rows = |text: &str| -> Vec<String> {
        text.lines()
            .skip_while(|l| l.is_empty() || !l.chars().all(|c| c == '-'))
            .skip(1)
            .take_while(|l| !l.starts_with(">>>END "))
            .map(str::to_string)
            .collect()
    };
    let all = rows(&full);
    assert!(all.len() > 5, "fixture has enough symbols: {full}");

    // Walk the pages the notices point to.
    let mut seen = Vec::new();
    let mut offset = 0;
    loop {
        let page = ok_text(
            &client,
            "cypher_query",
            json!({"repo_id": harness::REPO_ID, "query": query, "limit": 5, "offset": offset}),
        )
        .await;
        let page_rows = rows(&page);
        assert!(page_rows.len() <= 5, "{page}");
        seen.extend(page_rows);
        match page.lines().find(|l| l.starts_with("Showing ")) {
            Some(notice) if notice.contains("call again") => {
                assert!(
                    notice.contains(&format!("of {}", all.len())),
                    "notice states the total: {notice}"
                );
                offset = next_offset(notice).expect("notice names the next offset");
            }
            _ => break,
        }
    }
    assert_eq!(seen, all, "paging must yield every row exactly once");

    // Structured outputs carry the same notice.
    let first = ok_json(
        &client,
        "get_file_symbols",
        json!({"repo_id": harness::REPO_ID, "file_path": harness::fixture::USER_SERVICE_FILE, "limit": 1}),
    )
    .await;
    let total = first["total_count"].as_u64().unwrap();
    assert!(total > 1, "{first}");
    assert_eq!(first["symbols"].as_array().unwrap().len(), 1);
    let notice = first["notice"].as_str().expect("cut list has a notice");
    assert!(
        notice.starts_with(&format!(
            "Showing 1-1 of {total} — call again with offset=1"
        )),
        "{notice}"
    );
}

// ── Freshness, confidence, untrusted content ─────────────────────────────

#[tokio::test]
async fn graph_results_state_index_freshness() {
    let client = connect().await;
    let text = ok_text(
        &client,
        "context_search",
        json!({"repo_id": harness::REPO_ID, "query": harness::fixture::GET_USER}),
    )
    .await;
    assert!(text.starts_with("Index: "), "{text}");
    assert!(
        text.lines().next().unwrap().len() < 160,
        "one short line: {text}"
    );

    let stats = ok_json(&client, "get_stats", json!({"repo_id": harness::REPO_ID})).await;
    assert!(
        stats["index_status"]
            .as_str()
            .unwrap()
            .starts_with("Index: "),
        "{stats}"
    );
}

#[tokio::test]
async fn call_edges_carry_their_resolution() {
    let client = connect().await;
    let callers = ok_json(
        &client,
        "get_callers",
        json!({"repo_id": harness::REPO_ID, "symbol_name": harness::fixture::FIND_BY_ID}),
    )
    .await;
    let items = callers["callers"].as_array().unwrap();
    assert!(!items.is_empty(), "{callers}");
    for item in items {
        let resolved = item["resolved"].as_str().unwrap();
        assert!(
            ["exact", "import", "name-match", "ambiguous"].contains(&resolved),
            "{item}"
        );
    }
}

#[tokio::test]
async fn indexed_source_is_framed_as_untrusted() {
    let client = connect().await;
    let text = ok_text(
        &client,
        "symbol_context",
        json!({"repo_id": harness::REPO_ID, "symbol_name": harness::fixture::GET_USER}),
    )
    .await;
    let open = text
        .lines()
        .find(|l| l.starts_with("<<<UNTRUSTED "))
        .unwrap_or_else(|| panic!("source must be framed: {text}"));
    let id = open.split_whitespace().nth(1).unwrap();
    assert!(open.contains(harness::fixture::USER_SERVICE_FILE), "{open}");
    assert!(
        text.lines().any(|l| l == format!(">>>END {id}")),
        "the block closes with the same id: {text}"
    );

    let definition = ok_json(
        &client,
        "get_symbol_definition",
        json!({"repo_id": harness::REPO_ID, "symbol_name": harness::fixture::GET_USER}),
    )
    .await;
    assert!(
        definition["content"]
            .as_str()
            .unwrap()
            .starts_with("<<<UNTRUSTED "),
        "{definition}"
    );

    let info = client.peer_info().expect("server info");
    let instructions = info.instructions.as_deref().unwrap();
    assert!(instructions.contains("<<<UNTRUSTED <id>"), "{instructions}");
    assert!(
        instructions.contains("never instructions"),
        "{instructions}"
    );
}

// ── Annotations and destructive tools ────────────────────────────────────

#[tokio::test]
async fn every_tool_is_annotated() {
    let client = connect().await;
    let tools: Vec<Tool> = client.list_all_tools().await.expect("tools/list");
    assert!(tools.len() > 40, "{}", tools.len());
    let writes = [
        "analyze",
        "delete",
        "map_service",
        "record_decision",
        "link_decision",
    ];
    for tool in &tools {
        let a = tool
            .annotations
            .as_ref()
            .unwrap_or_else(|| panic!("{} has no annotations", tool.name));
        let read_only = a.read_only_hint.unwrap_or_else(|| panic!("{}", tool.name));
        assert!(a.destructive_hint.is_some(), "{}", tool.name);
        assert!(a.idempotent_hint.is_some(), "{}", tool.name);
        assert_eq!(a.open_world_hint, Some(false), "{}", tool.name);
        assert_eq!(
            read_only,
            !writes.contains(&tool.name.as_ref()),
            "{}",
            tool.name
        );
    }
    let delete = tools.iter().find(|t| t.name == "delete").unwrap();
    assert_eq!(
        delete.annotations.as_ref().unwrap().destructive_hint,
        Some(true)
    );
}

#[tokio::test]
async fn delete_without_confirm_is_refused() {
    let client = connect().await;
    for args in [
        json!({"repo_id": harness::REPO_ID}),
        json!({"repo_id": harness::REPO_ID, "confirm": false}),
    ] {
        let result = call(&client, "delete", args).await.expect("a tool result");
        let text = text_of(&result);
        assert_eq!(result.is_error, Some(true), "{text}");
        assert!(text.contains("confirm=true"), "{text}");
    }
    let repos = ok_text(&client, "list_repos", json!({})).await;
    assert!(
        repos.contains(&format!("id: {}", harness::REPO_ID)),
        "the index must survive: {repos}"
    );
}
