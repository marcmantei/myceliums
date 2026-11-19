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
//! * untrusted framing of indexed text; annotations; `delete` confirmation;
//! * output channels (#97): text tools send plain text only, structured
//!   tools typed JSON with an output schema; toolsets; the core tools.

mod harness;

use myceliums_mcp::{MyceliumsMcp, Toolsets};
use rmcp::model::{CallToolRequestParams, CallToolResult, ErrorCode, Tool};
use rmcp::service::{RoleClient, RunningService, ServiceError};
use rmcp::ServiceExt;
use serde_json::{json, Value};

type Client = RunningService<RoleClient, ()>;

/// Serves a fresh server with every toolset over an in-process pipe and
/// connects a client.
async fn connect() -> Client {
    connect_with(Toolsets::all()).await
}

/// Same, exposing only `toolsets`.
async fn connect_with(toolsets: Toolsets) -> Client {
    harness::ensure_indexed().await;
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

/// The text of a result: its first content block (the whole answer of a
/// text tool, the JSON of a structured one, the message of an `isError`).
fn text_of(result: &CallToolResult) -> String {
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

/// Rows of a one-column Cypher table: after the `----` rule, up to the end
/// of the untrusted block.
fn table_rows(text: &str) -> Vec<String> {
    text.lines()
        .skip_while(|l| l.is_empty() || !l.chars().all(|c| c == '-'))
        .skip(1)
        .take_while(|l| !l.starts_with(">>>END "))
        .map(str::to_string)
        .collect()
}

/// Rows of a multi-column Cypher table (same bounds as [`table_rows`]).
fn any_table_rows(text: &str) -> Vec<String> {
    text.lines()
        .skip_while(|l| l.is_empty() || !l.chars().all(|c| c == '-' || c == ' '))
        .skip(1)
        .take_while(|l| !l.starts_with(">>>END "))
        .map(str::to_string)
        .collect()
}

/// `  * item` lines (impact reports), tagged with their section heading.
fn bullet_items(text: &str) -> Vec<String> {
    let mut section = String::new();
    let mut items = Vec::new();
    for line in text.lines() {
        if let Some(item) = line.strip_prefix("  * ") {
            items.push(format!("{section}|{item}"));
        } else if line.ends_with(':') {
            section = line.to_string();
        }
    }
    items
}

/// Follows the `offset=` each page's notice names, from offset 0 to the
/// end, and returns every item seen, in order.
async fn walk_pages(
    client: &Client,
    tool: &str,
    mut args: Value,
    limit: usize,
    items_of: fn(&str) -> Vec<String>,
) -> Vec<String> {
    let mut seen = Vec::new();
    let mut offset = 0;
    for _ in 0..100 {
        args["limit"] = json!(limit);
        args["offset"] = json!(offset);
        let page = ok_text(client, tool, args.clone()).await;
        let items = items_of(&page);
        assert!(items.len() <= limit * 3, "{tool} page over budget: {page}");
        seen.extend(items);
        match page.lines().find(|l| l.contains("call again with offset=")) {
            Some(notice) => offset = next_offset(notice).expect("notice names an offset"),
            None => return seen,
        }
    }
    panic!("{tool}: paging did not end");
}

/// Pages walked one at a time must add up to the single big page: nothing
/// twice, nothing missing — even where the core produces hash-map order.
async fn assert_pages_partition(
    client: &Client,
    tool: &str,
    args: Value,
    items_of: fn(&str) -> Vec<String>,
) {
    let mut whole_args = args.clone();
    whole_args["limit"] = json!(500);
    let whole = items_of(&ok_text(client, tool, whole_args).await);
    assert!(whole.len() > 2, "{tool}: too few items to page: {whole:?}");
    // Several walks: an unstable order would show up as overlap or gaps.
    for _ in 0..3 {
        let paged = walk_pages(client, tool, args.clone(), 2, items_of).await;
        let mut unique = paged.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            paged.len(),
            "{tool}: an item came twice: {paged:?}"
        );
        assert_eq!(paged, whole, "{tool}");
    }
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
            "explain_symbol",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "definitelyNotASymbol"}),
            "not found",
        ),
        (
            "find_callers",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "definitelyNotASymbol"}),
            "not found",
        ),
        (
            "trace_flow",
            json!({"repo_id": harness::REPO_ID, "from_symbol": "main", "to_symbol": "nope"}),
            "not found",
        ),
        (
            "impact_of_change",
            json!({"repo_id": harness::REPO_ID, "file_path": "no/such/file.ts"}),
            "not in the index",
        ),
        (
            // The harness index records no analysis snapshot.
            "detect_architecture_drift",
            json!({"repo_id": harness::REPO_ID}),
            "nothing to compare",
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
        (
            "get_git_context",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "zzzNoSuchSymbolzzz"}),
            "No symbol name contains",
        ),
        (
            "link_decision",
            json!({"repo_id": harness::REPO_ID, "decision_id": "no-such-adr", "symbol_name": "getUser"}),
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
        (
            "impact_of_change",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "getUser", "diff": "x"}),
        ),
        // Unknown search mode.
        (
            "search_code",
            json!({"repo_id": harness::REPO_ID, "query": "user", "mode": "fuzzy"}),
        ),
        // A chain needs two different ends.
        (
            "trace_flow",
            json!({"repo_id": harness::REPO_ID, "from_symbol": "main", "to_symbol": "main"}),
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
    let rows = table_rows;
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

#[tokio::test]
async fn pages_partition_lists_built_from_hash_maps() {
    let client = connect().await;
    let diff = "\
diff --git a/src/services/user.ts b/src/services/user.ts
--- a/src/services/user.ts
+++ b/src/services/user.ts
@@ -16,3 +16,4 @@ export class UserService {
     getUser(id: string): User | null {
+        // touched
         return this.db.findById('users', id);
     }
";
    assert_pages_partition(
        &client,
        "impact_of_change",
        json!({"repo_id": harness::REPO_ID, "diff": diff}),
        bullet_items,
    )
    .await;
    // Symbol mode pages over dependents and files the same way.
    assert_pages_partition(
        &client,
        "impact_of_change",
        json!({"repo_id": harness::REPO_ID, "symbol_name": harness::fixture::FIND_BY_ID}),
        bullet_items,
    )
    .await;
    assert_pages_partition(
        &client,
        "cypher_query",
        json!({
            "repo_id": harness::REPO_ID,
            "query": "MATCH (s:CodeSymbol) RETURN s.kind, count(s)"
        }),
        any_table_rows,
    )
    .await;
    // `get_dependencies` sorts its hash-set lists the same way, but the
    // fixture has no file-level IMPORTS edges to page through.
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
        "explain_symbol",
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

    // Search tables carry indexed names and signatures (email subjects,
    // Markdown headings): framed once per response.
    let search = ok_text(
        &client,
        "context_search",
        json!({"repo_id": harness::REPO_ID, "query": "user"}),
    )
    .await;
    assert!(
        search.contains("<<<UNTRUSTED ") && search.contains(" search results\n"),
        "{search}"
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
    let repos = ok_json(&client, "list_repos", json!({})).await;
    assert!(
        repos["repos"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["id"] == harness::REPO_ID),
        "the index must survive: {repos}"
    );
}

// ── Output channels (#97) ────────────────────────────────────────────────

#[tokio::test]
async fn text_tools_send_plain_text_and_structured_tools_typed_json() {
    let client = connect().await;
    let tools: Vec<Tool> = client.list_all_tools().await.expect("tools/list");
    let schema_of = |name: &str| {
        tools
            .iter()
            .find(|t| t.name == name)
            .unwrap_or_else(|| panic!("{name} listed"))
            .output_schema
            .clone()
    };

    // A text tool: the answer is the text content itself — no JSON
    // envelope, no structuredContent, no output schema.
    let result = call(
        &client,
        "explain_symbol",
        json!({"repo_id": harness::REPO_ID, "symbol_name": harness::fixture::GET_USER}),
    )
    .await
    .unwrap();
    assert!(result.structured_content.is_none());
    assert_eq!(result.content.len(), 1);
    let text = text_of(&result);
    assert!(text.starts_with("Index: "), "{text}");
    assert!(serde_json::from_str::<Value>(&text).is_err(), "{text}");
    assert!(schema_of("explain_symbol").is_none());
    for tool in &tools {
        let schema = tool.output_schema.as_ref();
        let wraps_text = schema
            .and_then(|s| s.get("properties"))
            .and_then(|p| p.as_object())
            .is_some_and(|p| p.len() == 1 && p.contains_key("text"));
        assert!(!wraps_text, "{} still wraps its text in JSON", tool.name);
    }

    // A structured tool: typed structuredContent, the same JSON as text
    // content (MCP spec), and an output schema that describes it.
    let result = call(
        &client,
        "find_callers",
        json!({"repo_id": harness::REPO_ID, "symbol_name": harness::fixture::FIND_BY_ID}),
    )
    .await
    .unwrap();
    let structured = result.structured_content.clone().expect("structured");
    let as_text: Value = serde_json::from_str(&text_of(&result)).expect("JSON text");
    assert_eq!(as_text, structured);
    let schema = schema_of("find_callers").expect("output schema");
    let properties = schema["properties"].as_object().unwrap();
    for key in structured.as_object().unwrap().keys() {
        assert!(properties.contains_key(key), "{key} not in the schema");
    }
    assert!(schema_of("list_repos").is_some());
}

// ── Toolsets ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn the_default_profile_is_the_core_toolset_loaded_up_front() {
    let client = connect_with(Toolsets::default()).await;
    let tools: Vec<Tool> = client.list_all_tools().await.expect("tools/list");
    let mut names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "analyze",
            "explain_symbol",
            "find_callees",
            "find_callers",
            "find_dead_code",
            "impact_of_change",
            "list_repos",
            "search_code",
            "trace_flow",
        ]
    );
    for tool in &tools {
        let meta = tool.meta.as_ref().expect("core tools carry _meta");
        assert_eq!(meta.0["anthropic/alwaysLoad"], json!(true), "{}", tool.name);
    }
    // The server instructions name only tools this profile has.
    let info = client.peer_info().expect("server info");
    let instructions = info.instructions.as_deref().unwrap();
    for word in instructions.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')) {
        if word.contains('_') && word.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
            assert!(
                names.contains(&word),
                "instructions name {word}, which the core profile lacks"
            );
        }
    }
    // Tools outside the profile do not exist for this client.
    let code = protocol_code(
        call(
            &client,
            "get_callers",
            json!({"repo_id": harness::REPO_ID, "symbol_name": "getUser"}),
        )
        .await,
    );
    assert_eq!(code, ErrorCode::INVALID_PARAMS);
}

// ── Core tools ───────────────────────────────────────────────────────────

#[tokio::test]
async fn core_tools_answer_the_common_structural_questions() {
    let client = connect_with(Toolsets::default()).await;
    let repo = harness::REPO_ID;

    // Who calls findById? Direct callers by default, deeper on request.
    let direct = ok_json(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": harness::fixture::FIND_BY_ID}),
    )
    .await;
    let names = |v: &Value, key: &str| -> Vec<String> {
        v[key]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&direct, "callers"), ["getUser"], "{direct}");
    let deep = ok_json(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": harness::fixture::FIND_BY_ID, "depth": 2}),
    )
    .await;
    let main = deep["callers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "main")
        .unwrap_or_else(|| panic!("main calls getUser: {deep}"));
    assert_eq!(main["depth"], 2);
    assert_eq!(main["via"], "UserService.getUser", "{main}");

    // What does main call?
    let callees = ok_json(
        &client,
        "find_callees",
        json!({"repo_id": repo, "symbol_name": "main"}),
    )
    .await;
    let callee_names = names(&callees, "callees");
    assert!(
        callee_names.contains(&"getUser".to_string())
            && callee_names.contains(&"formatName".to_string()),
        "{callees}"
    );

    // How does main reach findById?
    let flow = ok_text(
        &client,
        "trace_flow",
        json!({"repo_id": repo, "from_symbol": "main", "to_symbol": harness::fixture::FIND_BY_ID}),
    )
    .await;
    assert!(flow.starts_with("Index: "), "{flow}");
    assert!(flow.contains(": 2 calls."), "{flow}");
    assert!(
        flow.contains("-> UserService.getUser (Method) src/services/user.ts:"),
        "{flow}"
    );
    let unreachable = ok_text(
        &client,
        "trace_flow",
        json!({"repo_id": repo, "from_symbol": harness::fixture::FIND_BY_ID, "to_symbol": "main"}),
    )
    .await;
    assert!(unreachable.contains("No call chain"), "{unreachable}");

    // What breaks if findById changes?
    let impact = ok_text(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "symbol_name": harness::fixture::FIND_BY_ID}),
    )
    .await;
    let items = bullet_items(&impact);
    assert!(
        items
            .iter()
            .any(|i| i.starts_with("Direct dependents") && i.contains("UserService.getUser")),
        "{impact}"
    );
    assert!(
        items
            .iter()
            .any(|i| i.starts_with("Files with dependents") && i.contains("src/index.ts")),
        "{impact}"
    );
    let by_file = ok_text(
        &client,
        "impact_of_change",
        json!({"repo_id": repo, "file_path": "src/db.ts"}),
    )
    .await;
    assert!(
        by_file.contains("Impact of changing src/db.ts"),
        "{by_file}"
    );
    assert!(by_file.contains("UserService.getUser"), "{by_file}");

    // Search: this index has no embeddings, so auto falls back to keyword.
    let found = ok_text(
        &client,
        "search_code",
        json!({"repo_id": repo, "query": "formatName"}),
    )
    .await;
    assert!(found.contains("Keyword search"), "{found}");
    assert!(found.contains("formatName"), "{found}");
    let keyword = ok_text(
        &client,
        "search_code",
        json!({"repo_id": repo, "query": "formatName", "mode": "keyword"}),
    )
    .await;
    assert!(
        !keyword.contains("Keyword search (") && keyword.contains("formatName"),
        "{keyword}"
    );

    // Unused code under a directory.
    let dead = ok_json(
        &client,
        "find_dead_code",
        json!({"repo_id": repo, "path": "src/services"}),
    )
    .await;
    let dead_files: Vec<&str> = dead["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["file_path"].as_str().unwrap())
        .collect();
    assert!(!dead_files.is_empty(), "{dead}");
    assert!(
        dead_files.iter().all(|f| f.starts_with("src/services/")),
        "{dead}"
    );
}

#[tokio::test]
async fn deprecated_aliases_answer_like_their_replacements() {
    let client = connect().await;
    let repo = harness::REPO_ID;
    let strip_index = |t: String| t.split_once("\n\n").map(|(_, b)| b.to_string()).unwrap();
    let strip_frames = |t: String| {
        t.lines()
            .filter(|l| !l.starts_with("<<<UNTRUSTED ") && !l.starts_with(">>>END "))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let old = ok_text(
        &client,
        "symbol_context",
        json!({"repo_id": repo, "symbol_name": harness::fixture::GET_USER}),
    )
    .await;
    let new = ok_text(
        &client,
        "explain_symbol",
        json!({"repo_id": repo, "symbol_name": harness::fixture::GET_USER}),
    )
    .await;
    assert_eq!(
        strip_frames(strip_index(old)),
        strip_frames(strip_index(new))
    );

    let old = ok_text(
        &client,
        "search_documents",
        json!({"repo_id": repo, "query": "user"}),
    )
    .await;
    let new = ok_text(
        &client,
        "search_code",
        json!({"repo_id": repo, "query": "user", "mode": "keyword"}),
    )
    .await;
    assert_eq!(
        strip_frames(strip_index(old)),
        strip_frames(strip_index(new))
    );

    let mut old = ok_json(
        &client,
        "get_callers",
        json!({"repo_id": repo, "symbol_name": harness::fixture::FIND_BY_ID, "max_depth": 2}),
    )
    .await;
    let mut new = ok_json(
        &client,
        "find_callers",
        json!({"repo_id": repo, "symbol_name": harness::fixture::FIND_BY_ID, "depth": 2}),
    )
    .await;
    old["index_status"] = Value::Null;
    new["index_status"] = Value::Null;
    assert_eq!(old, new);

    // The three graph-diff tools are one now.
    for tool in [
        "get_graph_diff",
        "snapshot_diff",
        "detect_architecture_drift",
    ] {
        let result = call(&client, tool, json!({"repo_id": repo})).await.unwrap();
        assert!(
            text_of(&result).contains("nothing to compare"),
            "{tool}: {}",
            text_of(&result)
        );
    }
}
