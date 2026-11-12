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
    CypherQueryParams, DeleteParams, DetectImpactParams, LinkDecisionParams, MapServiceParams,
    MyceliumsMcp, RecordDecisionParams, RenameParams, SearchParams, SymbolContextParams,
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
            query: harness::fixture::GET_USER.to_string(),
            repo_id: Some(harness::REPO_ID.to_string()),
            limit: Some(10),
            explain: None,
        })
        .await
        .expect("context_search should succeed");
    assert!(
        out.contains(harness::fixture::GET_USER),
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
        out.contains(harness::fixture::USER_SERVICE_FILE),
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
            symbol_name: harness::fixture::GET_USER.to_string(),
            new_name: harness::fixture::FETCH_USER.to_string(),
            repo_id: harness::REPO_ID.to_string(),
        })
        .await
        .expect("rename of a known symbol should succeed");
    assert!(
        out.contains(harness::fixture::FETCH_USER) || out.contains(harness::fixture::GET_USER),
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
        out.contains(harness::fixture::USER_SERVICE) && out.contains(harness::fixture::DATABASE),
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

// ── repo_id validation / list_repos (#89) ────────────────────────────

#[tokio::test]
async fn list_repos_lists_registered_repo() {
    harness::ensure_indexed().await;
    let out = server()
        .list_repos_for_test()
        .await
        .expect("list_repos should succeed");
    assert!(
        out.contains(&format!("id: {}", harness::REPO_ID)),
        "list_repos should show the registered id, got: {out}"
    );
}

#[tokio::test]
async fn unknown_repo_id_is_an_error_that_lists_known_ids() {
    let data = harness::ensure_indexed().await;
    let err = server()
        .cypher_query_for_test(CypherQueryParams {
            query: "MATCH (s:CodeSymbol) RETURN s.name".to_string(),
            repo_id: "no-such-repo".to_string(),
        })
        .await
        .expect_err("an unknown repo_id must not silently return []");
    assert!(
        err.contains("unknown repo_id") && err.contains(harness::REPO_ID),
        "error should name the known ids, got: {err}"
    );
    assert!(
        !data.join("data").join("no-such-repo").exists(),
        "a read path must not create a directory for an unknown id"
    );
}

#[tokio::test]
async fn path_traversal_repo_id_is_rejected_on_every_entry_path() {
    let data = harness::ensure_indexed().await;
    let srv = server();
    let bad = "../escape";

    // Direct store open (`registered_db_path`).
    let err = srv
        .cypher_query_for_test(CypherQueryParams {
            query: "MATCH (s:CodeSymbol) RETURN s.name".to_string(),
            repo_id: bad.to_string(),
        })
        .await
        .expect_err("traversal id must be rejected");
    assert!(err.contains("invalid repo_id"), "got: {err}");

    // Optional repo_id resolved by the handler.
    let err = srv
        .context_search_for_test(SearchParams {
            query: "x".to_string(),
            repo_id: Some(bad.to_string()),
            limit: None,
            explain: None,
        })
        .await
        .expect_err("traversal id must be rejected");
    assert!(err.contains("invalid repo_id"), "got: {err}");

    // GraphService-backed handler.
    let err = srv
        .symbol_context_for_test(SymbolContextParams {
            symbol_name: harness::fixture::GET_USER.to_string(),
            repo_id: bad.to_string(),
        })
        .await
        .expect_err("traversal id must be rejected");
    assert!(err.contains("invalid repo_id"), "got: {err}");

    assert!(!data.join("escape").exists());
}

#[tokio::test]
async fn write_tools_reject_traversal_and_unknown_ids_without_writing() {
    let data = harness::ensure_indexed().await;
    let srv = server();
    for bad in ["../escape", "..", "no-such-repo"] {
        let errs = [
            srv.delete_for_test(DeleteParams {
                repo_id: bad.to_string(),
            })
            .await
            .expect_err("delete"),
            srv.map_service_for_test(MapServiceParams {
                repo_id: bad.to_string(),
                community_label: "c".to_string(),
                service_name: "s".to_string(),
            })
            .await
            .expect_err("map_service"),
            srv.record_decision_for_test(RecordDecisionParams {
                repo_id: bad.to_string(),
                title: "t".to_string(),
                context: "c".to_string(),
                decision: "d".to_string(),
                ..Default::default()
            })
            .await
            .expect_err("record_decision"),
            srv.link_decision_for_test(LinkDecisionParams {
                repo_id: bad.to_string(),
                decision_id: "x".to_string(),
                symbol_name: harness::fixture::GET_USER.to_string(),
            })
            .await
            .expect_err("link_decision"),
        ];
        for err in errs {
            assert!(
                err.contains("invalid repo_id") || err.contains("unknown repo_id"),
                "{bad}: {err}"
            );
        }
    }
    // Nothing was written next to or above the data dir.
    for dir in ["services", "decisions"] {
        assert!(!data.join(dir).exists(), "{dir} must not be created");
    }
    assert!(!data.parent().unwrap().join("escape.json").exists());
    // The real repo survived the `delete` attempts.
    assert!(server()
        .list_repos_for_test()
        .await
        .unwrap()
        .contains(harness::REPO_ID));
}

#[tokio::test]
async fn resources_reject_traversal_and_unknown_repo_ids() {
    harness::ensure_indexed().await;
    let srv = server();
    for uri in [
        "repository://../x/schema",
        "repository://../x",
        "repository://..",
        "repository://no-such-repo/map",
    ] {
        let err = srv.read_resource_for_test(uri).await.expect_err(uri);
        assert!(
            err.contains("invalid repo_id") || err.contains("unknown repo_id"),
            "{uri}: {err}"
        );
    }
    let ok = srv
        .read_resource_for_test(&format!("repository://{}/schema", harness::REPO_ID))
        .await
        .expect("registered repo resource");
    assert!(ok.contains("symbol_count"), "{ok}");
}
