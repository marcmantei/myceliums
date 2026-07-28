//! Result-correctness tests for the Cypher executor against a real
//! `myceliums_storage::Store`.
//!
//! A small, fully known graph is written into a tempdir store, then queries are
//! run through `CypherExecutor::from_store` and asserted at the row level —
//! covering MATCH, WHERE, ORDER BY, shortest-path, the result-row cap, and the
//! blocked-mutation guard (#20 regression). Fully offline and deterministic.

use myceliums_cypher::CypherExecutor;
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, Store, SymbolKind};
use serde_json::Value;

/// Build a symbol with the fields the executor reads.
fn symbol(uid: &str, name: &str, kind: SymbolKind, file: &str, line: u32) -> CodeSymbol {
    CodeSymbol {
        uid: uid.to_string(),
        name: name.to_string(),
        qualified_name: format!("{file}::{name}"),
        kind,
        file_path: file.to_string(),
        start_line: line,
        end_line: line + 5,
        signature: format!("fn {name}()"),
        content: String::new(),
        repo_id: "cypher-test".to_string(),
        metadata: None,
    }
}

/// Build a CALLS edge between two symbol uids.
fn calls(uid: &str, from: &str, to: &str) -> Relationship {
    Relationship {
        uid: uid.to_string(),
        source_uid: from.to_string(),
        target_uid: to.to_string(),
        kind: RelationshipKind::Calls,
        repo_id: "cypher-test".to_string(),
        metadata: "{}".to_string(),
    }
}

/// A known graph:  main → helper → leaf ;  main → sibling (a Class).
async fn executor_over_known_graph() -> (CypherExecutor, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("temp dir");
    let store = Store::open(tmp.path(), "cypher-test")
        .await
        .expect("open store");

    let symbols = vec![
        symbol("u_main", "main", SymbolKind::Function, "src/app.ts", 1),
        symbol("u_helper", "helper", SymbolKind::Function, "src/app.ts", 20),
        symbol("u_leaf", "leaf", SymbolKind::Function, "src/util.ts", 3),
        symbol("u_widget", "Widget", SymbolKind::Class, "src/widget.ts", 1),
    ];
    let rels = vec![
        calls("r1", "u_main", "u_helper"),
        calls("r2", "u_helper", "u_leaf"),
        calls("r3", "u_main", "u_widget"),
    ];
    store.store_symbols(&symbols).await.expect("store symbols");
    store
        .store_relationships(&rels)
        .await
        .expect("store relationships");

    let executor = CypherExecutor::from_store(&store)
        .await
        .expect("build executor");
    (executor, tmp)
}

fn names(rows: &[std::collections::HashMap<String, Value>], key: &str) -> Vec<String> {
    rows.iter()
        .filter_map(|r| r.get(key).and_then(|v| v.as_str()).map(str::to_string))
        .collect()
}

#[tokio::test]
async fn match_all_symbols_returns_every_node() {
    let (exec, _tmp) = executor_over_known_graph().await;
    let rows = exec
        .execute("MATCH (s:CodeSymbol) RETURN s.name")
        .expect("query ok");
    let mut got = names(&rows, "s.name");
    got.sort();
    assert_eq!(got, vec!["Widget", "helper", "leaf", "main"]);
}

#[tokio::test]
async fn where_filters_by_kind() {
    let (exec, _tmp) = executor_over_known_graph().await;
    let rows = exec
        .execute("MATCH (s:CodeSymbol) WHERE s.kind = 'Function' RETURN s.name")
        .expect("query ok");
    let mut got = names(&rows, "s.name");
    got.sort();
    assert_eq!(got, vec!["helper", "leaf", "main"]);
}

#[tokio::test]
async fn order_by_sorts_rows() {
    let (exec, _tmp) = executor_over_known_graph().await;
    let rows = exec
        .execute("MATCH (s:CodeSymbol) WHERE s.kind = 'Function' RETURN s.name ORDER BY s.name ASC")
        .expect("query ok");
    // ORDER BY must preserve order, so assert the exact sequence (not sorted).
    assert_eq!(names(&rows, "s.name"), vec!["helper", "leaf", "main"]);
}

#[tokio::test]
async fn match_relationship_returns_exact_pairs() {
    let (exec, _tmp) = executor_over_known_graph().await;
    let rows = exec
        .execute("MATCH (a:CodeSymbol)-[:CALLS]->(b:CodeSymbol) RETURN a.name, b.name")
        .expect("query ok");
    let mut pairs: Vec<(String, String)> = rows
        .iter()
        .map(|r| {
            (
                r.get("a.name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
                r.get("b.name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            )
        })
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            ("helper".to_string(), "leaf".to_string()),
            ("main".to_string(), "Widget".to_string()),
            ("main".to_string(), "helper".to_string()),
        ]
    );
}

#[tokio::test]
async fn limit_caps_row_count() {
    let (exec, _tmp) = executor_over_known_graph().await;
    let rows = exec
        .execute("MATCH (s:CodeSymbol) RETURN s.name LIMIT 2")
        .expect("query ok");
    assert_eq!(rows.len(), 2, "LIMIT 2 must return exactly two rows");
}

#[tokio::test]
async fn shortest_path_connects_endpoints() {
    let (exec, _tmp) = executor_over_known_graph().await;
    // main → helper → leaf is the only path to leaf. Bind both endpoints with
    // node patterns, then compute the shortest path between them.
    let rows = exec
        .execute(
            "MATCH (a:CodeSymbol), (b:CodeSymbol), path = shortestPath((a)-[*..5]->(b)) \
             WHERE a.name = 'main' AND b.name = 'leaf' RETURN path",
        )
        .expect("shortest-path query ok");
    assert_eq!(rows.len(), 1, "exactly one shortest path from main to leaf");
    // The rendered path value should reference both endpoints.
    let path_str = serde_json::to_string(&rows[0]).unwrap();
    assert!(
        path_str.contains("main") && path_str.contains("leaf"),
        "path should traverse main..leaf, got: {path_str}"
    );
}

#[tokio::test]
async fn mutations_are_blocked() {
    let (exec, _tmp) = executor_over_known_graph().await;
    // #20 regression: write clauses must be rejected, leaving the graph intact.
    for query in [
        "CREATE (n:CodeSymbol {name: 'evil'})",
        "MATCH (n:CodeSymbol) DELETE n",
        "MATCH (n:CodeSymbol) SET n.name = 'x'",
        "MERGE (n:CodeSymbol {name: 'evil'})",
    ] {
        let result = exec.execute(query);
        assert!(
            result.is_err(),
            "mutation `{query}` must be rejected, but it succeeded"
        );
    }

    // The graph is unchanged after the blocked mutations.
    let rows = exec
        .execute("MATCH (s:CodeSymbol) RETURN s.name")
        .expect("read-back ok");
    assert_eq!(
        rows.len(),
        4,
        "graph must still hold its 4 original symbols"
    );
}

#[tokio::test]
async fn unknown_label_is_rejected() {
    let (exec, _tmp) = executor_over_known_graph().await;
    let result = exec.execute("MATCH (s:Bogus) RETURN s.name");
    assert!(result.is_err(), "unknown label must be rejected");
}
