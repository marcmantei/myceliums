//! Graph API v1: embedding-based overview, node detail and search.
//!
//! The semantic search and the semantic neighbours of a node rank test code
//! below product code unless `?tests=include|prefer` (#150), like
//! `search_code` and `myc semantic-search`.

use crate::error::{ApiError, Repo};
use crate::handlers::graph::{
    build_community_map, calculate_depths, color_for_kind, symbol_kind_display,
};
use crate::handlers::{test_policy, unclassified_notice, TestsParam};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use myceliums_core::TestPolicy;
use myceliums_storage::{CodeSymbol, RelationshipKind, Store};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Deserialize)]
pub(crate) struct GraphOverviewParams {
    page: Option<u32>,
    per_page: Option<u32>,
}

/// A cluster representing a community of related symbols.
/// Positioned in 2D space based on average embedding similarity.
#[derive(Serialize, Clone)]
struct ClusterResponse {
    id: String,
    label: String,
    size: u32,
    x: f32,
    y: f32,
    color: String,
    top_symbols: Vec<ClusterSymbolSummary>,
    edges: Vec<ClusterEdge>,
}

#[derive(Serialize, Clone)]
struct ClusterSymbolSummary {
    id: String,
    name: String,
    kind: String,
}

#[derive(Serialize, Clone)]
struct ClusterEdge {
    target: String,
    weight: f32,
}

#[derive(Serialize)]
pub(crate) struct GraphOverviewApiResponse {
    clusters: Vec<ClusterResponse>,
    pagination: PaginationInfo,
}

#[derive(Serialize)]
struct PaginationInfo {
    page: u32,
    per_page: u32,
    total: u32,
}

/// Detailed node with neighbors and semantic similarity info.
#[derive(Serialize)]
pub(crate) struct NodeDetailResponse {
    id: String,
    name: String,
    kind: String,
    symbol_kind_display: String,
    file: String,
    line: u32,
    end_line: u32,
    signature: String,
    community: String,
    color: String,
    depth: u32,
    is_entry_point: bool,
    /// How test code was ranked among `semantic_neighbors`.
    tests: &'static str,
    neighbors: Vec<NodeNeighbor>,
    semantic_neighbors: Vec<SemanticNeighbor>,
}

#[derive(Serialize)]
struct NodeNeighbor {
    id: String,
    name: String,
    kind: String,
    relationship: String,
    direction: String,
}

#[derive(Serialize)]
struct SemanticNeighbor {
    id: String,
    name: String,
    kind: String,
    similarity: f32,
}

#[derive(Deserialize)]
pub(crate) struct GraphSearchParams {
    q: String,
    top_k: Option<usize>,
    /// `demote` (default), `include` or `prefer` (#150). Not a flattened
    /// [`TestsParam`]: `serde(flatten)` breaks the numeric `top_k`.
    tests: Option<String>,
}

#[derive(Serialize)]
struct GraphSearchResult {
    id: String,
    name: String,
    kind: String,
    file: String,
    line: u32,
    score: f32,
}

#[derive(Serialize)]
pub(crate) struct GraphSearchResponse {
    query: String,
    /// How test code was ranked: `demote`, `include` or `prefer`.
    tests: &'static str,
    /// Set when the index cannot tell test code from product code.
    #[serde(skip_serializing_if = "Option::is_none")]
    notice: Option<String>,
    results: Vec<GraphSearchResult>,
}

/// The `limit` stored vectors nearest to `query_vector`, test code placed by
/// `tests` (#150) — the ranking of semantic `search_code` — without the
/// symbol `exclude`. Read through [`myceliums_core::RepoGraph::store`]: call
/// `verify()` after it.
async fn nearest(
    store: &Store,
    query_vector: &[f32],
    limit: usize,
    tests: &TestPolicy,
    exclude: Option<&str>,
) -> myceliums_core::error::Result<Vec<(CodeSymbol, f32)>> {
    let fetch = limit + usize::from(exclude.is_some());
    let hits = myceliums_core::semantic_search(store, query_vector, fetch, tests).await?;
    Ok(hits
        .into_iter()
        .filter(|(s, _)| Some(s.uid.as_str()) != exclude)
        .take(limit)
        .map(|(s, score)| (s, score as f32))
        .collect())
}

/// Whether the index holds embeddings, as `search_code` decides it: without
/// any, no model is loaded (or downloaded) for an index that could not use
/// it. A failed read is an error, not "no embeddings".
async fn has_vectors(store: &Store) -> myceliums_core::error::Result<bool> {
    if store
        .get_index_meta(myceliums_core::IndexEmbeddingMeta::META_KEY)
        .await?
        .is_none()
    {
        return Ok(false);
    }
    Ok(match myceliums_core::EmbeddingStats::load(store).await? {
        Some(stats) => stats.symbols_embedded > 0,
        None => true,
    })
}

/// Project 384-dim embeddings to 2D using PCA (first two principal components).
/// Returns a map of uid -> (x, y).
fn project_embeddings_to_2d(vectors: &[(String, Vec<f32>)]) -> HashMap<String, (f32, f32)> {
    if vectors.is_empty() {
        return HashMap::new();
    }

    let dim = vectors[0].1.len();
    let n = vectors.len();

    // Compute centroid
    let mut centroid = vec![0.0f64; dim];
    for (_, v) in vectors {
        for (j, &val) in v.iter().enumerate() {
            centroid[j] += val as f64;
        }
    }
    for c in &mut centroid {
        *c /= n as f64;
    }

    // Center the data
    let centered: Vec<Vec<f64>> = vectors
        .iter()
        .map(|(_, v)| {
            v.iter()
                .enumerate()
                .map(|(j, &val)| val as f64 - centroid[j])
                .collect()
        })
        .collect();

    // Power iteration to find first two principal components
    let pc1 = power_iteration(&centered, dim, None);
    let pc2 = power_iteration(&centered, dim, Some(&pc1));

    // Project each vector onto PC1 and PC2
    let mut positions = HashMap::new();
    for (i, (uid, _)) in vectors.iter().enumerate() {
        let x: f64 = centered[i].iter().zip(pc1.iter()).map(|(a, b)| a * b).sum();
        let y: f64 = centered[i].iter().zip(pc2.iter()).map(|(a, b)| a * b).sum();
        positions.insert(uid.clone(), (x as f32, y as f32));
    }

    // Normalize to [-1, 1] range
    let (mut min_x, mut max_x) = (f32::MAX, f32::MIN);
    let (mut min_y, mut max_y) = (f32::MAX, f32::MIN);
    for &(x, y) in positions.values() {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let range_x = (max_x - min_x).max(1e-6);
    let range_y = (max_y - min_y).max(1e-6);

    for (x, y) in positions.values_mut() {
        *x = (*x - min_x) / range_x * 2.0 - 1.0;
        *y = (*y - min_y) / range_y * 2.0 - 1.0;
    }

    positions
}

/// Power iteration to find a principal component vector.
/// If `deflate_against` is provided, deflects the matrix to find the next PC.
fn power_iteration(data: &[Vec<f64>], dim: usize, deflate_against: Option<&[f64]>) -> Vec<f64> {
    let mut v = vec![0.0f64; dim];
    // Initialize with a deterministic vector
    for (i, val) in v.iter_mut().enumerate() {
        *val = ((i * 7 + 3) % 100) as f64 / 100.0;
    }
    let norm: f64 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
    for val in &mut v {
        *val /= norm;
    }

    for _ in 0..50 {
        // Multiply: v_new = X^T * X * v  (computed as X^T * (X * v) for efficiency)
        let projections: Vec<f64> = data
            .iter()
            .map(|row| row.iter().zip(v.iter()).map(|(a, b)| a * b).sum())
            .collect();

        let mut v_new = vec![0.0f64; dim];
        for (i, &proj) in projections.iter().enumerate() {
            for (j, val) in v_new.iter_mut().enumerate() {
                *val += data[i][j] * proj;
            }
        }

        // Deflate against previous PC if needed
        if let Some(pc) = deflate_against {
            let dot: f64 = v_new.iter().zip(pc.iter()).map(|(a, b)| a * b).sum();
            for (j, val) in v_new.iter_mut().enumerate() {
                *val -= dot * pc[j];
            }
        }

        // Normalize
        let norm: f64 = v_new.iter().map(|x| x * x).sum::<f64>().sqrt();
        if norm < 1e-10 {
            break;
        }
        for val in &mut v_new {
            *val /= norm;
        }
        v = v_new;
    }

    v
}

/// Compute cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a < 1e-8 || norm_b < 1e-8 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

/// GET /api/v1/graph/overview/{repo_id}
/// Returns community clusters positioned by embedding similarity.
pub(crate) async fn graph_v1_overview(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<GraphOverviewParams>,
) -> Result<Json<GraphOverviewApiResponse>, ApiError> {
    let repo = Repo::new(&state, &repo_id);
    let graph = state
        .graph
        .graph(&repo_id)
        .await
        .map_err(|e| repo.error(e))?;
    let store = graph.store();

    // Tables first, then the store read, then `verify()`: everything below
    // comes from one index generation.
    let relationships = graph.relationships().await.map_err(|e| repo.error(e))?;
    graph.communities().await.map_err(|e| repo.error(e))?;
    let symbols_with_vecs = store
        .get_symbols_with_vectors()
        .await
        .map_err(|e| repo.error(e))?;
    graph.verify().map_err(|e| repo.error(e))?;
    let symbols: Vec<_> = symbols_with_vecs.iter().map(|(s, _)| s.clone()).collect();
    let community_map = build_community_map(&graph, &symbols, &relationships)
        .await
        .map_err(|e| repo.error(e))?;

    // Group symbols by community
    type SymbolWithVec = (myceliums_storage::CodeSymbol, Option<Vec<f32>>);
    let mut community_symbols: HashMap<String, Vec<SymbolWithVec>> = HashMap::new();
    for (sym, vec) in &symbols_with_vecs {
        let comm = community_map
            .get(&sym.uid)
            .cloned()
            .unwrap_or_else(|| "uncategorized".to_string());
        community_symbols
            .entry(comm)
            .or_default()
            .push((sym.clone(), vec.clone()));
    }

    // Compute average embedding per community for positioning
    let mut community_avg_vectors: Vec<(String, Vec<f32>)> = Vec::new();
    for (comm_label, members) in &community_symbols {
        let embedded: Vec<&Vec<f32>> = members.iter().filter_map(|(_, v)| v.as_ref()).collect();
        if !embedded.is_empty() {
            let dim = embedded[0].len();
            let mut avg = vec![0.0f32; dim];
            for v in &embedded {
                for (j, &val) in v.iter().enumerate() {
                    avg[j] += val;
                }
            }
            let count = embedded.len() as f32;
            for val in &mut avg {
                *val /= count;
            }
            community_avg_vectors.push((comm_label.clone(), avg));
        } else {
            // No embeddings — use zero vector (will get random-ish position)
            community_avg_vectors.push((comm_label.clone(), vec![0.0; 384]));
        }
    }

    // Project community centroids to 2D
    let positions_2d = project_embeddings_to_2d(&community_avg_vectors);

    // Build inter-community edge weights from actual relationships
    let mut inter_community_weights: HashMap<(String, String), f32> = HashMap::new();
    for rel in &relationships {
        let src_comm = community_map
            .get(&rel.source_uid)
            .cloned()
            .unwrap_or_default();
        let tgt_comm = community_map
            .get(&rel.target_uid)
            .cloned()
            .unwrap_or_default();
        if !src_comm.is_empty() && !tgt_comm.is_empty() && src_comm != tgt_comm {
            let key = if src_comm < tgt_comm {
                (src_comm, tgt_comm)
            } else {
                (tgt_comm, src_comm)
            };
            *inter_community_weights.entry(key).or_insert(0.0) += 1.0;
        }
    }

    // Compute cosine similarities between community centroids for edge weights
    let comm_vec_map: HashMap<&str, &[f32]> = community_avg_vectors
        .iter()
        .map(|(label, v)| (label.as_str(), v.as_slice()))
        .collect();

    // Build cluster responses
    let mut clusters: Vec<ClusterResponse> = Vec::new();
    let mut sorted_communities: Vec<_> = community_symbols.iter().collect();
    sorted_communities.sort_by_key(|(_, members)| Reverse(members.len()));

    for (comm_label, members) in &sorted_communities {
        let (x, y) = positions_2d
            .get(comm_label.as_str())
            .copied()
            .unwrap_or((0.0, 0.0));

        // Top 5 symbols by name for preview
        let top_symbols: Vec<ClusterSymbolSummary> = members
            .iter()
            .take(5)
            .map(|(s, _)| ClusterSymbolSummary {
                id: s.uid.clone(),
                name: s.name.clone(),
                kind: s.kind.to_string(),
            })
            .collect();

        // Edges to other communities (with cosine similarity)
        let mut edges: Vec<ClusterEdge> = Vec::new();
        let this_vec = comm_vec_map.get(comm_label.as_str());
        for (other_label, _) in &sorted_communities {
            if other_label == comm_label {
                continue;
            }
            if let (Some(v1), Some(v2)) = (this_vec, comm_vec_map.get(other_label.as_str())) {
                let sim = cosine_similarity(v1, v2);
                if sim > 0.1 {
                    edges.push(ClusterEdge {
                        target: other_label.to_string(),
                        weight: sim,
                    });
                }
            }
        }
        // Sort edges by weight descending, keep top 10
        edges.sort_by(|a, b| {
            b.weight
                .partial_cmp(&a.weight)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        edges.truncate(10);

        // Dominant kind for color
        let mut kind_counts: HashMap<String, usize> = HashMap::new();
        for (s, _) in members.iter() {
            *kind_counts.entry(s.kind.to_string()).or_insert(0) += 1;
        }
        let dominant_kind = kind_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(k, _)| k.as_str())
            .unwrap_or("Unknown");

        clusters.push(ClusterResponse {
            id: comm_label.to_string(),
            label: comm_label.to_string(),
            size: members.len() as u32,
            x,
            y,
            color: color_for_kind(dominant_kind),
            top_symbols,
            edges,
        });
    }

    // Paginate
    let page = params.page.unwrap_or(1).max(1);
    let per_page = params.per_page.unwrap_or(100).clamp(1, 500);
    let total = clusters.len() as u32;
    let start = ((page - 1) * per_page) as usize;
    let end = (start + per_page as usize).min(clusters.len());
    let paginated = if start < clusters.len() {
        clusters[start..end].to_vec()
    } else {
        vec![]
    };

    Ok(Json(GraphOverviewApiResponse {
        clusters: paginated,
        pagination: PaginationInfo {
            page,
            per_page,
            total,
        },
    }))
}

/// GET /api/v1/graph/nodes/{repo_id}/{node_id}?tests=demote
/// Returns detailed node metadata with graph and semantic neighbors.
pub(crate) async fn graph_v1_node(
    State(state): State<Arc<AppState>>,
    Path((repo_id, node_id)): Path<(String, String)>,
    Query(tests): Query<TestsParam>,
) -> Result<Json<NodeDetailResponse>, ApiError> {
    let tests = tests.policy()?;
    let repo = Repo::new(&state, &repo_id);
    let graph = state
        .graph
        .graph(&repo_id)
        .await
        .map_err(|e| repo.error(e))?;
    let store = graph.store();

    let symbols = graph.symbols().await.map_err(|e| repo.error(e))?;
    let relationships = graph.relationships().await.map_err(|e| repo.error(e))?;
    let community_map = build_community_map(&graph, &symbols, &relationships)
        .await
        .map_err(|e| repo.error(e))?;
    let depths = calculate_depths(&symbols, &relationships).await;

    let symbol = symbols
        .iter()
        .find(|s| s.uid == node_id)
        .ok_or_else(|| repo.symbol_not_found(&node_id))?;

    // Entry point detection
    let has_caller: HashSet<&str> = relationships
        .iter()
        .filter(|r| matches!(r.kind, RelationshipKind::Calls))
        .map(|r| r.target_uid.as_str())
        .collect();
    let is_entry_point = !has_caller.contains(node_id.as_str());

    // Graph neighbors
    let sym_map: HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut neighbors = Vec::new();
    for rel in &relationships {
        if rel.target_uid == node_id {
            if let Some(src) = sym_map.get(rel.source_uid.as_str()) {
                neighbors.push(NodeNeighbor {
                    id: src.uid.clone(),
                    name: src.name.clone(),
                    kind: src.kind.to_string(),
                    relationship: rel.kind.to_string(),
                    direction: "incoming".to_string(),
                });
            }
        }
        if rel.source_uid == node_id {
            if let Some(tgt) = sym_map.get(rel.target_uid.as_str()) {
                neighbors.push(NodeNeighbor {
                    id: tgt.uid.clone(),
                    name: tgt.name.clone(),
                    kind: tgt.kind.to_string(),
                    relationship: rel.kind.to_string(),
                    direction: "outgoing".to_string(),
                });
            }
        }
    }

    // Semantic neighbors via vector search, test code placed by `tests`.
    // A storage failure here is a real backend error, not "no neighbors": surface
    // it as 5xx so callers can distinguish it from a genuine empty neighbor set.
    let svecs = store
        .get_symbols_with_vectors()
        .await
        .map_err(|e| repo.error(e))?;
    let this_vec: Option<Vec<f32>> = svecs
        .into_iter()
        .find(|(s, _)| s.uid == node_id)
        .and_then(|(_, v)| v);
    let semantic_neighbors = match this_vec {
        Some(query_vec) => nearest(&store, &query_vec, 10, &tests, Some(&node_id))
            .await
            .map_err(|e| ApiError::internal("vector search failed", e, &repo))?
            .into_iter()
            .map(|(s, score)| SemanticNeighbor {
                id: s.uid,
                name: s.name,
                kind: s.kind.to_string(),
                similarity: score,
            })
            .collect(),
        // Symbol has no embedding vector: legitimately no semantic neighbors.
        None => vec![],
    };
    // Every store read before `verify()`: one after it is not covered.
    graph.verify().map_err(|e| repo.error(e))?;

    let comm = community_map.get(&node_id).cloned().unwrap_or_default();

    Ok(Json(NodeDetailResponse {
        id: symbol.uid.clone(),
        name: symbol.name.clone(),
        kind: symbol.kind.to_string(),
        symbol_kind_display: symbol_kind_display(&symbol.kind.to_string()),
        file: symbol.file_path.clone(),
        line: symbol.start_line,
        end_line: symbol.end_line,
        signature: symbol.signature.clone(),
        community: comm,
        color: color_for_kind(&symbol.kind.to_string()),
        depth: depths.get(&node_id).copied().unwrap_or(0),
        is_entry_point,
        tests: tests.ranking.as_str(),
        neighbors,
        semantic_neighbors,
    }))
}

/// GET /api/v1/graph/search/{repo_id}?q=...&top_k=20&tests=demote
/// Semantic search across the knowledge graph using embeddings; test code
/// ranks below product code unless `tests=include|prefer` (#150).
pub(crate) async fn graph_v1_search(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<GraphSearchParams>,
) -> Result<Json<GraphSearchResponse>, ApiError> {
    let tests = test_policy(params.tests.as_deref())?;
    let repo = Repo::new(&state, &repo_id);
    let graph = state
        .graph
        .graph(&repo_id)
        .await
        .map_err(|e| repo.error(e))?;
    let store = graph.store();

    let top_k = params.top_k.unwrap_or(20).clamp(1, 100);

    if !has_vectors(&store).await.map_err(|e| repo.error(e))? {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            format!(
                "the index of {repo_id} has no embeddings (built with --skip-embeddings, \
                 or before embedding metadata was recorded), so it cannot be searched by meaning"
            ),
            format!(
                "re-run `myc analyze {}` without --skip-embeddings, or use POST \
                 /api/repos/{repo_id}/knowledge (keyword search)",
                graph.info().path
            ),
        ));
    }
    // Embed the query using the same model as indexing
    let embedder = myceliums_core::embedder_for_index(&store)
        .await
        .map_err(|e| {
            ApiError::internal("cannot load the embedding model of this index", e, &repo)
        })?;
    let query_vector = embedder
        .embed_query(&params.q)
        .await
        .map_err(|e| ApiError::internal("cannot embed the query", e, &repo))?;

    // A storage failure is not "no matches": surface it as 5xx so callers
    // can distinguish a real backend error from a genuine empty result set.
    let results = nearest(&store, &query_vector, top_k, &tests, None)
        .await
        .map_err(|e| ApiError::internal("vector search failed", e, &repo))?
        .into_iter()
        .map(|(s, score)| GraphSearchResult {
            id: s.uid,
            name: s.name,
            kind: s.kind.to_string(),
            file: s.file_path,
            line: s.start_line,
            score,
        })
        .collect();
    // Every store read before `verify()`: one after it is not covered.
    graph.verify().map_err(|e| repo.error(e))?;

    Ok(Json(GraphSearchResponse {
        query: params.q,
        tests: tests.ranking.as_str(),
        notice: unclassified_notice(graph.info(), &tests),
        results,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::response::IntoResponse;

    use crate::test_support::{
        app_state, corrupt_seeded_store, error_body, request, sample_symbol, seed_healthy_store,
        seed_store, test_data_dir, test_symbol, tilted_vector,
    };

    // --- Regression tests for #30: search/neighbor endpoints must not collapse
    // backend errors into an empty result set. A storage failure returns 5xx;
    // HTTP 200 + `[]` is reserved for a genuine no-match. ---

    #[tokio::test]
    async fn node_detail_returns_ok_for_genuine_no_semantic_match() {
        let dir = test_data_dir("node_no_match");
        let _ = std::fs::remove_dir_all(&dir);
        let repo_id = "repo_ok";
        let node_id = "only_node";
        seed_healthy_store(&dir, repo_id, node_id).await;

        let resp = graph_v1_node(
            State(app_state(dir.clone())),
            Path((repo_id.to_string(), node_id.to_string())),
            Query(TestsParam::default()),
        )
        .await
        .into_response();

        // The single symbol is filtered out of its own neighbor list, so this is a
        // genuine empty semantic-neighbor set — it must be 200, not an error.
        assert_eq!(resp.status(), StatusCode::OK);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn node_detail_does_not_return_silent_ok_when_store_errors() {
        let dir = test_data_dir("node_store_err");
        let _ = std::fs::remove_dir_all(&dir);
        let repo_id = "repo_broken";
        let node_id = "broken_node";
        corrupt_seeded_store(&dir, repo_id, node_id).await;

        let resp = graph_v1_node(
            State(app_state(dir.clone())),
            Path((repo_id.to_string(), node_id.to_string())),
            Query(TestsParam::default()),
        )
        .await
        .into_response();

        // A backend failure must never be served as a healthy 200 + empty
        // neighbors — the endpoint must surface an error status instead. (The
        // shared `symbols` table means `get_symbols` fails first, so this
        // corrupt-store case exercises the endpoint's failure handling as a
        // whole rather than the semantic-neighbor branch in isolation.)
        assert_ne!(
            resp.status(),
            StatusCode::OK,
            "corrupt store must not yield a silent 200"
        );
        assert!(
            resp.status().is_client_error() || resp.status().is_server_error(),
            "expected an error status on store failure, got {}",
            resp.status()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn graph_search_returns_5xx_when_backend_errors() {
        let dir = test_data_dir("search_store_err");
        let _ = std::fs::remove_dir_all(&dir);
        let repo_id = "repo_search_broken";
        let node_id = "search_node";
        corrupt_seeded_store(&dir, repo_id, node_id).await;

        let resp = graph_v1_search(
            State(app_state(dir.clone())),
            Path(repo_id.to_string()),
            Query(GraphSearchParams {
                q: "anything".to_string(),
                top_k: Some(20),
                tests: None,
            }),
        )
        .await
        .into_response();

        // The search endpoint must not answer a backend failure with 200 + `[]`.
        assert!(
            resp.status().is_server_error(),
            "expected 5xx on store error, got {}",
            resp.status()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The semantic neighbours of a node rank test code below product code
    /// by default, and like product code with `tests=include` (#150): the
    /// two test symbols are nearer than the product one.
    #[tokio::test]
    async fn semantic_neighbors_demote_test_code_unless_included() {
        let dir = test_data_dir("node_demotion");
        let _ = std::fs::remove_dir_all(&dir);
        let repo = "repo-demotion";
        seed_store(
            &dir,
            repo,
            &[
                (sample_symbol("anchor", repo), Some(tilted_vector(0.0))),
                (test_symbol("test_near", repo), Some(tilted_vector(0.1))),
                (test_symbol("test_mid", repo), Some(tilted_vector(0.2))),
                (sample_symbol("product_far", repo), Some(tilted_vector(0.4))),
            ],
            &[],
        )
        .await;
        let state = app_state(dir.clone());
        let neighbors = |body: &serde_json::Value| -> Vec<String> {
            body["semantic_neighbors"]
                .as_array()
                .unwrap_or_else(|| panic!("no semantic_neighbors: {body}"))
                .iter()
                .map(|n| n["id"].as_str().unwrap().to_string())
                .collect()
        };

        let uri = format!("/api/v1/graph/nodes/{repo}/anchor");
        let (status, _, body) = request(state.clone(), "GET", &uri, None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["tests"], "demote");
        assert_eq!(neighbors(&body), ["product_far", "test_near", "test_mid"]);

        let (status, _, body) =
            request(state.clone(), "GET", &format!("{uri}?tests=include"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["tests"], "include");
        assert_eq!(neighbors(&body), ["test_near", "test_mid", "product_far"]);

        let (status, _, body) =
            request(state.clone(), "GET", &format!("{uri}?tests=prefer"), None).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(neighbors(&body), ["test_near", "test_mid", "product_far"]);

        let (status, _, body) = request(state, "GET", &format!("{uri}?tests=bogus"), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let (error, next) = error_body(&body);
        assert!(error.contains("bogus"), "{error}");
        assert!(next.contains("tests=include"), "{next}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Semantic search ranks through the same demoting helper as the
    /// neighbours; the endpoint itself needs the index's embedding model,
    /// so this checks the helper on the stored vectors.
    #[tokio::test]
    async fn semantic_search_ranking_demotes_test_code() {
        let dir = test_data_dir("search_demotion");
        let _ = std::fs::remove_dir_all(&dir);
        let repo = "repo-search-demotion";
        seed_store(
            &dir,
            repo,
            &[
                (test_symbol("test_near", repo), Some(tilted_vector(0.05))),
                (sample_symbol("product_far", repo), Some(tilted_vector(0.4))),
            ],
            &[],
        )
        .await;
        let state = app_state(dir.clone());
        let graph = state.graph.graph(repo).await.unwrap();
        let ids = |hits: Vec<(CodeSymbol, f32)>| -> Vec<String> {
            hits.into_iter().map(|(s, _)| s.uid).collect()
        };
        let query = tilted_vector(0.0);
        let store = graph.store();
        let demoted = nearest(&store, &query, 2, &TestPolicy::default(), None)
            .await
            .unwrap();
        assert_eq!(ids(demoted), ["product_far", "test_near"]);
        let included = nearest(&store, &query, 2, &TestPolicy::include(), None)
            .await
            .unwrap();
        assert_eq!(ids(included), ["test_near", "product_far"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An index without embeddings cannot be searched by meaning: `409`
    /// with the way out, before any model is loaded; a bad `tests` value is
    /// a `400` before anything is read.
    #[tokio::test]
    async fn semantic_search_without_embeddings_is_409_and_bad_tests_is_400() {
        let dir = test_data_dir("search_no_vectors");
        let _ = std::fs::remove_dir_all(&dir);
        let repo = "repo-no-vectors";
        seed_store(&dir, repo, &[(sample_symbol("only", repo), None)], &[]).await;
        let state = app_state(dir.clone());

        let (status, _, body) = request(
            state.clone(),
            "GET",
            &format!("/api/v1/graph/search/{repo}?q=anything&top_k=5"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::CONFLICT, "{body}");
        let (error, next) = error_body(&body);
        assert!(error.contains("no embeddings"), "{error}");
        assert!(
            next.contains("myc analyze /nonexistent/repo-no-vectors"),
            "{next}"
        );
        assert!(next.contains("/knowledge"), "{next}");

        let (status, _, body) = request(
            state,
            "GET",
            &format!("/api/v1/graph/search/{repo}?q=anything&tests=sometimes"),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
        error_body(&body);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
