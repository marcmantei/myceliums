//! `POST /api/repos/{repo_id}/knowledge`: knowledge-base search.
//!
//! The query is matched against symbol names by keyword search, with test
//! code ranked below product code unless `?tests=include|prefer` (#150);
//! the answer lists what mentions the best `limit` matches.

use crate::error::{ApiError, Repo};
use crate::handlers::{unclassified_notice, TestsParam};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use myceliums_core::mentions::MentionMetadata;
use myceliums_core::search::search_symbols_ranked;
use myceliums_storage::RelationshipKind;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub(crate) struct KnowledgeRequest {
    query: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
pub(crate) struct KnowledgeResponse {
    query: String,
    /// How test code was ranked: `demote`, `include` or `prefer`.
    tests: &'static str,
    /// Set when the index cannot tell test code from product code.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    total_mentions: usize,
    source_count: usize,
    results: Vec<KnowledgeResult>,
}

#[derive(Serialize)]
struct KnowledgeResult {
    source_name: String,
    source_kind: String,
    source_file: String,
    mentioned_symbol: String,
    mentioned_kind: String,
    mentioned_file: String,
    mentioned_line: u32,
    match_context: String,
    match_line: u32,
}

pub(crate) async fn query_knowledge(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(tests): Query<TestsParam>,
    Json(body): Json<KnowledgeRequest>,
) -> Result<Json<KnowledgeResponse>, ApiError> {
    let tests = tests.policy()?;
    let repo = Repo::new(&state, &repo_id);
    let graph = state
        .graph
        .graph(&repo_id)
        .await
        .map_err(|e| repo.error(e))?;

    let symbols = graph.symbols().await.map_err(|e| repo.error(e))?;
    let relationships = graph.relationships().await.map_err(|e| repo.error(e))?;

    let limit = body.limit.unwrap_or(20);
    let search_results = search_symbols_ranked(&symbols, &body.query, false, &tests);

    // Collect UIDs of matching symbols (up to limit)
    let matched_uids: std::collections::HashSet<String> = search_results
        .iter()
        .take(limit)
        .map(|r| r.symbol.uid.clone())
        .collect();

    // Build symbol lookup map
    let sym_map: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    // Find Mentions relationships where the target is a matched symbol
    let mut results = Vec::new();
    let mut source_ids = std::collections::HashSet::new();

    for rel in &relationships {
        if rel.kind != RelationshipKind::Mentions {
            continue;
        }
        if !matched_uids.contains(&rel.target_uid) {
            continue;
        }

        let source = match sym_map.get(rel.source_uid.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let target = match sym_map.get(rel.target_uid.as_str()) {
            Some(s) => s,
            None => continue,
        };

        source_ids.insert(rel.source_uid.clone());

        // Parse metadata for match locations. Best-effort by design: malformed
        // metadata is a data-quality issue on a single relationship, not a backend
        // failure. The row is still surfaced below (in the `else` branch) without
        // match details, so nothing is silently dropped.
        let metadata: Option<MentionMetadata> = serde_json::from_str(&rel.metadata).ok();

        if let Some(meta) = metadata {
            for m in &meta.matches {
                results.push(KnowledgeResult {
                    source_name: source.name.clone(),
                    source_kind: source.kind.to_string(),
                    source_file: source.file_path.clone(),
                    mentioned_symbol: target.name.clone(),
                    mentioned_kind: target.kind.to_string(),
                    mentioned_file: target.file_path.clone(),
                    mentioned_line: target.start_line,
                    match_context: m.context.clone(),
                    match_line: m.line,
                });
            }
        } else {
            // No parseable metadata; still include a result without match details
            results.push(KnowledgeResult {
                source_name: source.name.clone(),
                source_kind: source.kind.to_string(),
                source_file: source.file_path.clone(),
                mentioned_symbol: target.name.clone(),
                mentioned_kind: target.kind.to_string(),
                mentioned_file: target.file_path.clone(),
                mentioned_line: target.start_line,
                match_context: String::new(),
                match_line: 0,
            });
        }
    }

    let total_mentions = results.len();
    let source_count = source_ids.len();

    Ok(Json(KnowledgeResponse {
        query: body.query,
        tests: tests.ranking.as_str(),
        notice: unclassified_notice(graph.info(), &tests),
        total_mentions,
        source_count,
        results,
    }))
}

#[cfg(test)]
mod tests {
    use crate::test_support::{
        app_state, error_body, register_repo_with, request, sample_symbol, seed_store,
        test_data_dir, test_symbol,
    };
    use axum::http::StatusCode;
    use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
    use serde_json::{json, Value};

    fn named(symbol: CodeSymbol, name: &str, content: &str) -> CodeSymbol {
        CodeSymbol {
            name: name.to_string(),
            qualified_name: name.to_string(),
            content: content.to_string(),
            ..symbol
        }
    }

    fn mention(uid: &str, target: &str, repo: &str) -> Relationship {
        Relationship {
            uid: uid.to_string(),
            source_uid: "readme".to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Mentions,
            repo_id: repo.to_string(),
            metadata: String::new(),
        }
    }

    /// The symbols the answer's mentions point at.
    fn mentioned(body: &Value) -> Vec<String> {
        body["results"]
            .as_array()
            .unwrap_or_else(|| panic!("no results: {body}"))
            .iter()
            .map(|r| r["mentioned_symbol"].as_str().unwrap().to_string())
            .collect()
    }

    /// The keyword search behind the knowledge endpoint ranks test code
    /// below product code by default (#150): with `limit: 1` the product
    /// function is the match whose mentions are listed, although the test
    /// repeats the query terms more often and ranks first with
    /// `tests=include`.
    #[tokio::test]
    async fn keyword_matches_demote_test_code_unless_included() {
        let dir = test_data_dir("knowledge_demotion");
        let _ = std::fs::remove_dir_all(&dir);
        let repo = "repo-knowledge";
        seed_store(
            &dir,
            repo,
            &[
                (sample_symbol("readme", repo), None),
                (
                    named(
                        sample_symbol("product", repo),
                        "parse_cookie_header",
                        "split",
                    ),
                    None,
                ),
                (
                    named(
                        test_symbol("tested", repo),
                        "test_parse_cookie_header",
                        "parse cookie header parse cookie header parse cookie header",
                    ),
                    None,
                ),
            ],
            &[
                mention("m1", "product", repo),
                mention("m2", "tested", repo),
            ],
        )
        .await;
        let state = app_state(dir.clone());
        let uri = format!("/api/repos/{repo}/knowledge");
        let body = || Some(json!({"query": "parse cookie header", "limit": 1}));

        let (status, _, json) = request(state.clone(), "POST", &uri, body()).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["tests"], "demote");
        assert!(json.get("notice").is_none(), "{json}");
        assert_eq!(mentioned(&json), ["parse_cookie_header"]);

        let (status, _, json) = request(
            state.clone(),
            "POST",
            &format!("{uri}?tests=include"),
            body(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(json["tests"], "include");
        assert_eq!(mentioned(&json), ["test_parse_cookie_header"]);

        let (status, _, json) = request(
            state.clone(),
            "POST",
            &format!("{uri}?tests=prefer"),
            body(),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{json}");
        assert_eq!(mentioned(&json), ["test_parse_cookie_header"]);

        let (status, _, json) =
            request(state.clone(), "POST", &format!("{uri}?tests=never"), body()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
        error_body(&json);

        // An index from before the classification cannot demote: it says so.
        register_repo_with(&dir, repo, 0);
        let (status, _, json) = request(state, "POST", &uri, body()).await;
        assert_eq!(status, StatusCode::OK, "{json}");
        let notice = json["notice"].as_str().unwrap_or_else(|| panic!("{json}"));
        assert!(
            notice.contains("before test code was classified"),
            "{notice}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
