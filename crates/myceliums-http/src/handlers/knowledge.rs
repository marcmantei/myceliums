//! `POST /api/repos/{repo_id}/knowledge`: knowledge-base search.

use crate::state::{open_store, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use myceliums_core::mentions::MentionMetadata;
use myceliums_core::search::search_symbols;
use myceliums_storage::RelationshipKind;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::error;

#[derive(Deserialize)]
pub(crate) struct KnowledgeRequest {
    query: String,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct KnowledgeResponse {
    query: String,
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

// ── Phase 2: Graph analytics endpoints ───────────────────────────────

pub(crate) async fn query_knowledge(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Json(body): Json<KnowledgeRequest>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(KnowledgeResponse {
                    query: body.query,
                    total_mentions: 0,
                    source_count: 0,
                    results: vec![],
                }),
            );
        }
    };

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();

    let limit = body.limit.unwrap_or(20);
    let search_results = search_symbols(&symbols, &body.query);

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

    (
        StatusCode::OK,
        Json(KnowledgeResponse {
            query: body.query,
            total_mentions,
            source_count,
            results,
        }),
    )
}
