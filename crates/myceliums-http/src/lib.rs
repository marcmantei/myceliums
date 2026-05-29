use anyhow::Result;
use axum::{
    extract::{Path, Query, State},
    http::{header, Request, StatusCode},
    middleware::Next,
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use myceliums_core::mentions::MentionMetadata;
use myceliums_core::search::search_symbols;
use myceliums_storage::{RelationshipKind, RepoRegistry, Store};
use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tower_http::cors::{Any, CorsLayer};
use tracing::{error, info};

const VISUALIZATION_HTML: &str = include_str!("visualization.html");

struct AppState {
    data_dir: PathBuf,
    registry_path: PathBuf,
    /// Pre-selected repo_id (if the user passed --repo)
    default_repo_id: Option<String>,
}

/// Phase 1 extended GraphResponse for 2D schematic view.
/// Includes enriched nodes, edges, and community/process clustering information.
#[derive(Serialize)]
struct GraphResponse {
    nodes: Vec<GraphNode>,
    edges: Vec<GraphEdge>,
    communities: Vec<GraphCommunity>,
    processes: Vec<ProcessCluster>,
}

/// Phase 1: Extended GraphNode with enriched metadata for 2D visualization.
/// - depth: hierarchical depth from entry points (for vertical layout)
/// - symbol_kind_display: human-readable kind label
/// - color: hex color for visualization (derived from symbol_kind)
/// - community_id: community membership (for grouping)
/// - cross_community_edges: count of edges to other communities
/// - is_entry_point: whether this is a process entry point
#[derive(Serialize)]
struct GraphNode {
    id: String,
    name: String,
    kind: String,
    symbol_kind_display: String,
    file: String,
    line: u32,
    signature: String,
    community: String,
    community_id: Option<String>,
    depth: u32,
    color: String,
    cross_community_edges: u32,
    is_entry_point: bool,
}

/// Phase 1: Extended GraphEdge with additional metadata for 2D visualization.
/// - edge_type_display: human-readable edge type label
/// - weight: relative importance (based on call frequency or relationship type)
#[derive(Serialize)]
struct GraphEdge {
    source: String,
    target: String,
    kind: String,
    edge_type_display: String,
    weight: f32,
}

/// Community metadata for 2D schematic grouping.
#[derive(Serialize)]
struct GraphCommunity {
    id: String,
    label: String,
    member_count: u32,
    symbol_ids: Vec<String>,
    internal_edge_count: u32,
    external_edge_count: u32,
}

/// Process cluster: ordered steps from entry point.
/// Used for identifying process flows in the 2D schematic.
#[derive(Serialize)]
struct ProcessCluster {
    id: String,
    name: String,
    entry_point_id: String,
    step_ids: Vec<String>,
    description: String,
}

#[derive(Serialize)]
struct RepoListItem {
    id: String,
    name: String,
    path: String,
    analyzed_at: String,
    symbol_count: u32,
    file_count: u32,
}

#[derive(Serialize)]
struct SymbolDetail {
    id: String,
    name: String,
    qualified_name: String,
    kind: String,
    file: String,
    start_line: u32,
    end_line: u32,
    signature: String,
    callers: Vec<ConnectionInfo>,
    callees: Vec<ConnectionInfo>,
}

#[derive(Serialize)]
struct ConnectionInfo {
    id: String,
    name: String,
    kind: String,
    rel_kind: String,
}

#[derive(Serialize)]
struct StatsResponse {
    symbols: usize,
    files: usize,
    relationships: usize,
    communities: usize,
}

// --- Helper Functions (Phase 1) ---

/// Get human-readable display name for a symbol kind.
fn symbol_kind_display(kind: &str) -> String {
    match kind {
        "Function" => "Function",
        "Method" => "Method",
        "Class" => "Class",
        "Interface" => "Interface",
        "TypeAlias" => "Type Alias",
        "Variable" => "Variable",
        "Constant" => "Constant",
        "Enum" => "Enum",
        "Module" => "Module",
        "Import" => "Import",
        "Section" => "Section",
        "Document" => "Document",
        "Rationale" => "Rationale",
        "Email" => "Email",
        "Conversation" => "Conversation",
        "Person" => "Person",
        "Attachment" => "Attachment",
        _ => "Unknown",
    }
    .to_string()
}

/// Get human-readable display name for a relationship kind.
fn edge_type_display(kind: &str) -> String {
    match kind {
        "CALLS" => "Calls",
        "CONTAINED_BY" => "Contained By",
        "MEMBER_OF" => "Member Of",
        "STEP_IN_PROCESS" => "Step In Process",
        "IMPORTS" => "Imports",
        "REFERENCES" => "References",
        "RATIONALE_FOR" => "Rationale For",
        "REPLY_TO" => "Reply To",
        "SENT_BY" => "Sent By",
        "RECEIVED_BY" => "Received By",
        "HAS_ATTACHMENT" => "Has Attachment",
        "PART_OF_CONVERSATION" => "Part Of Conversation",
        "MENTIONS" => "Mentions",
        _ => "Related",
    }
    .to_string()
}

/// Color hash: deterministic color based on symbol kind.
/// Returns hex color string for visualization.
fn color_for_kind(kind: &str) -> String {
    match kind {
        "Function" => "#6366F1",     // indigo
        "Method" => "#8B5CF6",       // violet
        "Class" => "#EC4899",        // pink
        "Interface" => "#06B6D4",    // cyan
        "TypeAlias" => "#F59E0B",    // amber
        "Variable" => "#10B981",     // emerald
        "Constant" => "#F59E0B",     // amber
        "Enum" => "#F87171",         // red
        "Module" => "#14B8A6",       // teal
        "Import" => "#64748B",       // slate
        "Section" => "#A78BFA",      // purple
        "Document" => "#60A5FA",     // blue
        "Rationale" => "#FBBF24",    // yellow
        "Email" => "#34D399",        // green
        "Conversation" => "#4F46E5", // indigo-600
        "Person" => "#F472B6",       // pink-400
        "Attachment" => "#7C3AED",   // violet-600
        _ => "#9CA3AF",              // gray
    }
    .to_string()
}

/// Calculate depth in call hierarchy using BFS from entry points.
/// Entry points are typically functions with no callers or process entry points.
async fn calculate_depths(
    symbols: &[myceliums_storage::CodeSymbol],
    relationships: &[myceliums_storage::Relationship],
) -> HashMap<String, u32> {
    let mut depths = HashMap::new();
    let mut has_caller = HashSet::new();

    // Find all symbols that have incoming calls
    for rel in relationships {
        if matches!(rel.kind, myceliums_storage::RelationshipKind::Calls) {
            has_caller.insert(rel.target_uid.clone());
        }
    }

    // Entry points: symbols with no callers
    let mut queue: VecDeque<String> = VecDeque::new();
    for sym in symbols {
        if !has_caller.contains(&sym.uid) {
            queue.push_back(sym.uid.clone());
            depths.insert(sym.uid.clone(), 0);
        }
    }

    // BFS to calculate depth
    let rel_map: HashMap<&str, Vec<&myceliums_storage::Relationship>> = {
        let mut m: HashMap<&str, Vec<&myceliums_storage::Relationship>> = HashMap::new();
        for rel in relationships {
            if matches!(rel.kind, myceliums_storage::RelationshipKind::Calls) {
                m.entry(rel.source_uid.as_str()).or_default().push(rel);
            }
        }
        m
    };

    while let Some(uid) = queue.pop_front() {
        if let Some(rels) = rel_map.get(uid.as_str()) {
            let current_depth = depths.get(&uid).copied().unwrap_or(0);
            for rel in rels {
                let target_depth = depths
                    .entry(rel.target_uid.clone())
                    .or_insert(current_depth + 1);
                if *target_depth == current_depth + 1 {
                    queue.push_back(rel.target_uid.clone());
                }
            }
        }
    }

    // Fill in any remaining symbols with a default depth
    for sym in symbols {
        depths.entry(sym.uid.clone()).or_insert(0);
    }

    depths
}

/// Count cross-community edges for each node.
/// A cross-community edge is an edge that goes to a node in a different community.
fn count_cross_community_edges(
    node_id: &str,
    node_community: &str,
    relationships: &[myceliums_storage::Relationship],
    community_map: &HashMap<String, String>,
) -> u32 {
    relationships
        .iter()
        .filter(|rel| {
            (rel.source_uid == node_id && {
                let target_comm = community_map
                    .get(&rel.target_uid)
                    .cloned()
                    .unwrap_or_default();
                target_comm != node_community && !target_comm.is_empty()
            }) || (rel.target_uid == node_id && {
                let source_comm = community_map
                    .get(&rel.source_uid)
                    .cloned()
                    .unwrap_or_default();
                source_comm != node_community && !source_comm.is_empty()
            })
        })
        .count() as u32
}

/// Build community membership map: symbol uid -> community label
async fn build_community_map(
    store: &Store,
    symbols: &[myceliums_storage::CodeSymbol],
    relationships: &[myceliums_storage::Relationship],
) -> HashMap<String, String> {
    // Try stored communities first
    if let Ok(communities) = store.get_communities().await {
        if !communities.is_empty() {
            let mut map = HashMap::new();
            for community in &communities {
                let top: Vec<&str> = community
                    .top_symbols
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                for sym in symbols {
                    if top.contains(&sym.name.as_str()) {
                        map.insert(sym.uid.clone(), community.label.clone());
                    }
                }
            }
            if !map.is_empty() {
                return map;
            }
        }
    }

    // Fallback: compute communities on-the-fly via Leiden
    if let Ok(map) = myceliums_core::compute_uid_to_community_label(symbols, relationships) {
        if !map.is_empty() {
            return map;
        }
    }

    // Final fallback: group by top-level directory
    let mut map = HashMap::new();
    for sym in symbols {
        let dir = sym
            .file_path
            .split('/')
            .take(2)
            .collect::<Vec<_>>()
            .join("/");
        if !dir.is_empty() {
            map.insert(sym.uid.clone(), dir);
        }
    }
    map
}

/// Extract process clusters from the stored processes.
/// Resolves step names from the description chain into symbol UIDs.
async fn extract_processes(
    store: &Store,
    symbols: &[myceliums_storage::CodeSymbol],
) -> Result<Vec<ProcessCluster>> {
    let processes = store.get_processes().await.unwrap_or_default();

    // Build name→uid lookup (first match wins; entry_point is already a name)
    let name_to_uid: HashMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.name.as_str(), s.uid.as_str()))
        .collect();

    let mut clusters = Vec::new();

    for process in processes {
        // Parse description chain: "1. foo → 2. bar → 3. baz"
        let step_names: Vec<&str> = process
            .description
            .split('→')
            .map(|s| s.trim())
            .map(|s| {
                // Strip leading "N. " prefix
                if let Some(dot_pos) = s.find(". ") {
                    &s[dot_pos + 2..]
                } else {
                    s
                }
            })
            .collect();

        let step_ids: Vec<String> = step_names
            .iter()
            .filter_map(|name| name_to_uid.get(name).map(|uid| uid.to_string()))
            .collect();

        let entry_point_id = name_to_uid
            .get(process.entry_point.as_str())
            .map(|uid| uid.to_string())
            .unwrap_or_else(|| process.entry_point.clone());

        clusters.push(ProcessCluster {
            id: process.uid.clone(),
            name: process.name.clone(),
            entry_point_id,
            step_ids,
            description: process.description.clone(),
        });
    }

    Ok(clusters)
}

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".myceliums")
}

fn registry_path() -> PathBuf {
    data_dir().join("repos.json")
}

async fn open_store(state: &AppState, repo_id: &str) -> Result<Store> {
    let db_path = RepoRegistry::repo_db_path(&state.data_dir, repo_id);
    Store::open(&db_path, repo_id).await
}

// --- Handlers ---

async fn index_handler() -> impl IntoResponse {
    (
        [
            ("cache-control", "no-cache, no-store, must-revalidate"),
            ("pragma", "no-cache"),
            ("expires", "0"),
        ],
        Html(VISUALIZATION_HTML),
    )
}

async fn favicon_handler() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "image/svg+xml")],
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><circle cx="16" cy="16" r="14" fill="#e94560"/></svg>"##,
    )
}

async fn list_repos(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let registry = match RepoRegistry::load(&state.registry_path) {
        Ok(r) => r,
        Err(e) => {
            error!("Failed to load registry: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Vec::<RepoListItem>::new()),
            );
        }
    };

    let repos: Vec<RepoListItem> = registry
        .list()
        .into_iter()
        .map(|r| RepoListItem {
            id: r.id.clone(),
            name: r.name.clone(),
            path: r.path.clone(),
            analyzed_at: r.analyzed_at.clone(),
            symbol_count: r.symbol_count,
            file_count: r.file_count,
        })
        .collect();

    // If there's a default repo, put it first
    let repos = if let Some(ref default_id) = state.default_repo_id {
        let mut sorted = repos;
        sorted.sort_by(|a, b| {
            let a_is_default = a.id == *default_id;
            let b_is_default = b.id == *default_id;
            b_is_default.cmp(&a_is_default)
        });
        sorted
    } else {
        repos
    };

    (StatusCode::OK, Json(repos))
}

/// Get enriched graph for 2D schematic view (Phase 1).
/// Includes extended node/edge metadata, communities, and processes.
async fn get_graph(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(GraphResponse {
                    nodes: vec![],
                    edges: vec![],
                    communities: vec![],
                    processes: vec![],
                }),
            );
        }
    };

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();
    let communities_raw = store.get_communities().await.unwrap_or_default();
    let community_map = build_community_map(&store, &symbols, &relationships).await;

    // Phase 1: Calculate depths
    let depths = calculate_depths(&symbols, &relationships).await;

    // Phase 1: Identify entry points
    let entry_points: HashSet<String> = {
        let mut has_caller = HashSet::new();
        for rel in &relationships {
            if matches!(rel.kind, myceliums_storage::RelationshipKind::Calls) {
                has_caller.insert(rel.target_uid.clone());
            }
        }
        symbols
            .iter()
            .filter(|s| !has_caller.contains(&s.uid))
            .map(|s| s.uid.clone())
            .collect()
    };

    // Phase 1: Build enriched nodes
    let nodes: Vec<GraphNode> = symbols
        .iter()
        .map(|s| {
            let comm_id = community_map.get(&s.uid).cloned();
            let comm = comm_id.clone().unwrap_or_default();
            let cross_edges =
                count_cross_community_edges(&s.uid, &comm, &relationships, &community_map);

            GraphNode {
                id: s.uid.clone(),
                name: s.name.clone(),
                kind: s.kind.to_string(),
                symbol_kind_display: symbol_kind_display(&s.kind.to_string()),
                file: s.file_path.clone(),
                line: s.start_line,
                signature: s.signature.clone(),
                community: comm.clone(),
                community_id: comm_id,
                depth: depths.get(&s.uid).copied().unwrap_or(0),
                color: color_for_kind(&s.kind.to_string()),
                cross_community_edges: cross_edges,
                is_entry_point: entry_points.contains(&s.uid),
            }
        })
        .collect();

    // Phase 1: Build enriched edges
    let edges: Vec<GraphEdge> = relationships
        .iter()
        .map(|r| {
            let kind_str = r.kind.to_string();
            GraphEdge {
                source: r.source_uid.clone(),
                target: r.target_uid.clone(),
                kind: kind_str.clone(),
                edge_type_display: edge_type_display(&kind_str),
                weight: match r.kind {
                    myceliums_storage::RelationshipKind::Calls => 1.0,
                    myceliums_storage::RelationshipKind::StepInProcess => 2.0,
                    _ => 0.5,
                },
            }
        })
        .collect();

    // Phase 1: Build community metadata
    let mut community_members: HashMap<String, Vec<String>> = HashMap::new();
    for node in &nodes {
        if !node.community.is_empty() {
            community_members
                .entry(node.community.clone())
                .or_default()
                .push(node.id.clone());
        }
    }

    // Synthesize communities from community_map if none stored
    let communities: Vec<GraphCommunity> = if !communities_raw.is_empty() {
        communities_raw
            .iter()
            .enumerate()
            .map(|(idx, c)| {
                let symbol_ids = community_members.get(&c.label).cloned().unwrap_or_default();
                GraphCommunity {
                    id: format!("community_{}", idx),
                    label: c.label.clone(),
                    member_count: c.member_count,
                    symbol_ids,
                    internal_edge_count: 0,
                    external_edge_count: 0,
                }
            })
            .collect()
    } else {
        // Build from community_map — sorted by member count descending
        let mut entries: Vec<(String, Vec<String>)> = community_members.into_iter().collect();
        entries.sort_by_key(|b| Reverse(b.1.len()));

        entries
            .into_iter()
            .enumerate()
            .map(|(idx, (label, symbol_ids))| {
                let member_count = symbol_ids.len() as u32;
                GraphCommunity {
                    id: format!("community_{}", idx),
                    label,
                    member_count,
                    symbol_ids,
                    internal_edge_count: 0,
                    external_edge_count: 0,
                }
            })
            .collect()
    };

    // Phase 1: Extract processes with resolved step UIDs
    let processes = extract_processes(&store, &symbols)
        .await
        .unwrap_or_default();

    (
        StatusCode::OK,
        Json(GraphResponse {
            nodes,
            edges,
            communities,
            processes,
        }),
    )
}

async fn get_symbol(
    State(state): State<Arc<AppState>>,
    Path((repo_id, uid)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (StatusCode::NOT_FOUND, Json(Option::<SymbolDetail>::None));
        }
    };

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();

    let symbol = match symbols.iter().find(|s| s.uid == uid) {
        Some(s) => s,
        None => return (StatusCode::NOT_FOUND, Json(None)),
    };

    let sym_map: HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut callers = Vec::new();
    let mut callees = Vec::new();

    for rel in &relationships {
        if rel.target_uid == uid {
            if let Some(src) = sym_map.get(rel.source_uid.as_str()) {
                callers.push(ConnectionInfo {
                    id: src.uid.clone(),
                    name: src.name.clone(),
                    kind: src.kind.to_string(),
                    rel_kind: rel.kind.to_string(),
                });
            }
        }
        if rel.source_uid == uid {
            if let Some(tgt) = sym_map.get(rel.target_uid.as_str()) {
                callees.push(ConnectionInfo {
                    id: tgt.uid.clone(),
                    name: tgt.name.clone(),
                    kind: tgt.kind.to_string(),
                    rel_kind: rel.kind.to_string(),
                });
            }
        }
    }

    let detail = SymbolDetail {
        id: symbol.uid.clone(),
        name: symbol.name.clone(),
        qualified_name: symbol.qualified_name.clone(),
        kind: symbol.kind.to_string(),
        file: symbol.file_path.clone(),
        start_line: symbol.start_line,
        end_line: symbol.end_line,
        signature: symbol.signature.clone(),
        callers,
        callees,
    };

    (StatusCode::OK, Json(Some(detail)))
}

async fn get_stats(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(StatsResponse {
                    symbols: 0,
                    files: 0,
                    relationships: 0,
                    communities: 0,
                }),
            );
        }
    };

    let symbols = store.symbol_count().await.unwrap_or(0);
    let files = store.file_count().await.unwrap_or(0);
    let relationships = store.relationship_count().await.unwrap_or(0);
    let communities = store.get_communities().await.map(|c| c.len()).unwrap_or(0);

    (
        StatusCode::OK,
        Json(StatsResponse {
            symbols,
            files,
            relationships,
            communities,
        }),
    )
}

#[derive(Deserialize)]
struct KnowledgeRequest {
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

async fn get_centrality(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
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

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();

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

async fn get_community_metrics(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
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

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();

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
struct ModuleCouplingParams {
    group_by_dir: Option<bool>,
}

async fn get_module_coupling(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<ModuleCouplingParams>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
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

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();
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

async fn get_cycles(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
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

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();

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

/// Phase 1: Get processes for 2D schematic view.
async fn get_processes(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (StatusCode::NOT_FOUND, Json(Vec::<ProcessCluster>::new()));
        }
    };

    let symbols = store.get_symbols().await.unwrap_or_default();
    let processes = extract_processes(&store, &symbols)
        .await
        .unwrap_or_default();

    (StatusCode::OK, Json(processes))
}

async fn query_knowledge(
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

        // Parse metadata for match locations
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

// --- LLM Provider Configuration ---

#[derive(Serialize, Deserialize)]
struct LlmProviderUpdate {
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
}

/// Get the current LLM provider configuration.
async fn get_llm_config() -> impl IntoResponse {
    use myceliums_core::global_config::GlobalConfig;

    let config = GlobalConfig::load(&data_dir()).unwrap_or_else(|_| GlobalConfig::default());

    let response = serde_json::json!({
        "provider": config.llm.provider,
        "model": config.llm.model,
        "base_url": config.llm.base_url,
        "api_key": config.llm.api_key,
    });

    (StatusCode::OK, Json(response))
}

/// Update the LLM provider configuration.
async fn set_llm_config(Json(body): Json<LlmProviderUpdate>) -> impl IntoResponse {
    use myceliums_core::global_config::GlobalConfig;

    let mut config = GlobalConfig::load(&data_dir()).unwrap_or_else(|_| GlobalConfig::default());

    if let Some(provider) = body.provider {
        if let Err(e) = config.set("llm.provider", &provider) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Some(model) = body.model {
        if let Err(e) = config.set("llm.model", &model) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Some(base_url) = body.base_url {
        if let Err(e) = config.set("llm.base_url", &base_url) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Some(api_key) = body.api_key {
        if let Err(e) = config.set("llm.api_key", &api_key) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            );
        }
    }

    if let Err(e) = config.save() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to save config: {}", e) })),
        );
    }

    let response = serde_json::json!({
        "provider": config.llm.provider,
        "model": config.llm.model,
        "base_url": config.llm.base_url,
        "api_key": config.llm.api_key,
    });

    (StatusCode::OK, Json(response))
}

// --- Graph API v1: Embedding-based Visualization ---

#[derive(Deserialize)]
struct GraphOverviewParams {
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
struct GraphSearchParams {
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
async fn graph_v1_overview(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<GraphOverviewParams>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
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

    let symbols_with_vecs = store.get_symbols_with_vectors().await.unwrap_or_default();
    let symbols: Vec<_> = symbols_with_vecs.iter().map(|(s, _)| s.clone()).collect();
    let relationships = store.get_relationships().await.unwrap_or_default();
    let community_map = build_community_map(&store, &symbols, &relationships).await;

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
async fn graph_v1_node(
    State(state): State<Arc<AppState>>,
    Path((repo_id, node_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
        Ok(s) => s,
        Err(e) => {
            error!("Failed to open store for {}: {}", repo_id, e);
            return (
                StatusCode::NOT_FOUND,
                Json(Option::<NodeDetailResponse>::None),
            );
        }
    };

    let symbols = store.get_symbols().await.unwrap_or_default();
    let relationships = store.get_relationships().await.unwrap_or_default();
    let community_map = build_community_map(&store, &symbols, &relationships).await;
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

    // Semantic neighbors via vector search
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
                    Err(_) => vec![],
                }
            } else {
                vec![]
            }
        }
        Err(_) => vec![],
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
async fn graph_v1_search(
    State(state): State<Arc<AppState>>,
    Path(repo_id): Path<String>,
    Query(params): Query<GraphSearchParams>,
) -> impl IntoResponse {
    let store = match open_store(&state, &repo_id).await {
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

    let top_k = params.top_k.unwrap_or(20).clamp(1, 100);

    // Embed the query using the same model as indexing
    let embedder = match myceliums_core::get_embedder().await {
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
    let query_vector = match embedder.embed_query(&params.q) {
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
        Err(_) => vec![],
    };

    (
        StatusCode::OK,
        Json(GraphSearchResponse {
            query: params.q,
            results,
        }),
    )
}

/// Middleware that logs request latency for API endpoints.
/// Logs path, method, status, and duration in milliseconds.
async fn latency_logger(request: Request<axum::body::Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let start = Instant::now();

    let response = next.run(request).await;

    let duration = start.elapsed();
    let status = response.status().as_u16();

    // Only log API requests (skip static assets)
    if path.starts_with("/api") {
        info!(
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration.as_millis() as u64,
            "request completed"
        );
    }

    response
}

/// Start the HTTP server on the given port.
///
/// If `repo_id` is provided, it will be pre-selected in the UI.
pub async fn start_server(port: u16, repo_id: Option<String>) -> Result<()> {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let state = Arc::new(AppState {
        data_dir: data_dir(),
        registry_path: registry_path(),
        default_repo_id: repo_id,
    });

    let app = Router::new()
        .route("/", get(index_handler))
        .route("/favicon.ico", get(favicon_handler))
        .route("/api/repos", get(list_repos))
        .route("/api/repos/{repo_id}/graph", get(get_graph))
        .route("/api/repos/{repo_id}/symbol/{uid}", get(get_symbol))
        .route("/api/repos/{repo_id}/stats", get(get_stats))
        .route("/api/repos/{repo_id}/processes", get(get_processes))
        .route("/api/repos/{repo_id}/knowledge", post(query_knowledge))
        // Graph analytics endpoints
        .route("/api/repos/{repo_id}/centrality", get(get_centrality))
        .route(
            "/api/repos/{repo_id}/community-metrics",
            get(get_community_metrics),
        )
        .route("/api/repos/{repo_id}/cycles", get(get_cycles))
        .route(
            "/api/repos/{repo_id}/module-coupling",
            get(get_module_coupling),
        )
        // LLM provider configuration endpoints
        .route("/api/llm/config", get(get_llm_config))
        .route("/api/llm/config", post(set_llm_config))
        // Graph API v1: embedding-based visualization endpoints
        .route("/api/v1/graph/overview/{repo_id}", get(graph_v1_overview))
        .route(
            "/api/v1/graph/nodes/{repo_id}/{node_id}",
            get(graph_v1_node),
        )
        .route("/api/v1/graph/search/{repo_id}", get(graph_v1_search))
        .layer(axum::middleware::from_fn(latency_logger))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_symbol_kind_display() {
        assert_eq!(symbol_kind_display("Function"), "Function");
        assert_eq!(symbol_kind_display("Method"), "Method");
        assert_eq!(symbol_kind_display("Class"), "Class");
        assert_eq!(symbol_kind_display("TypeAlias"), "Type Alias");
        assert_eq!(symbol_kind_display("Unknown"), "Unknown");
    }

    #[test]
    fn test_edge_type_display() {
        assert_eq!(edge_type_display("CALLS"), "Calls");
        assert_eq!(edge_type_display("IMPORTS"), "Imports");
        assert_eq!(edge_type_display("STEP_IN_PROCESS"), "Step In Process");
        assert_eq!(edge_type_display("UNKNOWN"), "Related");
    }

    #[test]
    fn test_color_for_kind() {
        assert_eq!(color_for_kind("Function"), "#6366F1");
        assert_eq!(color_for_kind("Class"), "#EC4899");
        assert_eq!(color_for_kind("Method"), "#8B5CF6");
        assert_eq!(color_for_kind("Variable"), "#10B981");
        assert_eq!(color_for_kind("Unknown"), "#9CA3AF");
    }

    #[test]
    fn test_count_cross_community_edges() {
        use myceliums_storage::{Relationship, RelationshipKind};

        let relationships = vec![
            Relationship {
                uid: "rel_1".to_string(),
                source_uid: "node_1".to_string(),
                target_uid: "node_2".to_string(),
                kind: RelationshipKind::Calls,
                repo_id: "test".to_string(),
                metadata: String::new(),
            },
            Relationship {
                uid: "rel_2".to_string(),
                source_uid: "node_1".to_string(),
                target_uid: "node_3".to_string(),
                kind: RelationshipKind::Calls,
                repo_id: "test".to_string(),
                metadata: String::new(),
            },
        ];

        let mut community_map = HashMap::new();
        community_map.insert("node_1".to_string(), "comm_a".to_string());
        community_map.insert("node_2".to_string(), "comm_a".to_string());
        community_map.insert("node_3".to_string(), "comm_b".to_string());

        let cross_edges =
            count_cross_community_edges("node_1", "comm_a", &relationships, &community_map);
        assert_eq!(cross_edges, 1); // edge to node_3 (different community)
    }

    #[tokio::test]
    async fn test_calculate_depths() {
        use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};

        let symbols = vec![
            CodeSymbol {
                uid: "sym_1".to_string(),
                name: "entry".to_string(),
                qualified_name: "entry".to_string(),
                kind: SymbolKind::Function,
                file_path: "test.rs".to_string(),
                start_line: 1,
                end_line: 5,
                signature: "fn entry()".to_string(),
                content: String::new(),
                repo_id: "test".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym_2".to_string(),
                name: "helper".to_string(),
                qualified_name: "helper".to_string(),
                kind: SymbolKind::Function,
                file_path: "test.rs".to_string(),
                start_line: 6,
                end_line: 10,
                signature: "fn helper()".to_string(),
                content: String::new(),
                repo_id: "test".to_string(),
                metadata: None,
            },
        ];

        let relationships = vec![Relationship {
            uid: "rel_1".to_string(),
            source_uid: "sym_1".to_string(),
            target_uid: "sym_2".to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }];

        let depths = calculate_depths(&symbols, &relationships).await;

        assert_eq!(depths.get("sym_1").copied(), Some(0)); // entry point
        assert_eq!(depths.get("sym_2").copied(), Some(1)); // called by entry point
    }

    #[test]
    fn test_graph_node_enrichment() {
        let node = GraphNode {
            id: "test_1".to_string(),
            name: "test_function".to_string(),
            kind: "Function".to_string(),
            symbol_kind_display: "Function".to_string(),
            file: "test.rs".to_string(),
            line: 42,
            signature: "fn test_function()".to_string(),
            community: "core".to_string(),
            community_id: Some("comm_1".to_string()),
            depth: 2,
            color: "#6366F1".to_string(),
            cross_community_edges: 3,
            is_entry_point: false,
        };

        assert_eq!(node.depth, 2);
        assert_eq!(node.cross_community_edges, 3);
        assert!(!node.is_entry_point);
    }

    #[test]
    fn test_graph_edge_enrichment() {
        let edge = GraphEdge {
            source: "sym_1".to_string(),
            target: "sym_2".to_string(),
            kind: "CALLS".to_string(),
            edge_type_display: "Calls".to_string(),
            weight: 1.0,
        };

        assert_eq!(edge.weight, 1.0);
        assert_eq!(edge.edge_type_display, "Calls");
    }

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
