//! Graph analytics: centrality, community metrics, module coupling and cycles.

use crate::state::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tracing::error;

#[derive(Serialize)]
struct CentralityResponse {
    nodes: Vec<CentralityNodeResponse>,
    total_nodes: usize,
    metric: String,
}

#[derive(Serialize)]
struct CentralityNodeResponse {
    uid: String,
    name: String,
    kind: String,
    file: String,
    degree: f64,
    betweenness: f64,
    closeness: f64,
    eigenvector: f64,
}

pub(crate) async fn get_centrality(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(CentralityResponse {
                    nodes: vec![],
                    total_nodes: 0,
                    metric: "betweenness".to_string(),
                }),
            );
        }
    };

    let symbols = graph.symbols().await.unwrap_or_default();
    let relationships = graph.relationships().await.unwrap_or_default();

    let centrality = match myceliums_core::compute_centrality(&relationships) {
        Ok(c) => c,
        Err(e) => {
            error!("Centrality computation failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CentralityResponse {
                    nodes: vec![],
                    total_nodes: 0,
                    metric: "betweenness".to_string(),
                }),
            );
        }
    };

    let metric = params
        .get("metric")
        .map(|s| s.as_str())
        .unwrap_or("betweenness");
    let top_n: usize = params
        .get("top_n")
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    let uid_map: HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut nodes: Vec<CentralityNodeResponse> = centrality
        .values()
        .filter_map(|c| {
            uid_map
                .get(c.uid.as_str())
                .map(|sym| CentralityNodeResponse {
                    uid: c.uid.clone(),
                    name: sym.name.clone(),
                    kind: sym.kind.to_string(),
                    file: sym.file_path.clone(),
                    degree: c.degree,
                    betweenness: c.betweenness,
                    closeness: c.closeness,
                    eigenvector: c.eigenvector,
                })
        })
        .collect();

    match metric {
        "degree" => nodes.sort_by(|a, b| {
            b.degree
                .partial_cmp(&a.degree)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "closeness" => nodes.sort_by(|a, b| {
            b.closeness
                .partial_cmp(&a.closeness)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "eigenvector" => nodes.sort_by(|a, b| {
            b.eigenvector
                .partial_cmp(&a.eigenvector)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        _ => nodes.sort_by(|a, b| {
            b.betweenness
                .partial_cmp(&a.betweenness)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }

    let total_nodes = nodes.len();
    nodes.truncate(top_n);

    (
        StatusCode::OK,
        Json(CentralityResponse {
            nodes,
            total_nodes,
            metric: metric.to_string(),
        }),
    )
}

#[derive(Serialize)]
struct CommunityMetricsResponse {
    modularity: f64,
    community_count: usize,
    cohesion: Vec<CohesionEntryResponse>,
    coupling: Vec<CouplingEntryResponse>,
}

#[derive(Serialize)]
struct CohesionEntryResponse {
    community: String,
    density: f64,
}

#[derive(Serialize)]
struct CouplingEntryResponse {
    community_a: String,
    community_b: String,
    edge_count: u32,
}

pub(crate) async fn get_community_metrics(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(CommunityMetricsResponse {
                    modularity: 0.0,
                    community_count: 0,
                    cohesion: vec![],
                    coupling: vec![],
                }),
            );
        }
    };

    let symbols = graph.symbols().await.unwrap_or_default();
    let relationships = graph.relationships().await.unwrap_or_default();

    let metrics = match myceliums_core::compute_community_metrics(&symbols, &relationships) {
        Ok(m) => m,
        Err(e) => {
            error!("Community metrics failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CommunityMetricsResponse {
                    modularity: 0.0,
                    community_count: 0,
                    cohesion: vec![],
                    coupling: vec![],
                }),
            );
        }
    };

    let mut cohesion: Vec<CohesionEntryResponse> = metrics
        .cohesion
        .into_iter()
        .map(|(community, density)| CohesionEntryResponse { community, density })
        .collect();
    cohesion.sort_by(|a, b| {
        b.density
            .partial_cmp(&a.density)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let coupling: Vec<CouplingEntryResponse> = metrics
        .coupling
        .into_iter()
        .map(|c| CouplingEntryResponse {
            community_a: c.community_a,
            community_b: c.community_b,
            edge_count: c.edge_count,
        })
        .collect();

    (
        StatusCode::OK,
        Json(CommunityMetricsResponse {
            modularity: metrics.modularity,
            community_count: metrics.community_count,
            cohesion,
            coupling,
        }),
    )
}

// --- Module Coupling Dashboard ---

#[derive(Serialize)]
struct ModuleCouplingResponse {
    modules: Vec<ModuleCouplingEntry>,
    total_modules: usize,
}

#[derive(Serialize)]
struct ModuleCouplingEntry {
    module_path: String,
    afferent: u32,
    efferent: u32,
    instability: f64,
}

#[derive(Deserialize)]
pub(crate) struct ModuleCouplingParams {
    group_by_dir: Option<bool>,
}

pub(crate) async fn get_module_coupling(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<ModuleCouplingParams>,
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(ModuleCouplingResponse {
                    modules: vec![],
                    total_modules: 0,
                }),
            );
        }
    };

    let symbols = graph.symbols().await.unwrap_or_default();
    let relationships = graph.relationships().await.unwrap_or_default();
    let group_by_dir = params.group_by_dir.unwrap_or(true);

    let coupling_data =
        myceliums_core::compute_module_coupling(&symbols, &relationships, group_by_dir);

    let total_modules = coupling_data.len();
    let modules: Vec<ModuleCouplingEntry> = coupling_data
        .into_iter()
        .map(|m| ModuleCouplingEntry {
            module_path: m.module_path,
            afferent: m.afferent,
            efferent: m.efferent,
            instability: m.instability,
        })
        .collect();

    (
        StatusCode::OK,
        Json(ModuleCouplingResponse {
            modules,
            total_modules,
        }),
    )
}

#[derive(Serialize)]
struct CyclesResponse {
    cycles: Vec<CycleEntryResponse>,
    total_count: usize,
}

#[derive(Serialize)]
struct CycleEntryResponse {
    members: Vec<String>,
    size: usize,
    files: Vec<String>,
}

pub(crate) async fn get_cycles(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let graph = match state.graph.graph(&repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(CyclesResponse {
                    cycles: vec![],
                    total_count: 0,
                }),
            );
        }
    };

    let symbols = graph.symbols().await.unwrap_or_default();
    let relationships = graph.relationships().await.unwrap_or_default();

    let cycles = match myceliums_core::detect_cycles(&symbols, &relationships, true, true, 2) {
        Ok(c) => c,
        Err(e) => {
            error!("Cycle detection failed: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CyclesResponse {
                    cycles: vec![],
                    total_count: 0,
                }),
            );
        }
    };

    let total_count = cycles.len();
    let cycle_entries: Vec<CycleEntryResponse> = cycles
        .into_iter()
        .map(|c| CycleEntryResponse {
            members: c.member_names,
            size: c.size,
            files: c.files,
        })
        .collect();

    (
        StatusCode::OK,
        Json(CyclesResponse {
            cycles: cycle_entries,
            total_count,
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_centrality_response_structure() {
        let resp = CentralityResponse {
            nodes: vec![CentralityNodeResponse {
                uid: "uid_hub".to_string(),
                name: "hub".to_string(),
                kind: "Function".to_string(),
                file: "src/lib.rs".to_string(),
                degree: 0.8,
                betweenness: 0.5,
                closeness: 0.7,
                eigenvector: 0.3,
            }],
            total_nodes: 50,
            metric: "betweenness".to_string(),
        };

        assert_eq!(resp.nodes.len(), 1);
        assert_eq!(resp.total_nodes, 50);
        assert_eq!(resp.metric, "betweenness");
        assert_eq!(resp.nodes[0].name, "hub");
        assert!((resp.nodes[0].betweenness - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_community_metrics_response_structure() {
        let resp = CommunityMetricsResponse {
            modularity: 0.42,
            community_count: 5,
            cohesion: vec![CohesionEntryResponse {
                community: "core".to_string(),
                density: 0.75,
            }],
            coupling: vec![CouplingEntryResponse {
                community_a: "core".to_string(),
                community_b: "utils".to_string(),
                edge_count: 12,
            }],
        };

        assert!((resp.modularity - 0.42).abs() < f64::EPSILON);
        assert_eq!(resp.community_count, 5);
        assert_eq!(resp.cohesion.len(), 1);
        assert_eq!(resp.cohesion[0].community, "core");
        assert_eq!(resp.coupling[0].edge_count, 12);
    }

    #[test]
    fn test_cycles_response_structure() {
        let resp = CyclesResponse {
            cycles: vec![CycleEntryResponse {
                members: vec!["alpha".to_string(), "beta".to_string()],
                size: 2,
                files: vec!["src/a.rs".to_string()],
            }],
            total_count: 1,
        };

        assert_eq!(resp.total_count, 1);
        assert_eq!(resp.cycles[0].size, 2);
        assert_eq!(resp.cycles[0].members.len(), 2);
        assert_eq!(resp.cycles[0].files.len(), 1);
    }
}
