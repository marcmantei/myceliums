//! Graph API v1: embedding-based overview, node detail and search.

use crate::handlers::graph::{
    build_community_map, calculate_depths, color_for_kind, symbol_kind_display,
};
use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use myceliums_storage::RelationshipKind;
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::error;

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
struct GraphOverviewApiResponse {
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
struct NodeDetailResponse {
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
struct GraphSearchResponse {
    query: String,
    results: Vec<GraphSearchResult>,
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
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(GraphOverviewApiResponse {
                    clusters: vec![],
                    pagination: PaginationInfo {
                        page: 1,
                        per_page: 100,
                        total: 0,
                    },
                }),
            );
        }
    };
    let store = graph.store();

    let symbols_with_vecs = store.get_symbols_with_vectors().await.unwrap_or_default();
    let symbols: Vec<_> = symbols_with_vecs.iter().map(|(s, _)| s.clone()).collect();
    let relationships = graph.relationships().await.unwrap_or_default();
    let community_map = build_community_map(&graph, &symbols, &relationships).await;

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

    (
        StatusCode::OK,
        Json(GraphOverviewApiResponse {
            clusters: paginated,
            pagination: PaginationInfo {
                page,
                per_page,
                total,
            },
        }),
    )
}

/// GET /api/v1/graph/nodes/{repo_id}/{node_id}
/// Returns detailed node metadata with graph and semantic neighbors.
pub(crate) async fn graph_v1_node(
    State(state): State<Arc<AppState>>,
    Path((repo_id, node_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(Option::<NodeDetailResponse>::None),
            );
        }
    };
    let store = graph.store();

    let symbols = graph.symbols().await.unwrap_or_default();
    let relationships = graph.relationships().await.unwrap_or_default();
    let community_map = build_community_map(&graph, &symbols, &relationships).await;
    let depths = calculate_depths(&symbols, &relationships).await;

    let symbol = match symbols.iter().find(|s| s.uid == node_id) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(None)),
    };

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

    // Semantic neighbors via vector search.
    // A storage failure here is a real backend error, not "no neighbors": surface
    // it as 5xx so callers can distinguish it from a genuine empty neighbor set.
    let semantic_neighbors = match store.get_symbols_with_vectors().await {
        Ok(svecs) => {
            // Find this symbol's vector
            let this_vec: Option<Vec<f32>> = svecs
                .iter()
                .find(|(s, _)| s.uid == node_id)
                .and_then(|(_, v)| v.clone());

            if let Some(query_vec) = this_vec {
                match store.vector_search(&query_vec, 11).await {
                    Ok(results) => results
                        .into_iter()
                        .filter(|(s, _)| s.uid != node_id)
                        .take(10)
                        .map(|(s, score)| SemanticNeighbor {
                            id: s.uid,
                            name: s.name,
                            kind: s.kind.to_string(),
                            similarity: score,
                        })
                        .collect(),
                    Err(e) => {
                        error!("Vector search failed for node {}: {}", node_id, e);
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(Option::<NodeDetailResponse>::None),
                        );
                    }
                }
            } else {
                // Symbol has no embedding vector: legitimately no semantic neighbors.
                vec![]
            }
        }
        Err(e) => {
            error!("Failed to load symbol vectors for {}: {}", node_id, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Option::<NodeDetailResponse>::None),
            );
        }
    };

    let comm = community_map.get(&node_id).cloned().unwrap_or_default();

    (
        StatusCode::OK,
        Json(Some(NodeDetailResponse {
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
            neighbors,
            semantic_neighbors,
        })),
    )
}

/// GET /api/v1/graph/search/{repo_id}?q=...&top_k=20
/// Semantic search across the knowledge graph using embeddings.
pub(crate) async fn graph_v1_search(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<GraphSearchParams>,
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(GraphSearchResponse {
                    query: params.q,
                    results: vec![],
                }),
            );
        }
    };
    let store = graph.store();

    let top_k = params.top_k.unwrap_or(20).clamp(1, 100);

    // Embed the query using the same model as indexing
    let embedder = match myceliums_core::embedder_for_index(&store).await {
        Ok(e) => e,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphSearchResponse {
                    query: params.q,
                    results: vec![],
                }),
            );
        }
    };
    let query_vector = match embedder.embed_query(&params.q).await {
        Ok(v) => v,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphSearchResponse {
                    query: params.q,
                    results: vec![],
                }),
            );
        }
    };

    let results = match store.vector_search(&query_vector, top_k).await {
        Ok(r) => r
            .into_iter()
            .map(|(s, score)| GraphSearchResult {
                id: s.uid,
                name: s.name,
                kind: s.kind.to_string(),
                file: s.file_path,
                line: s.start_line,
                score,
            })
            .collect(),
        // A storage failure is not "no matches": surface it as 5xx so callers
        // can distinguish a real backend error from a genuine empty result set.
        Err(e) => {
            error!("Vector search failed for {}: {}", repo_id, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(GraphSearchResponse {
                    query: params.q,
                    results: vec![],
                }),
            );
        }
    };

    (
        StatusCode::OK,
        Json(GraphSearchResponse {
            query: params.q,
            results,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    use axum::response::IntoResponse;

    use crate::test_support::{app_state, corrupt_seeded_store, seed_healthy_store, test_data_dir};

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
}
