//! Repository listing, the 2D schematic graph, symbol details, stats and processes.

use crate::state::{open_store, AppState};
use anyhow::Result;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use myceliums_storage::{RepoRegistry, Store};
use serde::Serialize;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use tracing::error;

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
pub(crate) fn symbol_kind_display(kind: &str) -> String {
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
pub(crate) fn color_for_kind(kind: &str) -> String {
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
pub(crate) async fn calculate_depths(
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
pub(crate) async fn build_community_map(
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

pub(crate) async fn list_repos(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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
pub(crate) async fn get_graph(
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

pub(crate) async fn get_symbol(
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

pub(crate) async fn get_stats(
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

/// Phase 1: Get processes for 2D schematic view.
pub(crate) async fn get_processes(
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::response::IntoResponse;
    use std::collections::HashMap;

    use crate::test_support::{app_state, register_repo, test_data_dir};

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

    #[tokio::test]
    async fn unknown_or_traversal_repo_id_is_404_and_creates_nothing() {
        let dir = test_data_dir("unknown_repo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        register_repo(&dir, "known-1");

        for bad in ["made-up", "..", "../outside"] {
            let resp = get_stats(State(app_state(dir.clone())), Path(bad.to_string()))
                .await
                .into_response();
            assert_eq!(resp.status(), StatusCode::NOT_FOUND, "{bad}");
        }
        assert!(
            !dir.join("data").exists(),
            "a read must never create a repo directory"
        );
        assert!(!dir.join("outside").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
