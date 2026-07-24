//! Integration tests for the MCP tool handlers, exercised against a real
//! on-disk index built from the `sample-ts-project` fixture.
//!
//! Handlers are reached through the `test-support` wrappers (thin delegates
//! over the private `#[tool]` methods) — no stdio transport required.
//!
//! Scope note: `semantic_search` and `hybrid_search` unconditionally load a
//! fastembed embedding model (downloading it on first use). Exercising them
//! would violate the "no model downloads in CI" rule and be non-deterministic,
//! so they are intentionally not covered here. The graph-backed tools below
//! need no model and are fully offline and deterministic.

mod harness;

use myceliums_mcp::{
    CypherQueryParams, DetectImpactParams, MyceliumsMcp, RenameParams, SearchParams,
};

fn server() -> MyceliumsMcp {
    MyceliumsMcp::new()
}

// ── context_search ───────────────────────────────────────────────────

#[tokio::test]
async fn context_search_finds_indexed_symbol() {
    harness::ensure_indexed().await;
    let out = server()
        .context_search_for_test(SearchParams {
            query: "getUser".to_string(),
            repo_id: Some(harness::REPO_ID.to_string()),
            limit: Some(10),
            explain: None,
        })
        .await
        .expect("context_search should succeed");
    assert!(
        out.contains("getUser"),
        "expected getUser in results, got: {out}"
    );
}

#[tokio::test]
async fn context_search_unknown_symbol_reports_no_results() {
    harness::ensure_indexed().await;
    let out = server()
        .context_search_for_test(SearchParams {
            query: "definitelyNotASymbolXYZ".to_string(),
            repo_id: Some(harness::REPO_ID.to_string()),
            limit: Some(10),
            explain: None,
        })
        .await
        .expect("query for a missing symbol still succeeds");
    assert!(
        out.contains("No results"),
        "missing symbol should report no results, got: {out}"
    );
}

// ── detect_impact ────────────────────────────────────────────────────

#[tokio::test]
async fn detect_impact_traces_changed_file() {
    harness::ensure_indexed().await;
    // A minimal unified diff touching the user service in the fixture.
    let diff = "\
diff --git a/src/services/user.ts b/src/services/user.ts
index 0000000..1111111 100644
--- a/src/services/user.ts
+++ b/src/services/user.ts
@@ -18,3 +18,4 @@ export class UserService {
     getUser(id: string): User | null {
+        // touched
         return this.db.findById('users', id);
     }
";
    let out = server()
        .detect_impact_for_test(DetectImpactParams {
            repo_id: Some(harness::REPO_ID.to_string()),
            diff: Some(diff.to_string()),
            depth: Some(2),
        })
        .await
        .expect("detect_impact should succeed");
    // The changed file must be surfaced in the impact report.
    assert!(
        out.contains("src/services/user.ts"),
        "impact report should mention the changed file, got: {out}"
    );
}

#[tokio::test]
async fn detect_impact_empty_diff_reports_no_changes() {
    harness::ensure_indexed().await;
    let out = server()
        .detect_impact_for_test(DetectImpactParams {
            repo_id: Some(harness::REPO_ID.to_string()),
            diff: Some("   \n  ".to_string()),
            depth: None,
        })
        .await
        .expect("empty diff is a success with a no-op message");
    assert!(
        out.to_lowercase().contains("no changes"),
        "empty diff should report no changes, got: {out}"
    );
}

// ── rename_symbol ────────────────────────────────────────────────────

#[tokio::test]
async fn rename_symbol_produces_plan_for_known_symbol() {
    harness::ensure_indexed().await;
    let out = server()
        .rename_symbol_for_test(RenameParams {
            symbol_name: "getUser".to_string(),
            new_name: "fetchUser".to_string(),
            repo_id: harness::REPO_ID.to_string(),
        })
        .await
        .expect("rename of a known symbol should succeed");
    assert!(
        out.contains("fetchUser") || out.contains("getUser"),
        "rename plan should reference the symbols, got: {out}"
    );
}

#[tokio::test]
async fn rename_symbol_unknown_symbol_errors() {
    harness::ensure_indexed().await;
    let err = server()
        .rename_symbol_for_test(RenameParams {
            symbol_name: "doesNotExist".to_string(),
            new_name: "whatever".to_string(),
            repo_id: harness::REPO_ID.to_string(),
        })
        .await
        .expect_err("renaming an unknown symbol must error");
    assert!(
        err.contains("not found"),
        "expected 'not found' error, got: {err}"
    );
}

// ── cypher_query (handler surface) ───────────────────────────────────

#[tokio::test]
async fn cypher_query_handler_returns_rows() {
    harness::ensure_indexed().await;
    let out = server()
        .cypher_query_for_test(CypherQueryParams {
            query: "MATCH (s:CodeSymbol) WHERE s.kind = 'Class' RETURN s.name".to_string(),
            repo_id: harness::REPO_ID.to_string(),
        })
        .await
        .expect("cypher_query should succeed");
    assert!(
        out.contains("UserService") && out.contains("Database"),
        "class query should list both classes, got: {out}"
    );
}

#[tokio::test]
async fn cypher_query_handler_blocks_mutations() {
    harness::ensure_indexed().await;
    let err = server()
        .cypher_query_for_test(CypherQueryParams {
            query: "CREATE (n:CodeSymbol {name: 'evil'})".to_string(),
            repo_id: harness::REPO_ID.to_string(),
        })
        .await
        .expect_err("write clauses must be rejected");
    assert!(!err.is_empty(), "mutation rejection should carry a message");
}
