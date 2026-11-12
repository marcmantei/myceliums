//! The MCP read tools serve the graph from `GraphService`'s per-repo cache,
//! and `analyze` / `delete` invalidate that cache (#93).
//!
//! This binary owns its data directory and its project copy: it re-analyzes
//! and deletes, so it must not share the read-only index of `harness`
//! (see the isolation note there). `MYCELIUMS_DATA_DIR` is process-wide, so
//! the whole scenario runs as one sequential test.

use myceliums_mcp::{
    AnalyzeParams, CypherQueryParams, DeleteParams, GetCallersParams, MyceliumsMcp, SearchParams,
};
use std::path::Path;

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

fn analyze_params(project: &Path) -> AnalyzeParams {
    AnalyzeParams {
        path: project.to_string_lossy().into_owned(),
        force: Some(true),
        skip_embeddings: Some(true),
        ..Default::default()
    }
}

fn search(repo_id: &str, query: &str) -> SearchParams {
    SearchParams {
        query: query.to_string(),
        repo_id: Some(repo_id.to_string()),
        ..Default::default()
    }
}

#[tokio::test]
async fn reads_are_cached_and_analyze_or_delete_invalidates_the_cache() {
    let data = tempfile::tempdir().unwrap();
    std::env::set_var("MYCELIUMS_DATA_DIR", data.path());

    let projects = tempfile::tempdir().unwrap();
    let project = projects.path().join("cache-project");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sample-ts-project");
    copy_dir(&fixture, &project);
    let project = project.canonicalize().unwrap();
    let repo_id = myceliums_core::analyzer::repo_id_from_path(&project);

    let server = MyceliumsMcp::new();
    server
        .analyze_for_test(analyze_params(&project))
        .await
        .expect("first analyze");

    // ── Repeated reads do not reload the graph tables ────────────────
    let callers = || GetCallersParams {
        symbol_name: "capitalize".to_string(),
        repo_id: repo_id.clone(),
        ..Default::default()
    };
    server
        .context_search_for_test(search(&repo_id, "formatName"))
        .await
        .unwrap();
    server.get_callers_for_test(callers()).await.unwrap();
    let loads_after_warmup = server.graph_table_loads_for_test();
    assert!(loads_after_warmup > 0, "the first reads load from disk");
    assert!(server.graph_is_cached_for_test(&repo_id));

    for _ in 0..10 {
        server
            .context_search_for_test(search(&repo_id, "formatName"))
            .await
            .unwrap();
        server.get_callers_for_test(callers()).await.unwrap();
    }
    assert_eq!(
        server.graph_table_loads_for_test(),
        loads_after_warmup,
        "repeated reads must be served from the cache"
    );

    let before = server
        .context_search_for_test(search(&repo_id, "zebraCacheProbe"))
        .await
        .unwrap();
    assert!(before.starts_with("No results"), "{before}");

    // ── analyze invalidates ─────────────────────────────────────────
    let utils = project.join("src/utils.ts");
    let mut source = std::fs::read_to_string(&utils).unwrap();
    source.push_str("\nexport function zebraCacheProbe(): number {\n    return 42;\n}\n");
    std::fs::write(&utils, source).unwrap();

    server
        .analyze_for_test(analyze_params(&project))
        .await
        .expect("re-analyze");
    assert!(
        !server.graph_is_cached_for_test(&repo_id),
        "analyze must drop the cached graph"
    );
    let after = server
        .context_search_for_test(search(&repo_id, "zebraCacheProbe"))
        .await
        .unwrap();
    assert!(
        !after.starts_with("No results") && after.contains("src/utils.ts"),
        "{after}"
    );
    let cypher = server
        .cypher_query_for_test(CypherQueryParams {
            query: "MATCH (s:CodeSymbol) WHERE s.name = 'zebraCacheProbe' RETURN s.name"
                .to_string(),
            repo_id: repo_id.clone(),
        })
        .await
        .unwrap();
    assert!(cypher.contains("zebraCacheProbe"), "{cypher}");

    // ── delete invalidates ──────────────────────────────────────────
    server
        .delete_for_test(DeleteParams {
            repo_id: repo_id.clone(),
        })
        .await
        .unwrap();
    assert!(!server.graph_is_cached_for_test(&repo_id));
    let gone = server
        .context_search_for_test(search(&repo_id, "zebraCacheProbe"))
        .await
        .expect_err("a deleted repo is no longer readable");
    assert!(gone.contains(&repo_id), "{gone}");
}
