use anyhow::Result;
use myceliums_core::analyzer::{self, Analyzer};
use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
use myceliums_core::{
    attach_graph_edges, build_snapshot, compute_centrality, compute_community_metrics,
    compute_file_dependencies, compute_god_nodes, compute_hotspot_scores, compute_module_coupling,
    compute_ownership, compute_surprising_connections, detect_contracts, detect_cycles,
    detect_drift, detect_impact, diff_snapshots, export_mermaid, generate_architecture_diagram,
    hybrid_search as core_hybrid_search, hybrid_search_explain as core_hybrid_search_explain,
    link_decision_to_symbol, lint_architecture, list_snapshots, load_decisions,
    load_service_mappings, load_snapshot, load_snapshot_by_id, parse_codeowners, rerank_results,
    run_git_diff, save_decision, save_service_mapping, save_snapshot, search_symbols,
    search_symbols_explain, AdrStatus, ArchDecisionRecord, CommunityDetector, MermaidDiagramType,
    Ontology, ProcessFilter, ProcessTracer, RenamePlan,
};
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use rmcp::handler::server::tool::ToolRouter;
use rmcp::handler::server::wrapper::{Json, Parameters};
use rmcp::model::*;
use rmcp::schemars;
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{tool, tool_router, ServerHandler, ServiceExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::info;

mod format;

fn data_dir() -> PathBuf {
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".myceliums")
}

fn registry_path() -> PathBuf {
    data_dir().join("repos.json")
}

pub struct MyceliumsMcp {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl Default for MyceliumsMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl MyceliumsMcp {
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
        }
    }
}

// Tool parameter types
#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct AnalyzeParams {
    /// Path to the project directory to analyze
    pub path: String,
    /// Force full re-analysis even if cache is fresh (default: false)
    pub force: Option<bool>,
    /// Maximum cache age in minutes before re-analysis (default: 60)
    pub max_age_minutes: Option<u64>,
    /// Skip embedding generation for faster analysis (default: false)
    pub skip_embeddings: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DeleteParams {
    /// Repository ID to delete
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SearchParams {
    /// Search query string
    pub query: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Maximum results to return
    pub limit: Option<usize>,
    /// Show scoring breakdown and graph paths for each result (default: false)
    pub explain: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SymbolContextParams {
    /// Name of the symbol to look up
    pub symbol_name: String,
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct CypherQueryParams {
    /// Cypher query string
    pub query: String,
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct RenameParams {
    /// Name of the symbol to rename
    pub symbol_name: String,
    /// New name for the symbol
    pub new_name: String,
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DetectImpactParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Diff string (optional -- if omitted, runs git diff HEAD in repo path)
    pub diff: Option<String>,
    /// Graph traversal depth (default: 2)
    pub depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct HybridSearchParams {
    /// Search query string
    pub query: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Maximum results to return
    pub limit: Option<usize>,
    /// Apply cross-encoder reranking to results (default: false)
    pub rerank: Option<bool>,
    /// Show scoring breakdown and graph paths for each result (default: false)
    pub explain: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ReviewContextParams {
    /// Diff string (optional — if omitted, runs git diff HEAD in repo path)
    pub diff: Option<String>,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Graph traversal depth for blast radius (default: 1)
    pub depth: Option<u32>,
    /// Include full source code of changed symbols (default: false)
    pub include_source: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetProcessesParams {
    /// Repository ID
    pub repo_id: String,
    /// Filter by entry point name (case-insensitive substring match)
    pub entry: Option<String>,
    /// Filter by keyword in process description/flow (case-insensitive substring match)
    pub filter: Option<String>,
    /// Limit number of processes to return
    pub limit: Option<usize>,
    /// Show only processes with N or more steps
    pub min_steps: Option<u32>,
}

// New tool param types
#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCommunitiesParams {
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCommunityDetailParams {
    /// Repository ID
    pub repo_id: String,
    /// Community UID or label to get details for
    pub community_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSymbolDefinitionParams {
    /// Repository ID
    pub repo_id: String,
    /// Symbol name or qualified name to find
    pub symbol_name: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct FindDeadCodeParams {
    /// Repository ID
    pub repo_id: String,
    /// Exclude patterns (comma-separated regexes) for known entry points (optional)
    pub exclude_patterns: Option<String>,
    /// Limit number of results
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCallersParams {
    /// Repository ID
    pub repo_id: String,
    /// Symbol name or qualified name to find callers for
    pub symbol_name: String,
    /// Maximum transitive depth (default: 3)
    pub max_depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCalleesParams {
    /// Repository ID
    pub repo_id: String,
    /// Symbol name or qualified name to find callees for
    pub symbol_name: String,
    /// Maximum transitive depth (default: 3)
    pub max_depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetFileSymbolsParams {
    /// Repository ID
    pub repo_id: String,
    /// File path to list symbols for
    pub file_path: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetStatsParams {
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetGodNodesParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Number of top nodes to return (default: 10)
    pub top_n: Option<usize>,
    /// Coupling threshold: nodes with degree > this are flagged as high-coupling (default: 20)
    pub coupling_threshold: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSurprisingConnectionsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Minimum surprise score in [0.0, 1.0] to include in results (default: 0.1)
    pub min_surprise_score: Option<f64>,
    /// Maximum number of connections to return (default: 50)
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct FindPathParams {
    /// Symbol name or qualified name for the start of the path
    pub from_symbol: String,
    /// Symbol name or qualified name for the end of the path
    pub to_symbol: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Maximum BFS depth (default: 10)
    pub max_depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetKnowledgeGapsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Filter by gap category: "untested", "isolated", "undocumented", "single_point_of_failure" (optional — returns all if omitted)
    pub category: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetRationaleParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Symbol name or qualified name to get rationale for (optional — provide this OR file_path)
    pub symbol_name: Option<String>,
    /// File path to get all rationale comments for (optional — provide this OR symbol_name)
    pub file_path: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSuggestedQuestionsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Diff string (optional — if omitted, runs git diff HEAD in repo path)
    pub diff: Option<String>,
    /// Maximum number of questions to return (default: 5)
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetGraphDiffParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SearchEmailsParams {
    /// Search keyword to match in email subject or body
    pub query: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Filter by person email address (optional)
    pub person: Option<String>,
    /// Filter by date (ISO 8601 prefix, e.g. "2026-04") (optional)
    pub date: Option<String>,
    /// Maximum results to return (default: 20)
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetConversationParams {
    /// Conversation symbol UID
    pub conversation_uid: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetPersonContextParams {
    /// Person email address or name to search for
    pub person: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Maximum results to return (default: 50)
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct QueryKnowledgeParams {
    /// Natural language query or symbol name to search for across all knowledge
    pub query: String,
    /// Repository ID or path (auto-detects if omitted)
    pub repo_id: Option<String>,
    /// Include source citations with line numbers and context (default: true)
    pub include_sources: Option<bool>,
    /// Maximum results to return (default: 20)
    pub limit: Option<usize>,
}

// Cross-repo comparison parameter types (premium feature)

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSchemaParams {
    /// Entity type name(s) to get schema for (comma-separated, e.g., "Function,Class,Method")
    pub entity_types: String,
    /// Whether to include edge type schemas in addition to entity schemas (default: false)
    pub include_edges: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct IsolateIntentParams {
    /// Natural language description of the intent/feature to isolate
    /// (e.g., "tree-sitter parsing", "authentication flow", "API routing")
    pub intent: String,
    /// Repository ID to isolate from
    pub repo_id: String,
    /// Maximum symbols to include in the slice (default: 50)
    pub max_symbols: Option<usize>,
    /// Call graph expansion depth (default: 2)
    pub depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DifferentiateIntentParams {
    /// Natural language description of the intent to compare
    /// (e.g., "tree-sitter parsing", "authentication flow")
    pub intent: String,
    /// Source repository ID (the approach to compare FROM)
    pub source_repo_id: String,
    /// Target repository ID (the approach to compare TO)
    pub target_repo_id: String,
    /// Similarity threshold for symbol alignment (default: 0.65, range 0.0-1.0)
    pub similarity_threshold: Option<f64>,
    /// Maximum symbols per slice (default: 50)
    pub max_symbols: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetGitContextParams {
    /// Symbol name to look up git metadata for
    pub symbol_name: String,
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct PlanAdaptationParams {
    /// Natural language description of the intent
    pub intent: String,
    /// Repository whose approach to adapt FROM
    pub source_repo_id: String,
    /// Repository whose approach to adapt TO
    pub target_repo_id: String,
    /// Direction: "source_to_target" (adapt target to match source) or "target_to_source" (default: "source_to_target")
    pub direction: Option<String>,
    /// Maximum symbols per slice (default: 50)
    pub max_symbols: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCentralityParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Centrality metric to sort by: "degree", "betweenness", "closeness", "eigenvector" (default: "betweenness")
    pub metric: Option<String>,
    /// Number of top nodes to return (default: 15)
    pub top_n: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCommunityMetricsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DetectCyclesParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Include CALLS edges in cycle detection (default: true)
    pub include_calls: Option<bool>,
    /// Include IMPORTS edges in cycle detection (default: true)
    pub include_imports: Option<bool>,
    /// Minimum number of symbols in a cycle to report (default: 2)
    pub min_cycle_size: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetDependenciesParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// File path to analyze dependencies for
    pub file_path: String,
    /// Maximum transitive depth (default: unlimited)
    pub max_depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetModuleCouplingParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Group by directory instead of individual files (default: false)
    pub group_by_directory: Option<bool>,
    /// Maximum results to return (default: 30)
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct QualityHotspotsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Number of top hotspots to return (default: 20)
    pub top_n: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ArchitectureLintParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Comma-separated rule IDs to check (default: all). Available: circular_dependency, god_node, high_fan_out, unstable_dependency
    pub rules: Option<String>,
    /// God node degree threshold (default: 20)
    pub god_node_threshold: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ArchitectureViewParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DetectDriftParams2 {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetOwnershipParams {
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct MapServiceParams {
    /// Repository ID
    pub repo_id: String,
    /// Community label to map
    pub community_label: String,
    /// Human-readable service name
    pub service_name: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetServiceMapParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct RecordDecisionParams {
    /// Repository ID
    pub repo_id: String,
    /// ADR title
    pub title: String,
    /// Status: "proposed", "accepted", "deprecated", "superseded" (default: "proposed")
    pub status: Option<String>,
    /// Context and motivation
    pub context: String,
    /// The decision made
    pub decision: String,
    /// Expected consequences (optional)
    pub consequences: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetDecisionsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Filter by status (optional)
    pub status: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct LinkDecisionParams {
    /// Repository ID
    pub repo_id: String,
    /// ADR ID to link
    pub decision_id: String,
    /// Symbol name to link to
    pub symbol_name: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SnapshotDiffParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Snapshot ID to compare FROM (default: second-to-latest)
    pub from_snapshot: Option<String>,
    /// Snapshot ID to compare TO (default: latest)
    pub to_snapshot: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetContractsParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ExportMermaidParams {
    /// Repository ID (optional, uses most recent if omitted)
    pub repo_id: Option<String>,
    /// Diagram type: "flowchart", "class", or "graph" (default: "flowchart")
    pub diagram_type: Option<String>,
}

// Output types

#[derive(Serialize, schemars::JsonSchema)]
pub struct CentralityOutput {
    pub nodes: Vec<CentralityNodeOutput>,
    pub total_nodes: usize,
    pub metric: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CentralityNodeOutput {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub degree: f64,
    pub betweenness: f64,
    pub closeness: f64,
    pub eigenvector: f64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CommunityMetricsOutput {
    pub modularity: f64,
    pub community_count: usize,
    pub cohesion: Vec<CohesionEntry>,
    pub coupling: Vec<CouplingEntry>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CohesionEntry {
    pub community: String,
    pub density: f64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CouplingEntry {
    pub community_a: String,
    pub community_b: String,
    pub edge_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CyclesOutput {
    pub cycles: Vec<CycleItemOutput>,
    pub total_count: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CycleItemOutput {
    pub members: Vec<String>,
    pub size: usize,
    pub files: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct DependenciesOutput {
    pub file_path: String,
    pub direct_deps: Vec<String>,
    pub transitive_deps: Vec<String>,
    pub dependents: Vec<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ModuleCouplingOutput {
    pub modules: Vec<ModuleCouplingEntry>,
    pub total_count: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ModuleCouplingEntry {
    pub module_path: String,
    pub afferent: u32,
    pub efferent: u32,
    pub instability: f64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct GraphDiffOutput {
    pub repo_id: String,
    pub previous_snapshot_at: String,
    pub current_snapshot_at: String,
    pub added_symbols: Vec<GraphDiffEntry>,
    pub removed_symbols: Vec<GraphDiffEntry>,
    pub added_edges: Vec<GraphDiffEntry>,
    pub removed_edges: Vec<GraphDiffEntry>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct GraphDiffEntry {
    pub uid: String,
    pub label: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct RationaleOutput {
    pub rationales: Vec<RationaleItem>,
    pub total_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct RationaleItem {
    pub prefix: String,
    pub text: String,
    pub file_path: String,
    pub line: u32,
    /// The symbol this rationale is linked to (if any)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_symbol: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct AnalyzeOutput {
    pub repo_id: String,
    pub symbols: usize,
    pub files: usize,
    pub relationships: usize,
    pub communities: usize,
    pub processes: usize,
    /// Number of symbols that received an embedding vector.
    pub symbols_embedded: usize,
    /// Number of symbols that were candidates for embedding.
    pub symbols_total: usize,
    /// Number of symbols whose embedding failed (0 = fully embedded / skipped).
    pub embedding_failures: usize,
    /// Whether this result came from cache (true) or a fresh analysis (false)
    pub cached: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SearchOutput {
    pub results: Vec<SearchResultItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SearchResultItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<serde_json::Value>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SymbolContextOutput {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    pub content: String,
    pub callers: Vec<String>,
    pub callees: Vec<String>,
    pub metadata: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct HybridSearchOutput {
    pub results: Vec<HybridSearchResultItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct HybridSearchResultItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    pub combined_score: f64,
    pub bm25_rank: Option<usize>,
    pub vector_rank: Option<usize>,
    pub bm25_score: Option<f64>,
    pub vector_score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explain: Option<serde_json::Value>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct TextOutput {
    pub text: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessesOutput {
    pub processes: Vec<ProcessItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct ProcessItem {
    pub name: String,
    pub entry_point: String,
    pub step_count: u32,
    pub description: String,
}

// New output types
#[derive(Serialize, schemars::JsonSchema)]
pub struct CommunitiesOutput {
    pub communities: Vec<CommunityItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CommunityItem {
    pub uid: String,
    pub label: String,
    pub member_count: u32,
    pub top_symbols: String,
    pub summary: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CommunityDetailOutput {
    pub uid: String,
    pub label: String,
    pub member_count: u32,
    pub summary: String,
    pub symbols: Vec<SymbolItem>,
    pub internal_edge_count: u32,
    pub external_edge_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SymbolItem {
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SymbolDefinitionOutput {
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    pub content: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct DeadCodeOutput {
    pub symbols: Vec<DeadCodeItem>,
    pub total_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct DeadCodeItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CallersOutput {
    pub symbol_name: String,
    pub callers: Vec<CallerItem>,
    pub total_count: u32,
    pub depth_limited: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CallerItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub depth: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CalleesOutput {
    pub symbol_name: String,
    pub callees: Vec<CalleeItem>,
    pub total_count: u32,
    pub depth_limited: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CalleeItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub depth: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FileSymbolsOutput {
    pub file_path: String,
    pub symbols: Vec<FileSymbolItem>,
    pub total_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FileSymbolItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct StatsOutput {
    pub symbol_counts: std::collections::HashMap<String, u32>,
    pub total_symbols: u32,
    pub total_files: u32,
    pub total_relationships: u32,
    pub relationship_counts: std::collections::HashMap<String, u32>,
    pub language_counts: std::collections::HashMap<String, u32>,
    pub community_count: u32,
    pub process_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct GodNodesOutput {
    pub nodes: Vec<GodNodeItemOutput>,
    pub total_symbols: usize,
    pub high_coupling_count: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct GodNodeItemOutput {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub degree: u32,
    pub in_degree: u32,
    pub out_degree: u32,
    pub is_high_coupling: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SurprisingConnectionsOutput {
    pub connections: Vec<SurprisingConnectionItemOutput>,
    pub total_cross_community_edges: usize,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SurprisingConnectionItemOutput {
    pub source_name: String,
    pub source_qualified_name: String,
    pub target_name: String,
    pub target_qualified_name: String,
    pub source_community: String,
    pub target_community: String,
    pub surprise_score: f64,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct KnowledgeGapsOutput {
    pub gaps: Vec<KnowledgeGapItem>,
    pub total_count: u32,
    pub summary: KnowledgeGapSummary,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct KnowledgeGapItem {
    /// Gap category: "untested", "isolated", "undocumented", "single_point_of_failure"
    pub category: String,
    /// Severity: "high", "medium", "low"
    pub severity: String,
    /// Affected symbol name
    pub symbol_name: String,
    /// Qualified name
    pub qualified_name: String,
    /// Symbol kind
    pub kind: String,
    /// File path
    pub file_path: String,
    /// Start line
    pub start_line: u32,
    /// Human-readable description of the gap
    pub description: String,
    /// Suggested action to address this gap
    pub suggestion: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct KnowledgeGapSummary {
    pub untested_count: u32,
    pub isolated_count: u32,
    pub undocumented_count: u32,
    pub single_point_of_failure_count: u32,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FindPathOutput {
    pub from_symbol: String,
    pub to_symbol: String,
    pub steps: Vec<PathStepItem>,
    pub total_depth: u32,
    pub found: bool,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct PathStepItem {
    pub symbol_name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub edge_type: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SuggestedQuestionsOutput {
    pub questions: Vec<SuggestedQuestion>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct SuggestedQuestion {
    /// The question text
    pub question: String,
    /// Severity level: high, medium, low
    pub severity: String,
    /// Category: callers, coverage, api_contract, complexity, unused, patterns
    pub category: String,
    /// Primary symbol(s) or file(s) referenced
    pub references: Vec<String>,
    /// Explanation of why this question matters
    pub rationale: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct KnowledgeQueryOutput {
    pub query: String,
    pub total_mentions: usize,
    pub unique_sources: usize,
    pub results: Vec<KnowledgeResultItem>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct KnowledgeResultItem {
    pub source_name: String,
    pub source_kind: String,
    pub source_file: String,
    pub source_uid: String,
    pub mentioned_symbol: String,
    pub mentioned_kind: String,
    pub mentioned_file: String,
    pub mentioned_line: u32,
    pub mentioned_uid: String,
    pub match_context: String,
    pub match_line: u32,
    pub confidence: f64,
}

/// Load the partial-index warning for a store, if the index is partially
/// embedded. Reads the accounting recorded at index time — no per-query vector
/// scan. Returns `None` when the index is complete or has no accounting.
///
/// A genuine load error is logged (so it is observable rather than silently
/// indistinguishable from "no warning") but does not fail the query.
async fn partial_index_warning(store: &Store) -> Option<String> {
    match myceliums_core::EmbeddingStats::load(store).await {
        Ok(stats) => stats.and_then(|stats| stats.partial_index_warning()),
        Err(e) => {
            tracing::warn!("Failed to load embedding accounting for partial-index warning: {e}");
            None
        }
    }
}

/// Load the embedding accounting for a store, for reporting in the `analyze`
/// response's cached path.
///
/// A genuine load error propagates. A *missing* record means a legacy index
/// built before accounting existed: treat it as fully embedded (using
/// `symbol_count` as the total) rather than "0 of N", which would falsely
/// imply a total embedding failure. Centralized so the cached and non-cached
/// query paths derive the reported counts the same way — the non-cached path
/// gets the equivalent record straight from `AnalysisResult::embedding_stats`.
async fn load_embedding_stats(
    store: &Store,
    symbol_count: usize,
) -> Result<myceliums_core::EmbeddingStats, rmcp::ErrorData> {
    let loaded = myceliums_core::EmbeddingStats::load(store)
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
    Ok(loaded
        .unwrap_or_else(|| myceliums_core::EmbeddingStats::complete(symbol_count, symbol_count)))
}

#[tool_router]
impl MyceliumsMcp {
    #[tool(
        name = "analyze",
        description = "Analyze a codebase and build its knowledge graph. Parses TypeScript and Python files, extracts symbols, resolves call relationships, detects communities, and traces execution flows. Uses cached analysis if fresh enough (set force=true to bypass cache)."
    )]
    async fn analyze(
        &self,
        Parameters(params): Parameters<AnalyzeParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let path = PathBuf::from(&params.path);
        let abs_path = std::fs::canonicalize(&path)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid path: {}", e), None))?;

        let repo_id = analyzer::repo_id_from_path(&abs_path);
        let repo_name = analyzer::repo_name_from_path(&abs_path);
        let force = params.force.unwrap_or(false);

        // Cache check: return cached results if fresh enough
        if !force {
            let registry = RepoRegistry::load(&registry_path())
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
            if let Some(repo_info) = registry.get(&repo_id) {
                let cache_config = CacheCheckConfig {
                    max_age_minutes: params.max_age_minutes.unwrap_or(60),
                    ..Default::default()
                };
                if let CacheDecision::UseCached { repo_id, reason } =
                    cache::check_cache(repo_info, &abs_path, &cache_config)
                {
                    info!("Using cached analysis: {}", reason);
                    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
                    let store = Store::open(&db_path, &repo_id).await.map_err(|e| {
                        rmcp::ErrorData::internal_error(format!("Store error: {}", e), None)
                    })?;
                    let symbols = store
                        .get_symbols()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let files = store
                        .get_files()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let relationships = store
                        .get_relationships()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let communities = store
                        .get_communities()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let processes = store
                        .get_processes()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let embedding_stats = load_embedding_stats(&store, symbols.len()).await?;
                    let output = AnalyzeOutput {
                        repo_id,
                        symbols: symbols.len(),
                        files: files.len(),
                        relationships: relationships.len(),
                        communities: communities.len(),
                        processes: processes.len(),
                        symbols_embedded: embedding_stats.symbols_embedded,
                        symbols_total: embedding_stats.symbols_total,
                        embedding_failures: embedding_stats.embedding_failures,
                        cached: true,
                    };
                    return Ok(Json(TextOutput {
                        text: format::format_analyze(&output),
                    }));
                }
            }
        }

        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        std::fs::create_dir_all(&db_path).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to create dir: {}", e), None)
        })?;

        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Store error: {}", e), None))?;
        store
            .delete_repo_data()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Delete error: {}", e), None))?;

        let skip_embeddings = params.skip_embeddings.unwrap_or(false);
        // Honor the project's .myceliums.toml (analysis filters, embedding
        // model) the same way the CLI does.
        let config_path = abs_path.join(myceliums_core::config::CONFIG_FILENAME);
        let analyzer = if config_path.exists() {
            let cfg = myceliums_core::config::ProjectConfig::load(&config_path).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Invalid {}: {}", config_path.display(), e),
                    None,
                )
            })?;
            Analyzer::with_config(store, abs_path.clone(), cfg)
        } else {
            Analyzer::new(store, abs_path.clone())
        };
        let analyzer = analyzer.set_skip_embeddings(skip_embeddings);
        let result = analyzer
            .analyze()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Analysis error: {}", e), None))?;

        // Community detection and process tracing
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Store error: {}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = CommunityDetector::detect(&symbols, &relationships, &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let community_count = store
            .store_communities(&communities)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let processes = ProcessTracer::trace(&symbols, &relationships, &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let process_count = store
            .store_processes(&processes)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Get current git commit for cache tracking
        let analyzed_commit = cache::get_head_commit(&abs_path).ok();

        let mut registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        registry.register(RepoInfo {
            id: repo_id.clone(),
            name: repo_name,
            path: abs_path.to_string_lossy().to_string(),
            analyzed_at: chrono::Utc::now().to_rfc3339(),
            symbol_count: result.symbol_count as u32,
            file_count: result.file_count as u32,
            analyzed_commit,
        });
        registry
            .save()
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Save lightweight snapshot for future diff comparisons
        let snapshot = build_snapshot(&repo_id, &symbols, &relationships);
        let _ = save_snapshot(&data_dir(), &snapshot); // best-effort

        let embedding_stats = result.embedding_stats();
        let output = AnalyzeOutput {
            repo_id,
            symbols: result.symbol_count,
            files: result.file_count,
            relationships: result.relationship_count,
            communities: community_count,
            processes: process_count,
            symbols_embedded: embedding_stats.symbols_embedded,
            symbols_total: embedding_stats.symbols_total,
            embedding_failures: embedding_stats.embedding_failures,
            cached: false,
        };
        Ok(Json(TextOutput {
            text: format::format_analyze(&output),
        }))
    }

    #[tool(name = "delete", description = "Delete a repository's analysis data")]
    async fn delete(
        &self,
        Parameters(params): Parameters<DeleteParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let mut registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let info = registry.remove(&params.repo_id).ok_or_else(|| {
            rmcp::ErrorData::internal_error(
                format!("Repository not found: {}", params.repo_id),
                None,
            )
        })?;
        registry
            .save()
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        if db_path.exists() {
            std::fs::remove_dir_all(&db_path)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        }
        Ok(Json(TextOutput {
            text: format!("Deleted: {} ({})", info.name, params.repo_id),
        }))
    }

    #[tool(
        name = "context_search",
        description = "Search for functions, classes, and symbols in the knowledge graph. Preferred over grep for finding code entities — returns structured results with file locations, types, and relevance scores."
    )]
    async fn context_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let explain = params.explain.unwrap_or(false);
        let results = if explain {
            search_symbols_explain(&symbols, &params.query)
        } else {
            search_symbols(&symbols, &params.query)
        };
        let limit = params.limit.unwrap_or(20);
        let items: Vec<SearchResultItem> = results
            .into_iter()
            .take(limit)
            .map(|r| SearchResultItem {
                name: r.symbol.name,
                qualified_name: r.symbol.qualified_name,
                kind: r.symbol.kind.to_string(),
                file_path: r.symbol.file_path,
                start_line: r.symbol.start_line,
                end_line: r.symbol.end_line,
                signature: r.symbol.signature,
                score: r.score,
                explain: r.explain.and_then(|e| serde_json::to_value(e).ok()),
            })
            .collect();
        Ok(Json(TextOutput {
            text: format::format_search_results(&params.query, &items),
        }))
    }

    #[tool(
        name = "symbol_context",
        description = "Get a symbol's full context: source code, callers, and callees. Use this to understand how a function is used before modifying it — reveals dependencies that file reading alone cannot."
    )]
    async fn symbol_context(
        &self,
        Parameters(params): Parameters<SymbolContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbol = symbols
            .iter()
            .find(|s| s.name == params.symbol_name || s.qualified_name == params.symbol_name)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!("Symbol not found: {}", params.symbol_name),
                    None,
                )
            })?;

        use myceliums_storage::RelationshipKind;
        let uid_to_name: std::collections::HashMap<&str, &str> = symbols
            .iter()
            .map(|s| (s.uid.as_str(), s.name.as_str()))
            .collect();

        let callers: Vec<String> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls && r.target_uid == symbol.uid)
            .filter_map(|r| {
                uid_to_name
                    .get(r.source_uid.as_str())
                    .map(|s| s.to_string())
            })
            .collect();

        let callees: Vec<String> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls && r.source_uid == symbol.uid)
            .filter_map(|r| {
                uid_to_name
                    .get(r.target_uid.as_str())
                    .map(|s| s.to_string())
            })
            .collect();
        let output = SymbolContextOutput {
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file_path: symbol.file_path.clone(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: symbol.signature.clone(),
            content: symbol.content.clone(),
            callers,
            callees,
            metadata: symbol.metadata.clone(),
        };
        Ok(Json(TextOutput {
            text: format::format_symbol_context(&output),
        }))
    }

    #[tool(
        name = "search_documents",
        description = "Search through all analyzed code content using BM25 text search"
    )]
    async fn search_documents(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        self.context_search(Parameters(params)).await
    }

    #[tool(
        name = "hybrid_search",
        description = "Search using hybrid BM25 + vector semantic search with Reciprocal Rank Fusion for better search quality"
    )]
    async fn hybrid_search(
        &self,
        Parameters(params): Parameters<HybridSearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to load embedding model: {}", e),
                    None,
                )
            })?;

        let explain = params.explain.unwrap_or(false);
        let limit = params.limit.unwrap_or(20);
        let mut results = if explain {
            core_hybrid_search_explain(&embedder, &symbols, &store, &params.query, limit)
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
        } else {
            core_hybrid_search(&embedder, &symbols, &store, &params.query, limit)
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
        };

        // Apply reranking if requested, using the reranker recorded at indexing time
        if params.rerank.unwrap_or(false) {
            let reranker_id = embedder.meta().reranker.clone();
            results = rerank_results(&params.query, results, reranker_id.as_deref())
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        }

        // Attach graph edges for explain mode
        if explain {
            let relationships = store
                .get_relationships()
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
            let uid_to_name: std::collections::HashMap<&str, &str> = symbols
                .iter()
                .map(|s| (s.uid.as_str(), s.name.as_str()))
                .collect();
            attach_graph_edges(&mut results, &relationships, &uid_to_name);
        }

        let items: Vec<HybridSearchResultItem> = results
            .into_iter()
            .map(|r| HybridSearchResultItem {
                name: r.symbol.name,
                qualified_name: r.symbol.qualified_name,
                kind: r.symbol.kind.to_string(),
                file_path: r.symbol.file_path,
                start_line: r.symbol.start_line,
                end_line: r.symbol.end_line,
                signature: r.symbol.signature,
                combined_score: r.combined_score,
                bm25_rank: r.bm25_rank,
                vector_rank: r.vector_rank,
                bm25_score: r.bm25_score,
                vector_score: r.vector_score,
                explain: r.explain.and_then(|e| serde_json::to_value(e).ok()),
            })
            .collect();
        let warning = partial_index_warning(&store).await;
        Ok(Json(TextOutput {
            text: format::with_index_warning(
                warning,
                format::format_hybrid_results(&params.query, &items),
            ),
        }))
    }

    #[tool(
        name = "cypher_query",
        description = "Execute a Cypher query against the knowledge graph. Supports MATCH, RETURN, WHERE, ORDER BY, LIMIT, SKIP, CONTAINS, IS NULL. Blocks write operations."
    )]
    async fn cypher_query(
        &self,
        Parameters(params): Parameters<CypherQueryParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let executor = myceliums_cypher::CypherExecutor::from_store(&store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let results = executor
            .execute(&params.query)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let json_results = serde_json::to_value(&results)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_cypher_results(&params.query, &json_results),
        }))
    }

    #[tool(
        name = "detect_impact",
        description = "Analyze the impact of code changes before committing. Traces changed symbols through the call graph to find indirectly affected code. Use this proactively when modifying functions to catch unintended side effects."
    )]
    async fn detect_impact_tool(
        &self,
        Parameters(params): Parameters<DetectImpactParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let diff_text = match params.diff {
            Some(d) => d,
            None => {
                let registry = RepoRegistry::load(&registry_path())
                    .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                let repo_info = registry.get(&repo_id).ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Repository not found: {}", repo_id),
                        None,
                    )
                })?;
                run_git_diff(&repo_info.path).map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("git diff failed: {}", e), None)
                })?
            }
        };
        if diff_text.trim().is_empty() {
            return Ok(Json(TextOutput {
                text: "No changes detected (empty diff).".to_string(),
            }));
        }
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let depth = params.depth.unwrap_or(2);
        let report = detect_impact(&diff_text, &symbols, &relationships, depth);
        let json_report = serde_json::to_value(&report)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_impact_report(&json_report),
        }))
    }

    #[tool(
        name = "rename_symbol",
        description = "Preview renaming a symbol across the codebase. Returns a rename plan with all edits needed (does not modify files)."
    )]
    async fn rename_symbol(
        &self,
        Parameters(params): Parameters<RenameParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let plan = RenamePlan::create(
            &symbols,
            &relationships,
            &params.symbol_name,
            &params.new_name,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let json_plan = serde_json::to_value(&plan)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_rename_plan(&json_plan),
        }))
    }

    #[tool(
        name = "semantic_search",
        description = "Search for symbols using semantic similarity (vector embeddings). Returns symbols most similar in meaning to the query."
    )]
    async fn semantic_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to load embedding model: {}", e),
                    None,
                )
            })?;
        let query_vector = embedder.embed_query(&params.query).await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to embed query: {}", e), None)
        })?;
        let limit = params.limit.unwrap_or(10);
        let results = store
            .vector_search(&query_vector, limit)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let items: Vec<SearchResultItem> = results
            .into_iter()
            .map(|(sym, score)| SearchResultItem {
                name: sym.name,
                qualified_name: sym.qualified_name,
                kind: sym.kind.to_string(),
                file_path: sym.file_path,
                start_line: sym.start_line,
                end_line: sym.end_line,
                signature: sym.signature,
                score: score as f64,
                explain: None,
            })
            .collect();
        let warning = partial_index_warning(&store).await;
        Ok(Json(TextOutput {
            text: format::with_index_warning(
                warning,
                format::format_search_results(&params.query, &items),
            ),
        }))
    }

    #[tool(
        name = "get_review_context",
        description = "Get a compact structural summary of code changes for efficient review. Analyzes a diff to identify changed symbols, their callers and callees, and affected communities — returning signatures instead of full source to minimize token usage."
    )]
    async fn get_review_context(
        &self,
        Parameters(params): Parameters<ReviewContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diff_text = match params.diff {
            Some(d) => d,
            None => {
                let registry = RepoRegistry::load(&registry_path())
                    .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                let repo_info = registry.get(&repo_id).ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Repository not found: {}", repo_id),
                        None,
                    )
                })?;
                run_git_diff(&repo_info.path).map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("git diff failed: {}", e), None)
                })?
            }
        };

        if diff_text.trim().is_empty() {
            return Ok(Json(TextOutput {
                text: "No changes detected (empty diff).".to_string(),
            }));
        }

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let depth = params.depth.unwrap_or(1);
        let include_source = params.include_source.unwrap_or(false);
        let report = detect_impact(&diff_text, &symbols, &relationships, depth);

        // Build uid -> symbol lookup
        let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build caller/callee maps
        use myceliums_storage::RelationshipKind;
        let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callers_of
                    .entry(rel.target_uid.as_str())
                    .or_default()
                    .push(rel.source_uid.as_str());
                callees_of
                    .entry(rel.source_uid.as_str())
                    .or_default()
                    .push(rel.target_uid.as_str());
            }
        }

        // Collect changed symbol details
        let mut changed_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        let mut changed_uids: std::collections::HashSet<String> = std::collections::HashSet::new();

        for cs in &report.directly_changed {
            if let Some(sym) = symbols
                .iter()
                .find(|s| s.qualified_name == cs.qualified_name)
            {
                changed_uids.insert(sym.uid.clone());
                changed_entries.push(format::ReviewSymbolEntry {
                    signature: if sym.signature.is_empty() {
                        sym.name.clone()
                    } else {
                        sym.signature.clone()
                    },
                    file_path: sym.file_path.clone(),
                    start_line: sym.start_line,
                    kind: sym.kind.to_string(),
                    source: if include_source {
                        Some(sym.content.clone())
                    } else {
                        None
                    },
                });
            }
        }

        // Collect callers of changed symbols
        let mut caller_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        let mut seen_callers: std::collections::HashSet<String> = std::collections::HashSet::new();
        for uid in &changed_uids {
            if let Some(caller_uids) = callers_of.get(uid.as_str()) {
                for caller_uid in caller_uids {
                    if !changed_uids.contains(*caller_uid)
                        && seen_callers.insert(caller_uid.to_string())
                    {
                        if let Some(sym) = uid_to_symbol.get(caller_uid) {
                            caller_entries.push(format::ReviewSymbolEntry {
                                signature: if sym.signature.is_empty() {
                                    sym.name.clone()
                                } else {
                                    sym.signature.clone()
                                },
                                file_path: sym.file_path.clone(),
                                start_line: sym.start_line,
                                kind: sym.kind.to_string(),
                                source: None,
                            });
                        }
                    }
                }
            }
        }

        // Collect callees of changed symbols
        let mut callee_entries: Vec<format::ReviewSymbolEntry> = Vec::new();
        let mut seen_callees: std::collections::HashSet<String> = std::collections::HashSet::new();
        for uid in &changed_uids {
            if let Some(callee_uids) = callees_of.get(uid.as_str()) {
                for callee_uid in callee_uids {
                    if !changed_uids.contains(*callee_uid)
                        && seen_callees.insert(callee_uid.to_string())
                    {
                        if let Some(sym) = uid_to_symbol.get(callee_uid) {
                            callee_entries.push(format::ReviewSymbolEntry {
                                signature: if sym.signature.is_empty() {
                                    sym.name.clone()
                                } else {
                                    sym.signature.clone()
                                },
                                file_path: sym.file_path.clone(),
                                start_line: sym.start_line,
                                kind: sym.kind.to_string(),
                                source: None,
                            });
                        }
                    }
                }
            }
        }

        // Find communities touched by changed symbols
        let mut touched_communities: Vec<String> = Vec::new();
        for community in &communities {
            let top_syms = &community.top_symbols;
            for cs in &report.directly_changed {
                if top_syms.contains(&cs.name) {
                    touched_communities.push(community.label.clone());
                    break;
                }
            }
        }
        touched_communities.sort();
        touched_communities.dedup();

        // Token estimate: count chars/4 for compact output, estimate full source size
        let mut full_source_chars: usize = 0;
        for file_path in &report.affected_files {
            for sym in &symbols {
                if sym.file_path == *file_path {
                    full_source_chars += sym.content.len();
                }
            }
        }

        let changed_files: std::collections::HashSet<&str> = report
            .directly_changed
            .iter()
            .map(|cs| cs.file_path.as_str())
            .collect();

        let context = format::ReviewContext {
            changed: changed_entries,
            callers: caller_entries,
            callees: callee_entries,
            communities_touched: touched_communities,
            changed_file_count: changed_files.len(),
            full_source_tokens: full_source_chars / 4,
        };

        Ok(Json(TextOutput {
            text: format::format_review_context(&context),
        }))
    }

    #[tool(
        name = "get_processes",
        description = "Get execution flows showing how functions chain together (e.g. request handler → validation → database → response). Use to understand architecture and data flow before refactoring."
    )]
    async fn get_processes(
        &self,
        Parameters(params): Parameters<GetProcessesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let processes = store
            .get_processes()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Apply filters
        let filter = ProcessFilter {
            entry: params.entry,
            filter: params.filter,
            limit: params.limit,
            min_steps: params.min_steps,
        };
        let filtered = filter.apply(&processes);

        let items: Vec<ProcessItem> = filtered
            .into_iter()
            .map(|p| ProcessItem {
                name: p.name,
                entry_point: p.entry_point,
                step_count: p.step_count,
                description: p.description,
            })
            .collect();
        Ok(Json(TextOutput {
            text: format::format_processes(&items),
        }))
    }

    #[tool(
        name = "get_communities",
        description = "List all detected communities with summary stats and top symbols. Use to understand code organization and find related modules."
    )]
    async fn get_communities(
        &self,
        Parameters(params): Parameters<GetCommunitiesParams>,
    ) -> Result<Json<CommunitiesOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let items: Vec<CommunityItem> = communities
            .into_iter()
            .map(|c| CommunityItem {
                uid: c.uid,
                label: c.label,
                member_count: c.member_count,
                top_symbols: c.top_symbols,
                summary: c.summary,
            })
            .collect();

        Ok(Json(CommunitiesOutput { communities: items }))
    }

    #[tool(
        name = "get_community_detail",
        description = "Get full details of a community: member symbols, internal relationships, and entry points. Use to understand community structure before refactoring."
    )]
    async fn get_community_detail(
        &self,
        Parameters(params): Parameters<GetCommunityDetailParams>,
    ) -> Result<Json<CommunityDetailOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let community = communities
            .iter()
            .find(|c| c.uid == params.community_id || c.label == params.community_id)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!("Community not found: {}", params.community_id),
                    None,
                )
            })?;

        // top_symbols is a comma-separated list of names (not UIDs)
        let member_names: std::collections::HashSet<String> = community
            .top_symbols
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        // Match symbols by name — top_symbols only has top ~5 names,
        // so this returns the most important community members
        let community_symbols: Vec<SymbolItem> = symbols
            .iter()
            .filter(|s| member_names.contains(&s.name))
            .map(|s| SymbolItem {
                uid: s.uid.clone(),
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind.to_string(),
                file_path: s.file_path.clone(),
                start_line: s.start_line,
                end_line: s.end_line,
                signature: s.signature.clone(),
            })
            .collect();

        // Build UID set from matched symbols for edge counting
        let member_uids: std::collections::HashSet<String> =
            community_symbols.iter().map(|s| s.uid.clone()).collect();

        // Count internal and external edges
        let mut internal_edges = 0u32;
        let mut external_edges = 0u32;

        for rel in &relationships {
            let src_in = member_uids.contains(&rel.source_uid);
            let tgt_in = member_uids.contains(&rel.target_uid);
            if src_in || tgt_in {
                if src_in && tgt_in {
                    internal_edges += 1;
                } else {
                    external_edges += 1;
                }
            }
        }

        Ok(Json(CommunityDetailOutput {
            uid: community.uid.clone(),
            label: community.label.clone(),
            member_count: community.member_count,
            summary: community.summary.clone(),
            symbols: community_symbols,
            internal_edge_count: internal_edges,
            external_edge_count: external_edges,
        }))
    }

    #[tool(
        name = "get_symbol_definition",
        description = "Get the complete definition and source code of a symbol. Use to understand what a function/class does before using or modifying it."
    )]
    async fn get_symbol_definition(
        &self,
        Parameters(params): Parameters<GetSymbolDefinitionParams>,
    ) -> Result<Json<SymbolDefinitionOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Try qualified name first (exact), then short name
        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == params.symbol_name)
            .collect();

        let symbol = if matches.len() == 1 {
            matches[0]
        } else if matches.is_empty() {
            let by_name: Vec<_> = symbols
                .iter()
                .filter(|s| s.name == params.symbol_name)
                .collect();
            match by_name.len() {
                0 => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", params.symbol_name),
                        None,
                    ))
                }
                1 => by_name[0],
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    return Err(rmcp::ErrorData::internal_error(
                        format!(
                            "Ambiguous symbol '{}'. Matches: {}",
                            params.symbol_name,
                            names.join(", ")
                        ),
                        None,
                    ));
                }
            }
        } else {
            matches[0]
        };

        Ok(Json(SymbolDefinitionOutput {
            uid: symbol.uid.clone(),
            name: symbol.name.clone(),
            qualified_name: symbol.qualified_name.clone(),
            kind: symbol.kind.to_string(),
            file_path: symbol.file_path.clone(),
            start_line: symbol.start_line,
            end_line: symbol.end_line,
            signature: symbol.signature.clone(),
            content: symbol.content.clone(),
        }))
    }

    #[tool(
        name = "find_dead_code",
        description = "Find symbols with no incoming function calls (potential dead code). Exclude common entry points with exclude_patterns. Use before cleanup/refactoring."
    )]
    async fn find_dead_code(
        &self,
        Parameters(params): Parameters<FindDeadCodeParams>,
    ) -> Result<Json<DeadCodeOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        use myceliums_storage::RelationshipKind;

        // Pre-build set of UIDs that are call targets — O(R) instead of O(S*R)
        let called_uids: std::collections::HashSet<&str> = relationships
            .iter()
            .filter(|r| r.kind == RelationshipKind::Calls)
            .map(|r| r.target_uid.as_str())
            .collect();

        let mut dead_code: Vec<DeadCodeItem> = symbols
            .iter()
            .filter(|s| {
                matches!(
                    s.kind,
                    myceliums_storage::SymbolKind::Function | myceliums_storage::SymbolKind::Method
                )
            })
            .filter(|s| !called_uids.contains(s.uid.as_str()))
            .filter(|s| {
                // Filter by exclude_patterns if provided
                if let Some(ref patterns) = params.exclude_patterns {
                    let exclude = patterns.split(',').collect::<Vec<_>>();
                    !exclude
                        .iter()
                        .any(|pattern| s.name.contains(pattern.trim()))
                } else {
                    true
                }
            })
            .map(|s| DeadCodeItem {
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind.to_string(),
                file_path: s.file_path.clone(),
                start_line: s.start_line,
                end_line: s.end_line,
                signature: s.signature.clone(),
            })
            .collect();

        let total = dead_code.len() as u32;
        if let Some(limit) = params.limit {
            dead_code.truncate(limit);
        }

        Ok(Json(DeadCodeOutput {
            symbols: dead_code,
            total_count: total,
        }))
    }

    #[tool(
        name = "get_callers",
        description = "Find all functions that call a given symbol, with optional depth limit. Use to understand impact of changes or find usage patterns."
    )]
    async fn get_callers(
        &self,
        Parameters(params): Parameters<GetCallersParams>,
    ) -> Result<Json<CallersOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == params.symbol_name)
            .collect();
        let target_symbol = if matches.len() == 1 {
            matches[0]
        } else if matches.is_empty() {
            let by_name: Vec<_> = symbols
                .iter()
                .filter(|s| s.name == params.symbol_name)
                .collect();
            match by_name.len() {
                0 => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", params.symbol_name),
                        None,
                    ))
                }
                1 => by_name[0],
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    return Err(rmcp::ErrorData::internal_error(
                        format!(
                            "Ambiguous symbol '{}'. Matches: {}",
                            params.symbol_name,
                            names.join(", ")
                        ),
                        None,
                    ));
                }
            }
        } else {
            matches[0]
        };

        use myceliums_storage::RelationshipKind;
        let max_depth = params.max_depth.unwrap_or(3);

        // Pre-build adjacency: target_uid -> [source_uids] for CALLS edges
        let mut callers_of: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callers_of
                    .entry(rel.target_uid.as_str())
                    .or_default()
                    .push(rel.source_uid.as_str());
            }
        }

        // Pre-build symbol lookup
        let sym_by_uid: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // BFS traversal
        let mut callers = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        queue.push_back((target_symbol.uid.as_str(), 0u32));
        visited.insert(target_symbol.uid.as_str());

        while let Some((uid, depth)) = queue.pop_front() {
            if depth > 0 && depth <= max_depth {
                if let Some(sym) = sym_by_uid.get(uid) {
                    callers.push(CallerItem {
                        name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        depth,
                    });
                }
            }

            if depth < max_depth {
                if let Some(sources) = callers_of.get(uid) {
                    for &source_uid in sources {
                        if !visited.contains(source_uid) {
                            visited.insert(source_uid);
                            queue.push_back((source_uid, depth + 1));
                        }
                    }
                }
            }
        }

        let total_count = callers.len() as u32;
        let depth_limited = callers.iter().any(|c| c.depth >= max_depth);

        Ok(Json(CallersOutput {
            symbol_name: target_symbol.name.clone(),
            callers,
            total_count,
            depth_limited,
        }))
    }

    #[tool(
        name = "get_callees",
        description = "Find all functions called by a given symbol, with optional depth limit. Use to understand dependencies and call chains."
    )]
    async fn get_callees(
        &self,
        Parameters(params): Parameters<GetCalleesParams>,
    ) -> Result<Json<CalleesOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == params.symbol_name)
            .collect();
        let source_symbol = if matches.len() == 1 {
            matches[0]
        } else if matches.is_empty() {
            let by_name: Vec<_> = symbols
                .iter()
                .filter(|s| s.name == params.symbol_name)
                .collect();
            match by_name.len() {
                0 => {
                    return Err(rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", params.symbol_name),
                        None,
                    ))
                }
                1 => by_name[0],
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    return Err(rmcp::ErrorData::internal_error(
                        format!(
                            "Ambiguous symbol '{}'. Matches: {}",
                            params.symbol_name,
                            names.join(", ")
                        ),
                        None,
                    ));
                }
            }
        } else {
            matches[0]
        };

        use myceliums_storage::RelationshipKind;
        let max_depth = params.max_depth.unwrap_or(3);

        // Pre-build adjacency: source_uid -> [target_uids] for CALLS edges
        let mut callees_of: std::collections::HashMap<&str, Vec<&str>> =
            std::collections::HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callees_of
                    .entry(rel.source_uid.as_str())
                    .or_default()
                    .push(rel.target_uid.as_str());
            }
        }

        let sym_by_uid: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // BFS traversal
        let mut callees = Vec::new();
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();

        queue.push_back((source_symbol.uid.as_str(), 0u32));
        visited.insert(source_symbol.uid.as_str());

        while let Some((uid, depth)) = queue.pop_front() {
            if depth > 0 && depth <= max_depth {
                if let Some(sym) = sym_by_uid.get(uid) {
                    callees.push(CalleeItem {
                        name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        end_line: sym.end_line,
                        depth,
                    });
                }
            }

            if depth < max_depth {
                if let Some(targets) = callees_of.get(uid) {
                    for &target_uid in targets {
                        if !visited.contains(target_uid) {
                            visited.insert(target_uid);
                            queue.push_back((target_uid, depth + 1));
                        }
                    }
                }
            }
        }

        let total_count = callees.len() as u32;
        let depth_limited = callees.iter().any(|c| c.depth >= max_depth);

        Ok(Json(CalleesOutput {
            symbol_name: source_symbol.name.clone(),
            callees,
            total_count,
            depth_limited,
        }))
    }

    #[tool(
        name = "get_file_symbols",
        description = "List all symbols defined in a specific file with kinds and signatures. Use for file-level navigation and understanding file contents."
    )]
    async fn get_file_symbols(
        &self,
        Parameters(params): Parameters<GetFileSymbolsParams>,
    ) -> Result<Json<FileSymbolsOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let file_symbols: Vec<FileSymbolItem> = symbols
            .iter()
            .filter(|s| s.file_path == params.file_path)
            .map(|s| FileSymbolItem {
                name: s.name.clone(),
                qualified_name: s.qualified_name.clone(),
                kind: s.kind.to_string(),
                start_line: s.start_line,
                end_line: s.end_line,
                signature: s.signature.clone(),
            })
            .collect();

        let total_count = file_symbols.len() as u32;

        Ok(Json(FileSymbolsOutput {
            file_path: params.file_path,
            symbols: file_symbols,
            total_count,
        }))
    }

    #[tool(
        name = "get_god_nodes",
        description = "Identify the highest-degree symbols (god nodes) in the call graph. Returns the top-N most connected symbols ranked by total incoming + outgoing CALLS edges. High-coupling nodes (degree > threshold) are flagged as architectural bottlenecks."
    )]
    async fn get_god_nodes(
        &self,
        Parameters(params): Parameters<GetGodNodesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let top_n = params.top_n.unwrap_or(10);
        let coupling_threshold = params.coupling_threshold.unwrap_or(20);
        let god_nodes = compute_god_nodes(&symbols, &relationships, top_n, coupling_threshold);

        let high_coupling_count = god_nodes.iter().filter(|n| n.is_high_coupling).count();
        let total_symbols = symbols.len();

        let nodes: Vec<GodNodeItemOutput> = god_nodes
            .into_iter()
            .map(|n| GodNodeItemOutput {
                name: n.name,
                qualified_name: n.qualified_name,
                kind: n.kind,
                file_path: n.file_path,
                degree: n.degree,
                in_degree: n.in_degree,
                out_degree: n.out_degree,
                is_high_coupling: n.is_high_coupling,
            })
            .collect();

        let output = GodNodesOutput {
            nodes,
            total_symbols,
            high_coupling_count,
        };
        Ok(Json(TextOutput {
            text: format::format_god_nodes(&output),
        }))
    }

    #[tool(
        name = "get_surprising_connections",
        description = "Detect surprising cross-community CALLS edges: connections between symbols in different Leiden communities that rarely interact. Ranked by surprise score (0–1); higher means more isolated/unexpected coupling. Use to find hidden architectural dependencies."
    )]
    async fn get_surprising_connections(
        &self,
        Parameters(params): Parameters<GetSurprisingConnectionsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let min_surprise = params.min_surprise_score.unwrap_or(0.1);
        let limit = params.limit.unwrap_or(50);

        let connections =
            compute_surprising_connections(&symbols, &relationships, min_surprise, limit)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let total_cross_community_edges = connections.len();
        let items: Vec<SurprisingConnectionItemOutput> = connections
            .into_iter()
            .map(|c| SurprisingConnectionItemOutput {
                source_name: c.source_name,
                source_qualified_name: c.source_qualified_name,
                target_name: c.target_name,
                target_qualified_name: c.target_qualified_name,
                source_community: c.source_community,
                target_community: c.target_community,
                surprise_score: c.surprise_score,
            })
            .collect();

        let output = SurprisingConnectionsOutput {
            connections: items,
            total_cross_community_edges,
        };
        Ok(Json(TextOutput {
            text: format::format_surprising_connections(&output),
        }))
    }

    #[tool(
        name = "find_path",
        description = "Find the shortest path between two symbols in the knowledge graph using BFS across all relationship types (CALLS, CONTAINED_BY, IMPORTS, etc.). Answers 'how are these two things connected?' directly. Returns the ordered list of symbols and edge types along the path."
    )]
    async fn find_path(
        &self,
        Parameters(params): Parameters<FindPathParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Resolve from_symbol
        let from_symbol = Self::resolve_symbol(&symbols, &params.from_symbol)?;
        // Resolve to_symbol
        let to_symbol = Self::resolve_symbol(&symbols, &params.to_symbol)?;

        let max_depth = params.max_depth.unwrap_or(10);

        // Build bidirectional adjacency list across ALL relationship types
        // Each entry: neighbor_uid -> edge_type_label
        let mut adjacency: HashMap<&str, Vec<(&str, String)>> = HashMap::new();
        for rel in &relationships {
            let edge_label = rel.kind.to_string();
            adjacency
                .entry(rel.source_uid.as_str())
                .or_default()
                .push((rel.target_uid.as_str(), edge_label.clone()));
            adjacency
                .entry(rel.target_uid.as_str())
                .or_default()
                .push((rel.source_uid.as_str(), format!("{}_REV", edge_label)));
        }

        // BFS from from_symbol to to_symbol
        let sym_by_uid: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        let mut visited: std::collections::HashSet<&str> = std::collections::HashSet::new();
        // parent map: child_uid -> (parent_uid, edge_type)
        let mut parent: HashMap<&str, (&str, String)> = HashMap::new();
        let mut queue: std::collections::VecDeque<(&str, u32)> = std::collections::VecDeque::new();

        queue.push_back((from_symbol.uid.as_str(), 0));
        visited.insert(from_symbol.uid.as_str());

        let mut found = false;
        while let Some((uid, depth)) = queue.pop_front() {
            if uid == to_symbol.uid.as_str() {
                found = true;
                break;
            }
            if depth >= max_depth {
                continue;
            }
            if let Some(neighbors) = adjacency.get(uid) {
                for (neighbor_uid, edge_type) in neighbors {
                    if !visited.contains(neighbor_uid) {
                        visited.insert(neighbor_uid);
                        parent.insert(neighbor_uid, (uid, edge_type.clone()));
                        queue.push_back((neighbor_uid, depth + 1));
                    }
                }
            }
        }

        let output = if found {
            // Reconstruct path from to_symbol back to from_symbol
            let mut path_uids: Vec<(&str, String)> = Vec::new();
            let mut current = to_symbol.uid.as_str();
            while current != from_symbol.uid.as_str() {
                if let Some((prev, edge)) = parent.get(current) {
                    path_uids.push((current, edge.clone()));
                    current = prev;
                } else {
                    break;
                }
            }
            path_uids.reverse();

            let mut steps = Vec::new();
            // First step: the from_symbol itself (no incoming edge)
            steps.push(PathStepItem {
                symbol_name: from_symbol.name.clone(),
                qualified_name: from_symbol.qualified_name.clone(),
                kind: from_symbol.kind.to_string(),
                file_path: from_symbol.file_path.clone(),
                start_line: from_symbol.start_line,
                edge_type: String::new(),
            });
            for (uid, edge) in &path_uids {
                if let Some(sym) = sym_by_uid.get(uid) {
                    steps.push(PathStepItem {
                        symbol_name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        edge_type: edge.clone(),
                    });
                }
            }

            let total_depth = path_uids.len() as u32;
            FindPathOutput {
                from_symbol: from_symbol.name.clone(),
                to_symbol: to_symbol.name.clone(),
                steps,
                total_depth,
                found: true,
            }
        } else {
            FindPathOutput {
                from_symbol: from_symbol.name.clone(),
                to_symbol: to_symbol.name.clone(),
                steps: Vec::new(),
                total_depth: 0,
                found: false,
            }
        };

        Ok(Json(TextOutput {
            text: format::format_path(&output),
        }))
    }

    #[tool(
        name = "query_knowledge",
        description = "Query cross-domain knowledge: find emails and documents that mention code symbols. Returns source citations with exact line numbers and context snippets. Useful for discovering how code is discussed in documentation and communication channels."
    )]
    async fn query_knowledge(
        &self,
        Parameters(params): Parameters<QueryKnowledgeParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let limit = params.limit.unwrap_or(20).min(100);
        let include_sources = params.include_sources.unwrap_or(true);

        // Step 1: Find matching code symbols using BM25 search
        let matching_symbols = search_symbols(&symbols, &params.query)
            .into_iter()
            .filter_map(|result| symbols.iter().find(|s| s.name == result.symbol.name))
            .collect::<Vec<_>>();

        if matching_symbols.is_empty() {
            let output = KnowledgeQueryOutput {
                query: params.query,
                total_mentions: 0,
                unique_sources: 0,
                results: vec![],
            };
            return Ok(Json(TextOutput {
                text: format::format_knowledge_query(&output),
            }));
        }

        // Step 2: Find all Mentions relationships pointing to matched symbols
        let mut results = Vec::new();
        let mut source_uids = std::collections::HashSet::new();

        for matched_symbol in &matching_symbols {
            for rel in &relationships {
                // Look for Mentions relationships where target is our matched code symbol
                if rel.kind == myceliums_storage::RelationshipKind::Mentions
                    && rel.target_uid == matched_symbol.uid
                {
                    source_uids.insert(rel.source_uid.clone());

                    // Step 3: Find source symbol
                    if let Some(source_symbol) = symbols.iter().find(|s| s.uid == rel.source_uid) {
                        // Filter to content symbols only
                        if !is_content_symbol(&source_symbol.kind) {
                            continue;
                        }

                        // Step 4: Parse metadata and extract matches
                        let matches_metadata = parse_mentions_metadata(&rel.metadata);
                        for match_info in matches_metadata {
                            let result_item = KnowledgeResultItem {
                                source_name: source_symbol.name.clone(),
                                source_kind: source_symbol.kind.to_string(),
                                source_file: source_symbol.file_path.clone(),
                                source_uid: source_symbol.uid.clone(),
                                mentioned_symbol: matched_symbol.name.clone(),
                                mentioned_kind: matched_symbol.kind.to_string(),
                                mentioned_file: matched_symbol.file_path.clone(),
                                mentioned_line: matched_symbol.start_line,
                                mentioned_uid: matched_symbol.uid.clone(),
                                match_context: if include_sources {
                                    match_info.context.clone()
                                } else {
                                    String::new()
                                },
                                match_line: match_info.line,
                                confidence: match_info.confidence,
                            };
                            results.push(result_item);
                        }
                    }
                }
            }
        }

        // Step 5: Sort and limit results
        results.sort_by(|a, b| {
            // Sort by: source_kind (Email first), then source_name, then match_line
            let kind_order = |k: &str| match k {
                "Email" => 0,
                "Document" => 1,
                "Section" => 2,
                _ => 3,
            };
            let kind_cmp = kind_order(&a.source_kind).cmp(&kind_order(&b.source_kind));
            if kind_cmp != std::cmp::Ordering::Equal {
                return kind_cmp;
            }
            let name_cmp = a.source_name.cmp(&b.source_name);
            if name_cmp != std::cmp::Ordering::Equal {
                return name_cmp;
            }
            a.match_line.cmp(&b.match_line)
        });

        let total_mentions = results.len();
        results.truncate(limit);

        let output = KnowledgeQueryOutput {
            query: params.query,
            total_mentions,
            unique_sources: source_uids.len(),
            results,
        };

        Ok(Json(TextOutput {
            text: format::format_knowledge_query(&output),
        }))
    }

    #[tool(
        name = "get_rationale",
        description = "Get design rationale comments (NOTE:, HACK:, WHY:, TODO:, FIXME:, IMPORTANT:) linked to a symbol or file. Use to understand *why* code was written a certain way — surfaces inline reasoning from comments."
    )]
    async fn get_rationale(
        &self,
        Parameters(params): Parameters<GetRationaleParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        use myceliums_storage::RelationshipKind;

        // Collect all rationale symbols
        let rationale_syms: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| s.kind == myceliums_storage::SymbolKind::Rationale)
            .collect();

        let uid_to_sym: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build rationale_uid -> target_symbol_name map
        let mut rationale_targets: HashMap<&str, String> = HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::RationaleFor {
                if let Some(target) = uid_to_sym.get(rel.target_uid.as_str()) {
                    rationale_targets.insert(rel.source_uid.as_str(), target.name.clone());
                }
            }
        }

        let items: Vec<RationaleItem> = if let Some(ref symbol_name) = params.symbol_name {
            // Find the target symbol
            let target = symbols
                .iter()
                .find(|s| s.qualified_name == *symbol_name || s.name == *symbol_name)
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Symbol not found: {}", symbol_name),
                        None,
                    )
                })?;

            // Find rationale nodes linked to this symbol via RATIONALE_FOR
            let linked_rationale_uids: std::collections::HashSet<&str> = relationships
                .iter()
                .filter(|r| r.kind == RelationshipKind::RationaleFor && r.target_uid == target.uid)
                .map(|r| r.source_uid.as_str())
                .collect();

            rationale_syms
                .iter()
                .filter(|s| linked_rationale_uids.contains(s.uid.as_str()))
                .map(|s| RationaleItem {
                    prefix: s.signature.clone(),
                    text: s.content.clone(),
                    file_path: s.file_path.clone(),
                    line: s.start_line,
                    target_symbol: Some(target.name.clone()),
                })
                .collect()
        } else if let Some(ref file_path) = params.file_path {
            // Return all rationale nodes in a given file
            rationale_syms
                .iter()
                .filter(|s| s.file_path == *file_path)
                .map(|s| RationaleItem {
                    prefix: s.signature.clone(),
                    text: s.content.clone(),
                    file_path: s.file_path.clone(),
                    line: s.start_line,
                    target_symbol: rationale_targets.get(s.uid.as_str()).cloned(),
                })
                .collect()
        } else {
            // Return all rationale nodes in the repo
            rationale_syms
                .iter()
                .map(|s| RationaleItem {
                    prefix: s.signature.clone(),
                    text: s.content.clone(),
                    file_path: s.file_path.clone(),
                    line: s.start_line,
                    target_symbol: rationale_targets.get(s.uid.as_str()).cloned(),
                })
                .collect()
        };

        let total_count = items.len() as u32;
        let output = RationaleOutput {
            rationales: items,
            total_count,
        };
        Ok(Json(TextOutput {
            text: format::format_rationale(&output),
        }))
    }

    #[tool(
        name = "get_knowledge_gaps",
        description = "Detect structural weaknesses in the codebase: untested code (functions with no test callers), isolated modules (communities with few external connections), documentation gaps (files with many symbols but no rationale/doc nodes), and single points of failure (symbols that are the only bridge between communities). Use to prioritize testing, documentation, and refactoring efforts."
    )]
    async fn get_knowledge_gaps(
        &self,
        Parameters(params): Parameters<GetKnowledgeGapsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        use myceliums_storage::RelationshipKind;

        let category_filter = params.category.as_deref();
        let mut gaps: Vec<KnowledgeGapItem> = Vec::new();

        // Pre-build lookup maps
        let uid_to_sym: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Build caller/callee adjacency
        let mut callers_of: HashMap<&str, Vec<&str>> = HashMap::new();
        let mut callees_of: HashMap<&str, Vec<&str>> = HashMap::new();
        for rel in &relationships {
            if rel.kind == RelationshipKind::Calls {
                callers_of
                    .entry(rel.target_uid.as_str())
                    .or_default()
                    .push(rel.source_uid.as_str());
                callees_of
                    .entry(rel.source_uid.as_str())
                    .or_default()
                    .push(rel.target_uid.as_str());
            }
        }

        // --- 1. Untested code: functions/methods with no callers from test files ---
        if category_filter.is_none() || category_filter == Some("untested") {
            let called_uids: std::collections::HashSet<&str> = relationships
                .iter()
                .filter(|r| r.kind == RelationshipKind::Calls)
                .map(|r| r.target_uid.as_str())
                .collect();

            for sym in &symbols {
                if !matches!(
                    sym.kind,
                    myceliums_storage::SymbolKind::Function | myceliums_storage::SymbolKind::Method
                ) {
                    continue;
                }
                // Skip test files themselves and trivial symbols
                if sym.file_path.contains("test") || sym.file_path.contains("spec") {
                    continue;
                }
                if !called_uids.contains(sym.uid.as_str()) {
                    continue; // Already dead code, handled by find_dead_code
                }

                // Check if any caller is from a test file
                let has_test_caller = callers_of
                    .get(sym.uid.as_str())
                    .map(|callers| {
                        callers.iter().any(|caller_uid| {
                            uid_to_sym
                                .get(caller_uid)
                                .map(|s| {
                                    s.file_path.contains("test") || s.file_path.contains("spec")
                                })
                                .unwrap_or(false)
                        })
                    })
                    .unwrap_or(false);

                if !has_test_caller {
                    let caller_count = callers_of
                        .get(sym.uid.as_str())
                        .map(|c| c.len())
                        .unwrap_or(0);
                    let severity = if caller_count > 5 {
                        "high"
                    } else if caller_count > 2 {
                        "medium"
                    } else {
                        "low"
                    };
                    gaps.push(KnowledgeGapItem {
                        category: "untested".to_string(),
                        severity: severity.to_string(),
                        symbol_name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        start_line: sym.start_line,
                        description: format!(
                            "No test callers found ({} non-test callers)",
                            caller_count
                        ),
                        suggestion: "Add unit tests covering this symbol".to_string(),
                    });
                }
            }
        }

        // --- 2. Isolated modules: communities with very few external connections ---
        if category_filter.is_none() || category_filter == Some("isolated") {
            // Build community membership from top_symbols
            let mut sym_to_community: HashMap<&str, &str> = HashMap::new();
            for community in &communities {
                for name in community.top_symbols.split(',').map(|s| s.trim()) {
                    if !name.is_empty() {
                        sym_to_community.insert(name, community.label.as_str());
                    }
                }
            }

            for community in &communities {
                if community.member_count < 2 {
                    continue;
                }
                let member_names: std::collections::HashSet<&str> = community
                    .top_symbols
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();

                // Count external edges (calls crossing community boundary)
                let member_uids: std::collections::HashSet<&str> = symbols
                    .iter()
                    .filter(|s| member_names.contains(s.name.as_str()))
                    .map(|s| s.uid.as_str())
                    .collect();

                let external_edge_count = relationships
                    .iter()
                    .filter(|r| r.kind == RelationshipKind::Calls)
                    .filter(|r| {
                        let src_in = member_uids.contains(r.source_uid.as_str());
                        let tgt_in = member_uids.contains(r.target_uid.as_str());
                        (src_in || tgt_in) && !(src_in && tgt_in)
                    })
                    .count();

                if external_edge_count <= 1 {
                    let severity = if external_edge_count == 0 {
                        "high"
                    } else {
                        "medium"
                    };
                    // Report on the community's first top symbol
                    if let Some(first_sym) = symbols
                        .iter()
                        .find(|s| member_names.contains(s.name.as_str()))
                    {
                        gaps.push(KnowledgeGapItem {
                            category: "isolated".to_string(),
                            severity: severity.to_string(),
                            symbol_name: community.label.clone(),
                            qualified_name: format!("community:{}", community.uid),
                            kind: "Community".to_string(),
                            file_path: first_sym.file_path.clone(),
                            start_line: first_sym.start_line,
                            description: format!(
                                "Community '{}' has only {} external connection(s) ({} members)",
                                community.label, external_edge_count, community.member_count
                            ),
                            suggestion: "Review whether this module should integrate with the rest of the codebase or be extracted as a standalone package".to_string(),
                        });
                    }
                }
            }
        }

        // --- 3. Documentation gaps: files with many symbols but no Rationale/Document nodes ---
        if category_filter.is_none() || category_filter == Some("undocumented") {
            // Group symbols by file
            let mut file_symbol_count: HashMap<&str, u32> = HashMap::new();
            let mut file_has_docs: std::collections::HashSet<&str> =
                std::collections::HashSet::new();

            for sym in &symbols {
                if matches!(
                    sym.kind,
                    myceliums_storage::SymbolKind::Rationale
                        | myceliums_storage::SymbolKind::Document
                        | myceliums_storage::SymbolKind::Section
                ) {
                    file_has_docs.insert(sym.file_path.as_str());
                } else if matches!(
                    sym.kind,
                    myceliums_storage::SymbolKind::Function
                        | myceliums_storage::SymbolKind::Method
                        | myceliums_storage::SymbolKind::Class
                        | myceliums_storage::SymbolKind::Interface
                ) {
                    *file_symbol_count.entry(sym.file_path.as_str()).or_insert(0) += 1;
                }
            }

            let threshold = 5u32; // Files with >= 5 code symbols and no docs
            for (file_path, count) in &file_symbol_count {
                if *count >= threshold && !file_has_docs.contains(file_path) {
                    let severity = if *count > 15 {
                        "high"
                    } else if *count > 8 {
                        "medium"
                    } else {
                        "low"
                    };
                    gaps.push(KnowledgeGapItem {
                        category: "undocumented".to_string(),
                        severity: severity.to_string(),
                        symbol_name: file_path.to_string(),
                        qualified_name: file_path.to_string(),
                        kind: "File".to_string(),
                        file_path: file_path.to_string(),
                        start_line: 1,
                        description: format!(
                            "{} symbols with no rationale or documentation nodes",
                            count
                        ),
                        suggestion: "Add inline rationale comments (NOTE:, WHY:, HACK:) or documentation to explain design decisions".to_string(),
                    });
                }
            }
        }

        // --- 4. Single points of failure: symbols that are the only bridge between communities ---
        if category_filter.is_none() || category_filter == Some("single_point_of_failure") {
            // Build community membership by UID
            let mut uid_to_community: HashMap<&str, &str> = HashMap::new();
            for community in &communities {
                let member_names: std::collections::HashSet<&str> = community
                    .top_symbols
                    .split(',')
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .collect();
                for sym in &symbols {
                    if member_names.contains(sym.name.as_str()) {
                        uid_to_community.insert(sym.uid.as_str(), community.label.as_str());
                    }
                }
            }

            // Find symbols that bridge two communities with only one edge between them
            // Count cross-community edges per community pair
            let mut community_pair_bridges: HashMap<(String, String), Vec<&str>> = HashMap::new();

            for rel in &relationships {
                if rel.kind != RelationshipKind::Calls {
                    continue;
                }
                let src_comm = uid_to_community.get(rel.source_uid.as_str());
                let tgt_comm = uid_to_community.get(rel.target_uid.as_str());
                if let (Some(src_c), Some(tgt_c)) = (src_comm, tgt_comm) {
                    if src_c != tgt_c {
                        let pair = if *src_c < *tgt_c {
                            (src_c.to_string(), tgt_c.to_string())
                        } else {
                            (tgt_c.to_string(), src_c.to_string())
                        };
                        community_pair_bridges
                            .entry(pair)
                            .or_default()
                            .push(rel.source_uid.as_str());
                    }
                }
            }

            // Symbols that are the sole bridge between two communities
            for (pair, bridge_uids) in &community_pair_bridges {
                if bridge_uids.len() == 1 {
                    if let Some(sym) = uid_to_sym.get(bridge_uids[0]) {
                        gaps.push(KnowledgeGapItem {
                            category: "single_point_of_failure".to_string(),
                            severity: "high".to_string(),
                            symbol_name: sym.name.clone(),
                            qualified_name: sym.qualified_name.clone(),
                            kind: sym.kind.to_string(),
                            file_path: sym.file_path.clone(),
                            start_line: sym.start_line,
                            description: format!(
                                "Only bridge between communities '{}' and '{}'",
                                pair.0, pair.1
                            ),
                            suggestion: "Consider adding redundant paths or abstracting the interface to reduce single-point-of-failure risk".to_string(),
                        });
                    }
                }
            }
        }

        // Sort by severity (high first)
        gaps.sort_by(|a, b| {
            let severity_order = |s: &str| -> u8 {
                match s {
                    "high" => 0,
                    "medium" => 1,
                    "low" => 2,
                    _ => 3,
                }
            };
            severity_order(&a.severity)
                .cmp(&severity_order(&b.severity))
                .then_with(|| a.category.cmp(&b.category))
        });

        let summary = KnowledgeGapSummary {
            untested_count: gaps.iter().filter(|g| g.category == "untested").count() as u32,
            isolated_count: gaps.iter().filter(|g| g.category == "isolated").count() as u32,
            undocumented_count: gaps.iter().filter(|g| g.category == "undocumented").count() as u32,
            single_point_of_failure_count: gaps
                .iter()
                .filter(|g| g.category == "single_point_of_failure")
                .count() as u32,
        };
        let total_count = gaps.len() as u32;

        let output = KnowledgeGapsOutput {
            gaps,
            total_count,
            summary,
        };
        Ok(Json(TextOutput {
            text: format::format_knowledge_gaps(&output),
        }))
    }

    #[tool(
        name = "get_stats",
        description = "Get codebase statistics: symbol counts by kind, files, relationships, languages, and communities. Use to understand overall codebase structure."
    )]
    async fn get_stats(
        &self,
        Parameters(params): Parameters<GetStatsParams>,
    ) -> Result<Json<StatsOutput>, rmcp::ErrorData> {
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let files = store
            .get_files()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let processes = store
            .get_processes()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Count symbols by kind
        let mut symbol_counts = std::collections::HashMap::new();
        for symbol in &symbols {
            *symbol_counts.entry(symbol.kind.to_string()).or_insert(0u32) += 1;
        }

        // Count relationships by kind
        let mut relationship_counts = std::collections::HashMap::new();
        for rel in &relationships {
            *relationship_counts
                .entry(rel.kind.to_string())
                .or_insert(0u32) += 1;
        }

        // Count files by language
        let mut language_counts = std::collections::HashMap::new();
        for file in &files {
            *language_counts.entry(file.language.clone()).or_insert(0u32) += 1;
        }

        Ok(Json(StatsOutput {
            symbol_counts,
            total_symbols: symbols.len() as u32,
            total_files: files.len() as u32,
            total_relationships: relationships.len() as u32,
            relationship_counts,
            language_counts,
            community_count: communities.len() as u32,
            process_count: processes.len() as u32,
        }))
    }

    #[tool(
        name = "get_suggested_questions",
        description = "Auto-generate contextual code review questions based on code graph structure and git diff. Returns ranked questions about potential issues like missing test coverage, high caller counts, or API contract violations."
    )]
    async fn get_suggested_questions(
        &self,
        Parameters(params): Parameters<GetSuggestedQuestionsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let files = store
            .get_files()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diff_text = match params.diff {
            Some(d) => d,
            None => {
                let registry = RepoRegistry::load(&registry_path())
                    .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                let repo_info = registry.get(&repo_id).ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Repository not found: {}", repo_id),
                        None,
                    )
                })?;
                run_git_diff(&repo_info.path).map_err(|e| {
                    rmcp::ErrorData::internal_error(format!("git diff failed: {}", e), None)
                })?
            }
        };

        let limit = params.limit.unwrap_or(5);
        let questions =
            generate_suggested_questions(&diff_text, &symbols, &relationships, &files, limit);

        Ok(Json(TextOutput {
            text: format::format_suggested_questions(&questions),
        }))
    }

    #[tool(
        name = "get_graph_diff",
        description = "Compare the current knowledge graph against the last stored snapshot to detect architectural drift. Shows new/removed symbols and relationships since the last analysis that saved a snapshot."
    )]
    async fn get_graph_diff(
        &self,
        Parameters(params): Parameters<GetGraphDiffParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);

        // Load previous snapshot
        let previous = load_snapshot(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let previous = match previous {
            Some(s) => s,
            None => {
                return Ok(Json(TextOutput {
                    text: format!(
                        "No previous snapshot found for '{}'. Run `analyze` first to create a baseline.",
                        repo_id
                    ),
                }));
            }
        };

        // Build current snapshot from graph
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let current = build_snapshot(&repo_id, &symbols, &relationships);

        let diff = diff_snapshots(&previous, &current);

        let output = GraphDiffOutput {
            repo_id: diff.repo_id,
            previous_snapshot_at: diff.previous_snapshot_at,
            current_snapshot_at: diff.current_snapshot_at,
            added_symbols: diff
                .added_symbols
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
            removed_symbols: diff
                .removed_symbols
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
            added_edges: diff
                .added_edges
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
            removed_edges: diff
                .removed_edges
                .into_iter()
                .map(|e| GraphDiffEntry {
                    uid: e.uid,
                    label: e.label,
                })
                .collect(),
        };

        Ok(Json(TextOutput {
            text: format::format_graph_diff(&output),
        }))
    }

    #[tool(
        name = "search_emails",
        description = "Search indexed emails by keyword, with optional person and date filters. Returns matching Email symbols with subject, sender, date, and body snippet."
    )]
    async fn search_emails(
        &self,
        Parameters(params): Parameters<SearchEmailsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let limit = params.limit.unwrap_or(20);
        let query_lower = params.query.to_lowercase();

        // Filter email symbols by keyword
        let mut results: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| s.kind == myceliums_storage::SymbolKind::Email)
            .filter(|s| {
                s.name.to_lowercase().contains(&query_lower)
                    || s.content.to_lowercase().contains(&query_lower)
            })
            .collect();

        // Apply person filter: check if email has SentBy or ReceivedBy relationship to a matching person
        if let Some(ref person_filter) = params.person {
            let person_lower = person_filter.to_lowercase();
            let person_uids: std::collections::HashSet<&str> = symbols
                .iter()
                .filter(|s| {
                    s.kind == myceliums_storage::SymbolKind::Person
                        && (s.signature.to_lowercase().contains(&person_lower)
                            || s.name.to_lowercase().contains(&person_lower))
                })
                .map(|s| s.uid.as_str())
                .collect();

            let email_uids_for_person: std::collections::HashSet<&str> = relationships
                .iter()
                .filter(|r| {
                    (r.kind == myceliums_storage::RelationshipKind::SentBy
                        || r.kind == myceliums_storage::RelationshipKind::ReceivedBy)
                        && person_uids.contains(r.target_uid.as_str())
                })
                .map(|r| r.source_uid.as_str())
                .collect();

            results.retain(|s| email_uids_for_person.contains(s.uid.as_str()));
        }

        // Apply date filter (matches on signature which contains the date)
        if let Some(ref date_filter) = params.date {
            results.retain(|s| s.signature.contains(date_filter));
        }

        results.truncate(limit);

        if results.is_empty() {
            return Ok(Json(TextOutput {
                text: format!("No emails found matching '{}'.", params.query),
            }));
        }

        let mut output = format!(
            "Found {} email(s) matching '{}':\n\n",
            results.len(),
            params.query
        );
        for s in &results {
            let snippet = if s.content.len() > 200 {
                format!("{}...", &s.content[..200])
            } else {
                s.content.clone()
            };
            output.push_str(&format!(
                "- **{}**\n  {}\n  Body: {}\n  UID: {}\n\n",
                s.name, s.signature, snippet, s.uid
            ));
        }

        Ok(Json(TextOutput { text: output }))
    }

    #[tool(
        name = "get_conversation",
        description = "Get a full email thread by conversation symbol UID. Returns all emails in the thread with their relationships."
    )]
    async fn get_conversation(
        &self,
        Parameters(params): Parameters<GetConversationParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Find the conversation symbol
        let conv = symbols
            .iter()
            .find(|s| {
                s.uid == params.conversation_uid
                    && s.kind == myceliums_storage::SymbolKind::Conversation
            })
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    format!("Conversation not found: {}", params.conversation_uid),
                    None,
                )
            })?;

        // Find all emails in this conversation via PartOfConversation relationships
        let email_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| {
                r.kind == myceliums_storage::RelationshipKind::PartOfConversation
                    && r.target_uid == conv.uid
            })
            .map(|r| r.source_uid.as_str())
            .collect();

        let email_uid_set: std::collections::HashSet<&str> = email_uids.iter().copied().collect();

        let emails: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| email_uid_set.contains(s.uid.as_str()))
            .collect();

        let mut output = format!(
            "Conversation: {}\n{}\n{} emails in thread:\n\n",
            conv.name,
            conv.signature,
            emails.len()
        );

        for email in &emails {
            // Find sender
            let sender: Option<&str> = relationships
                .iter()
                .find(|r| {
                    r.source_uid == email.uid
                        && r.kind == myceliums_storage::RelationshipKind::SentBy
                })
                .and_then(|r| symbols.iter().find(|s| s.uid == r.target_uid))
                .map(|s| s.name.as_str());

            let snippet = if email.content.len() > 300 {
                format!("{}...", &email.content[..300])
            } else {
                email.content.clone()
            };

            output.push_str(&format!(
                "---\nFrom: {}\nSubject: {}\n{}\n\n{}\n\n",
                sender.unwrap_or("unknown"),
                email.name,
                email.signature,
                snippet,
            ));
        }

        Ok(Json(TextOutput { text: output }))
    }

    #[tool(
        name = "get_person_context",
        description = "Get all emails involving a specific person (sent by or received by). Returns the person's email activity summary."
    )]
    async fn get_person_context(
        &self,
        Parameters(params): Parameters<GetPersonContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let limit = params.limit.unwrap_or(50);
        let person_lower = params.person.to_lowercase();

        // Find matching person symbols
        let person_syms: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| {
                s.kind == myceliums_storage::SymbolKind::Person
                    && (s.signature.to_lowercase().contains(&person_lower)
                        || s.name.to_lowercase().contains(&person_lower))
            })
            .collect();

        if person_syms.is_empty() {
            return Ok(Json(TextOutput {
                text: format!("No person found matching '{}'.", params.person),
            }));
        }

        let person_uid_set: std::collections::HashSet<&str> =
            person_syms.iter().map(|s| s.uid.as_str()).collect();

        // Find all emails linked to these persons
        let sent_email_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| {
                r.kind == myceliums_storage::RelationshipKind::SentBy
                    && person_uid_set.contains(r.target_uid.as_str())
            })
            .map(|r| r.source_uid.as_str())
            .collect();

        let received_email_uids: Vec<&str> = relationships
            .iter()
            .filter(|r| {
                r.kind == myceliums_storage::RelationshipKind::ReceivedBy
                    && person_uid_set.contains(r.target_uid.as_str())
            })
            .map(|r| r.source_uid.as_str())
            .collect();

        let mut all_email_uids: std::collections::HashSet<&str> = std::collections::HashSet::new();
        all_email_uids.extend(sent_email_uids.iter());
        all_email_uids.extend(received_email_uids.iter());

        let mut emails: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| all_email_uids.contains(s.uid.as_str()))
            .collect();
        emails.truncate(limit);

        let person_name = person_syms
            .first()
            .map(|s| s.name.as_str())
            .unwrap_or(&params.person);
        let person_email = person_syms
            .first()
            .map(|s| s.signature.as_str())
            .unwrap_or("");

        let mut output = format!(
            "Person: {} ({})\nSent: {} emails, Received: {} emails\nShowing {} of {} total:\n\n",
            person_name,
            person_email,
            sent_email_uids.len(),
            received_email_uids.len(),
            emails.len(),
            all_email_uids.len(),
        );

        for email in &emails {
            let role = if sent_email_uids.contains(&email.uid.as_str()) {
                "sent"
            } else {
                "received"
            };
            output.push_str(&format!(
                "- [{}] **{}**\n  {}\n  UID: {}\n\n",
                role, email.name, email.signature, email.uid,
            ));
        }

        Ok(Json(TextOutput { text: output }))
    }

    // ── Cross-repo comparison tools (premium) ────────────────────────────

    #[tool(
        name = "get_git_context",
        description = "Get git ownership and history metadata for a symbol. Returns last author, modification date, commit count, and age in days."
    )]
    async fn get_git_context(
        &self,
        Parameters(params): Parameters<GetGitContextParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(Some(&params.repo_id))?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbol_lower = params.symbol_name.to_lowercase();
        let matching_symbols: Vec<&myceliums_storage::CodeSymbol> = symbols
            .iter()
            .filter(|s| {
                s.name.to_lowercase().contains(&symbol_lower)
                    || s.qualified_name.to_lowercase().contains(&symbol_lower)
            })
            .collect();

        if matching_symbols.is_empty() {
            return Ok(Json(TextOutput {
                text: format!("No symbol found matching '{}'", params.symbol_name),
            }));
        }

        let mut output = String::new();
        for (idx, symbol) in matching_symbols.iter().enumerate() {
            if idx > 0 {
                output.push_str("\n---\n");
            }
            output.push_str(&format!("Symbol: {} ({})\n", symbol.name, symbol.kind));
            output.push_str(&format!("Qualified Name: {}\n", symbol.qualified_name));
            output.push_str(&format!(
                "Location: {}:{}-{}\n",
                symbol.file_path, symbol.start_line, symbol.end_line
            ));

            // Parse and display git metadata if available
            if let Some(metadata_json) = &symbol.metadata {
                if let Ok(metadata) =
                    serde_json::from_str::<myceliums_storage::SymbolMetadata>(metadata_json)
                {
                    if let Some(git_meta) = metadata.git {
                        output.push_str("\nGit Metadata:\n");
                        output.push_str(&format!("  Last Author: {}\n", git_meta.last_author));
                        output.push_str(&format!("  Last Modified: {}\n", git_meta.last_modified));
                        output
                            .push_str(&format!("  Commits Touching: {}\n", git_meta.commit_count));
                        output.push_str(&format!("  Age (days): {}\n", git_meta.age_days));
                        if let Some(hash) = git_meta.last_commit_hash {
                            output.push_str(&format!("  Last Commit: {}\n", hash));
                        }
                    } else {
                        output.push_str("Git Metadata: Not available (symbol may not be in git)\n");
                    }
                }
            }
        }

        Ok(Json(TextOutput { text: output }))
    }

    // ── Cross-repo comparison tools (premium) ────────────────────────────

    #[tool(
        name = "isolate_intent",
        description = "Isolate the symbols implementing a specific intent/feature in a repository. Uses hybrid search to find seed symbols, then expands via call graph traversal with community-aware pruning. Returns the IntentSlice: seed symbols, expanded symbols, internal relationships, and structural metadata."
    )]
    async fn isolate_intent(
        &self,
        Parameters(params): Parameters<IsolateIntentParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        use myceliums_core::cross_repo::{self, IsolateConfig};

        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let repo_name = registry
            .get(&params.repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.repo_id.clone());

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let config = IsolateConfig {
            max_symbols: params.max_symbols.unwrap_or(50),
            expansion_depth: params.depth.unwrap_or(2),
        };

        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let slice = cross_repo::isolate_intent_hybrid(
            &embedder,
            &params.intent,
            &params.repo_id,
            &repo_name,
            &symbols,
            &store,
            &relationships,
            &communities,
            &config,
        )
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format::format_intent_slice(&slice),
        }))
    }

    #[tool(
        name = "differentiate_intent",
        description = "Compare how two repositories implement the same intent/feature. Isolates the relevant symbols in each repo, aligns them via embedding similarity, and reports structural differences. Returns: matched symbol pairs, unmatched symbols, and structural comparison metrics."
    )]
    async fn differentiate_intent(
        &self,
        Parameters(params): Parameters<DifferentiateIntentParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        use myceliums_core::cross_repo::{self, DifferentiateConfig, IsolateConfig};

        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let max_symbols = params.max_symbols.unwrap_or(50);
        let config = IsolateConfig {
            max_symbols,
            expansion_depth: 2,
        };

        // Isolate intent in source repo
        let src_db = RepoRegistry::repo_db_path(&data_dir(), &params.source_repo_id);
        let src_store = Store::open(&src_db, &params.source_repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Source store: {}", e), None))?;
        let src_name = registry
            .get(&params.source_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.source_repo_id.clone());
        let src_symbols = src_store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let src_rels = src_store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let src_communities = src_store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let src_embedder = myceliums_core::embedder_for_index(&src_store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Source isolation: {}", e), None)
            })?;
        let source_slice = cross_repo::isolate_intent_hybrid(
            &src_embedder,
            &params.intent,
            &params.source_repo_id,
            &src_name,
            &src_symbols,
            &src_store,
            &src_rels,
            &src_communities,
            &config,
        )
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("Source isolation: {}", e), None))?;

        // Isolate intent in target repo
        let tgt_db = RepoRegistry::repo_db_path(&data_dir(), &params.target_repo_id);
        let tgt_store = Store::open(&tgt_db, &params.target_repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Target store: {}", e), None))?;
        let tgt_name = registry
            .get(&params.target_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.target_repo_id.clone());
        let tgt_symbols = tgt_store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_rels = tgt_store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_communities = tgt_store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let tgt_embedder = myceliums_core::embedder_for_index(&tgt_store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Target isolation: {}", e), None)
            })?;
        let target_slice = cross_repo::isolate_intent_hybrid(
            &tgt_embedder,
            &params.intent,
            &params.target_repo_id,
            &tgt_name,
            &tgt_symbols,
            &tgt_store,
            &tgt_rels,
            &tgt_communities,
            &config,
        )
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("Target isolation: {}", e), None))?;

        // Get vectors for differentiation
        let src_vectors = src_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_vectors = tgt_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diff_config = DifferentiateConfig {
            similarity_threshold: params.similarity_threshold.unwrap_or(0.65),
        };

        let report = cross_repo::differentiate_with_vectors(
            &source_slice,
            &target_slice,
            &src_vectors,
            &tgt_vectors,
            &diff_config,
        );

        Ok(Json(TextOutput {
            text: format::format_differentiation_report(&report),
        }))
    }

    #[tool(
        name = "plan_adaptation",
        description = "Generate an actionable adaptation plan for migrating one repository's approach to another. Compares intent implementations, then produces ordered steps with dependency tracking, effort estimates, and risk analysis."
    )]
    async fn plan_adaptation(
        &self,
        Parameters(params): Parameters<PlanAdaptationParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        use myceliums_core::cross_repo::{self, DifferentiateConfig, IsolateConfig};

        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let max_symbols = params.max_symbols.unwrap_or(50);
        let config = IsolateConfig {
            max_symbols,
            expansion_depth: 2,
        };

        // Isolate both repos
        let src_db = RepoRegistry::repo_db_path(&data_dir(), &params.source_repo_id);
        let src_store = Store::open(&src_db, &params.source_repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let src_name = registry
            .get(&params.source_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.source_repo_id.clone());
        let src_symbols = src_store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let src_rels = src_store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let src_communities = src_store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let src_embedder = myceliums_core::embedder_for_index(&src_store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let source_slice = cross_repo::isolate_intent_hybrid(
            &src_embedder,
            &params.intent,
            &params.source_repo_id,
            &src_name,
            &src_symbols,
            &src_store,
            &src_rels,
            &src_communities,
            &config,
        )
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let tgt_db = RepoRegistry::repo_db_path(&data_dir(), &params.target_repo_id);
        let tgt_store = Store::open(&tgt_db, &params.target_repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_name = registry
            .get(&params.target_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.target_repo_id.clone());
        let tgt_symbols = tgt_store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_rels = tgt_store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_communities = tgt_store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let tgt_embedder = myceliums_core::embedder_for_index(&tgt_store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let target_slice = cross_repo::isolate_intent_hybrid(
            &tgt_embedder,
            &params.intent,
            &params.target_repo_id,
            &tgt_name,
            &tgt_symbols,
            &tgt_store,
            &tgt_rels,
            &tgt_communities,
            &config,
        )
        .await
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Differentiate
        let src_vectors = src_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_vectors = tgt_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diff_config = DifferentiateConfig {
            similarity_threshold: 0.65,
        };

        let report = cross_repo::differentiate_with_vectors(
            &source_slice,
            &target_slice,
            &src_vectors,
            &tgt_vectors,
            &diff_config,
        );

        // Generate adaptation plan
        let direction = params.direction.as_deref().unwrap_or("source_to_target");
        let plan = cross_repo::plan_adaptation(&report, direction);

        Ok(Json(TextOutput {
            text: format::format_adaptation_plan(&plan),
        }))
    }

    #[tool(
        name = "get_schema",
        description = "Get property definitions and schema information for entity types or edge types in the ontology. Returns detailed information about what properties are expected for each specified entity type."
    )]
    async fn get_schema(
        &self,
        Parameters(params): Parameters<GetSchemaParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        // Load the ontology from the config directory
        let ontology_dir = std::path::PathBuf::from("config/ontology");
        let ontology = Ontology::load_from_directory(&ontology_dir).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to load ontology: {}", e), None)
        })?;

        let mut output = String::new();
        let include_edges = params.include_edges.unwrap_or(false);

        // Parse the entity types from the comma-separated input
        let entity_types: Vec<&str> = params.entity_types.split(',').map(|s| s.trim()).collect();

        output.push_str("# Ontology Schema\n\n");

        // Display entity schemas
        output.push_str("## Entity Types\n\n");
        for entity_type in &entity_types {
            match ontology.get_entity(entity_type) {
                Some(entity) => {
                    output.push_str(&format!("### {}\n", entity.name));
                    output.push_str(&format!("**Description**: {}\n\n", entity.description));

                    if !entity.tags.is_empty() {
                        output.push_str(&format!("**Tags**: {}\n\n", entity.tags.join(", ")));
                    }

                    output.push_str("**Properties**:\n\n");
                    output.push_str("| Name | Type | Required | Description |\n");
                    output.push_str("|------|------|----------|-------------|\n");

                    for prop in &entity.properties {
                        let required = if prop.required { "Yes" } else { "No" };
                        let desc = prop.description.as_deref().unwrap_or("-");
                        output.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            prop.name, prop.prop_type, required, desc
                        ));
                    }
                    output.push('\n');
                }
                None => {
                    output.push_str(&format!("❌ Unknown entity type: {}\n\n", entity_type));
                }
            }
        }

        // Display edge schemas if requested
        if include_edges {
            output.push_str("## Edge Types\n\n");
            for edge in ontology.get_edges().values() {
                output.push_str(&format!("### {}\n", edge.name));
                output.push_str(&format!("**Description**: {}\n\n", edge.description));
                output.push_str(&format!("**From Types**: {}\n", edge.from_types.join(", ")));
                output.push_str(&format!("**To Types**: {}\n\n", edge.to_types.join(", ")));

                if !edge.tags.is_empty() {
                    output.push_str(&format!("**Tags**: {}\n\n", edge.tags.join(", ")));
                }

                if !edge.properties.is_empty() {
                    output.push_str("**Properties**:\n\n");
                    output.push_str("| Name | Type | Required | Description |\n");
                    output.push_str("|------|------|----------|-------------|\n");

                    for prop in &edge.properties {
                        let required = if prop.required { "Yes" } else { "No" };
                        let desc = prop.description.as_deref().unwrap_or("-");
                        output.push_str(&format!(
                            "| {} | {} | {} | {} |\n",
                            prop.name, prop.prop_type, required, desc
                        ));
                    }
                    output.push('\n');
                }
            }
        }

        Ok(Json(TextOutput { text: output }))
    }

    #[tool(
        name = "get_centrality_report",
        description = "Compute centrality metrics (degree, betweenness, closeness, eigenvector) for all symbols in the call graph. Returns the top-N symbols ranked by the chosen metric. Betweenness identifies bridge/bottleneck symbols, closeness measures how central a symbol is, eigenvector highlights symbols connected to other important symbols."
    )]
    async fn get_centrality_report(
        &self,
        Parameters(params): Parameters<GetCentralityParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let centrality = compute_centrality(&relationships)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let metric = params.metric.as_deref().unwrap_or("betweenness");
        let top_n = params.top_n.unwrap_or(15);

        let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        let mut nodes: Vec<CentralityNodeOutput> = centrality
            .values()
            .filter_map(|c| {
                uid_to_symbol
                    .get(c.uid.as_str())
                    .map(|sym| CentralityNodeOutput {
                        name: sym.name.clone(),
                        qualified_name: sym.qualified_name.clone(),
                        kind: sym.kind.to_string(),
                        file_path: sym.file_path.clone(),
                        degree: c.degree,
                        betweenness: c.betweenness,
                        closeness: c.closeness,
                        eigenvector: c.eigenvector,
                    })
            })
            .collect();

        // Sort by chosen metric
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

        let output = CentralityOutput {
            nodes,
            total_nodes,
            metric: metric.to_string(),
        };
        Ok(Json(TextOutput {
            text: format::format_centrality(&output),
        }))
    }

    #[tool(
        name = "get_community_metrics",
        description = "Compute quality metrics for the Leiden community partitioning: overall modularity score (higher = better separation), per-community cohesion (internal edge density), and inter-community coupling (edge counts between community pairs). Use to assess code architecture quality."
    )]
    async fn get_community_metrics(
        &self,
        Parameters(params): Parameters<GetCommunityMetricsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let metrics = compute_community_metrics(&symbols, &relationships)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let mut cohesion: Vec<CohesionEntry> = metrics
            .cohesion
            .into_iter()
            .map(|(community, density)| CohesionEntry { community, density })
            .collect();
        cohesion.sort_by(|a, b| {
            b.density
                .partial_cmp(&a.density)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let coupling: Vec<CouplingEntry> = metrics
            .coupling
            .into_iter()
            .map(|c| CouplingEntry {
                community_a: c.community_a,
                community_b: c.community_b,
                edge_count: c.edge_count,
            })
            .collect();

        let output = CommunityMetricsOutput {
            modularity: metrics.modularity,
            community_count: metrics.community_count,
            cohesion,
            coupling,
        };
        Ok(Json(TextOutput {
            text: format::format_community_metrics(&output),
        }))
    }

    #[tool(
        name = "detect_circular_dependencies",
        description = "Detect circular dependencies in the codebase using Tarjan's strongly connected components algorithm. Returns groups of symbols that form dependency cycles. Use to find architectural issues like mutual imports or call cycles."
    )]
    async fn detect_circular_dependencies(
        &self,
        Parameters(params): Parameters<DetectCyclesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let include_calls = params.include_calls.unwrap_or(true);
        let include_imports = params.include_imports.unwrap_or(true);
        let min_cycle_size = params.min_cycle_size.unwrap_or(2);

        let cycles = detect_cycles(
            &symbols,
            &relationships,
            include_calls,
            include_imports,
            min_cycle_size,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let total_count = cycles.len();
        let cycle_items: Vec<CycleItemOutput> = cycles
            .into_iter()
            .map(|c| CycleItemOutput {
                members: c.member_names,
                size: c.size,
                files: c.files,
            })
            .collect();

        let output = CyclesOutput {
            cycles: cycle_items,
            total_count,
        };
        Ok(Json(TextOutput {
            text: format::format_cycles(&output),
        }))
    }

    #[tool(
        name = "get_dependencies",
        description = "Compute file-level dependencies: direct imports, transitive closure (all files reachable via import chains), and reverse dependents (files that import this file). Use before refactoring to understand the full impact of moving or deleting a file."
    )]
    async fn get_dependencies(
        &self,
        Parameters(params): Parameters<GetDependenciesParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let dep = compute_file_dependencies(
            &symbols,
            &relationships,
            &params.file_path,
            params.max_depth,
        );

        let output = DependenciesOutput {
            file_path: dep.file_path,
            direct_deps: dep.direct_deps,
            transitive_deps: dep.transitive_deps,
            dependents: dep.dependents,
        };
        Ok(Json(TextOutput {
            text: format::format_dependencies(&output),
        }))
    }

    #[tool(
        name = "get_module_coupling",
        description = "Compute module-level coupling metrics (afferent Ca, efferent Ce, instability I) for all files or directories. Instability ranges from 0 (maximally stable, many dependents) to 1 (maximally unstable, depends on many others). Use to find fragile modules."
    )]
    async fn get_module_coupling(
        &self,
        Parameters(params): Parameters<GetModuleCouplingParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let group_by_dir = params.group_by_directory.unwrap_or(false);
        let mut coupling = compute_module_coupling(&symbols, &relationships, group_by_dir);

        let total_count = coupling.len();
        let limit = params.limit.unwrap_or(30);
        coupling.truncate(limit);

        let modules: Vec<ModuleCouplingEntry> = coupling
            .into_iter()
            .map(|c| ModuleCouplingEntry {
                module_path: c.module_path,
                afferent: c.afferent,
                efferent: c.efferent,
                instability: c.instability,
            })
            .collect();

        let output = ModuleCouplingOutput {
            modules,
            total_count,
        };
        Ok(Json(TextOutput {
            text: format::format_module_coupling(&output),
        }))
    }

    #[tool(
        name = "export_mermaid",
        description = "Export the knowledge graph as a Mermaid diagram. Supports flowchart (call graph), class (class hierarchy), and graph (community-grouped) views. Returns a Mermaid-syntax string that can be rendered by any Mermaid-compatible tool."
    )]
    async fn export_mermaid_tool(
        &self,
        Parameters(params): Parameters<ExportMermaidParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diagram_type = match params.diagram_type.as_deref() {
            Some("class") => MermaidDiagramType::ClassDiagram,
            Some("graph") => MermaidDiagramType::Graph,
            _ => MermaidDiagramType::Flowchart,
        };

        let mermaid = export_mermaid(&symbols, &relationships, diagram_type);

        Ok(Json(TextOutput { text: mermaid }))
    }

    #[tool(
        name = "quality_hotspots",
        description = "Identify refactoring hotspots by combining graph centrality, git churn, and module instability into a composite score. High-scoring symbols are architecturally critical AND frequently changed — prime candidates for refactoring."
    )]
    async fn quality_hotspots(
        &self,
        Parameters(params): Parameters<QualityHotspotsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let top_n = params.top_n.unwrap_or(20);
        let hotspots = compute_hotspot_scores(&symbols, &relationships, top_n);

        Ok(Json(TextOutput {
            text: format::format_hotspots(&hotspots),
        }))
    }

    #[tool(
        name = "architecture_lint",
        description = "Run architecture quality checks: circular dependencies, god nodes, high fan-out, unstable dependencies. Returns findings with severity levels and affected entities."
    )]
    async fn architecture_lint(
        &self,
        Parameters(params): Parameters<ArchitectureLintParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let rules: Option<Vec<&str>> = params
            .rules
            .as_ref()
            .map(|r| r.split(',').map(|s| s.trim()).collect());
        let threshold = params.god_node_threshold.unwrap_or(20);

        let report = lint_architecture(&symbols, &relationships, rules.as_deref(), threshold);

        Ok(Json(TextOutput {
            text: format::format_lint_results(&report),
        }))
    }

    #[tool(
        name = "architecture_view",
        description = "Generate a service-level architecture diagram from the knowledge graph. Communities become service nodes, cross-community edges become connections. Returns both structured JSON and a Mermaid diagram string."
    )]
    async fn architecture_view(
        &self,
        Parameters(params): Parameters<ArchitectureViewParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diagram = generate_architecture_diagram(&symbols, &relationships, &communities);

        Ok(Json(TextOutput {
            text: format::format_architecture_view(&diagram),
        }))
    }

    #[tool(
        name = "detect_architecture_drift",
        description = "Detect architectural drift by comparing the current knowledge graph against the last saved snapshot. Returns a drift score (0-100, higher = less drift) and details on structural changes."
    )]
    async fn detect_architecture_drift(
        &self,
        Parameters(params): Parameters<DetectDriftParams2>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let baseline = load_snapshot(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "No saved snapshot found. Run analysis first to create a baseline.".to_string(),
                    None,
                )
            })?;

        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let communities = store
            .get_communities()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let report = detect_drift(&baseline, &symbols, &relationships, &communities, &[]);

        Ok(Json(TextOutput {
            text: format::format_drift_report(&report),
        }))
    }

    #[tool(
        name = "get_ownership",
        description = "Resolve file ownership from CODEOWNERS rules. Parses .github/CODEOWNERS or CODEOWNERS and matches symbols to their owners."
    )]
    async fn get_ownership(
        &self,
        Parameters(params): Parameters<GetOwnershipParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let repo = registry
            .get(&params.repo_id)
            .ok_or_else(|| rmcp::ErrorData::internal_error("Repo not found".to_string(), None))?;

        let db_path = RepoRegistry::repo_db_path(&data_dir(), &params.repo_id);
        let store = Store::open(&db_path, &params.repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let repo_path = std::path::Path::new(&repo.path);
        let codeowners_content = ["CODEOWNERS", ".github/CODEOWNERS", "docs/CODEOWNERS"]
            .iter()
            .find_map(|p| std::fs::read_to_string(repo_path.join(p)).ok())
            .unwrap_or_default();

        let entries = parse_codeowners(&codeowners_content);
        let report = compute_ownership(&symbols, &entries);

        Ok(Json(TextOutput {
            text: format::format_ownership(&report),
        }))
    }

    #[tool(
        name = "map_service",
        description = "Assign a human-readable service name to a community. Use with architecture_view to create meaningful service labels."
    )]
    async fn map_service(
        &self,
        Parameters(params): Parameters<MapServiceParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        save_service_mapping(
            &data_dir(),
            &params.repo_id,
            &params.community_label,
            &params.service_name,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format!(
                "Mapped community '{}' to service '{}'",
                params.community_label, params.service_name
            ),
        }))
    }

    #[tool(
        name = "get_service_map",
        description = "List all community-to-service name mappings for a repository."
    )]
    async fn get_service_map(
        &self,
        Parameters(params): Parameters<GetServiceMapParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let mapping = load_service_mappings(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        if mapping.mappings.is_empty() {
            return Ok(Json(TextOutput {
                text: "No service mappings configured.".to_string(),
            }));
        }

        let mut out = format!("Service mappings for {}:\n\n", repo_id);
        for entry in &mapping.mappings {
            out.push_str(&format!(
                "  {} → {}\n",
                entry.community_label, entry.service_name
            ));
        }
        Ok(Json(TextOutput { text: out }))
    }

    #[tool(
        name = "record_decision",
        description = "Create an Architecture Decision Record (ADR). Records architectural decisions with context, rationale, and consequences. Link to code symbols with link_decision."
    )]
    async fn record_decision(
        &self,
        Parameters(params): Parameters<RecordDecisionParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let status = match params.status.as_deref() {
            Some("accepted") => AdrStatus::Accepted,
            Some("deprecated") => AdrStatus::Deprecated,
            Some("superseded") => AdrStatus::Superseded,
            _ => AdrStatus::Proposed,
        };

        let now = chrono::Utc::now().to_rfc3339();
        let adr = ArchDecisionRecord {
            id: uuid::Uuid::new_v4().to_string(),
            title: params.title.clone(),
            status,
            context: params.context,
            decision: params.decision,
            consequences: params.consequences.unwrap_or_default(),
            linked_symbols: Vec::new(),
            superseded_by: None,
            created_at: now.clone(),
            updated_at: now,
        };

        save_decision(&data_dir(), &params.repo_id, &adr)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format!("ADR created: {} (id: {})", params.title, adr.id),
        }))
    }

    #[tool(
        name = "get_decisions",
        description = "List Architecture Decision Records (ADRs) for a repository. Optionally filter by status."
    )]
    async fn get_decisions(
        &self,
        Parameters(params): Parameters<GetDecisionsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let decisions = load_decisions(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let filtered: Vec<_> = if let Some(status_filter) = &params.status {
            decisions
                .into_iter()
                .filter(|d| d.status.to_string().to_lowercase() == status_filter.to_lowercase())
                .collect()
        } else {
            decisions
        };

        Ok(Json(TextOutput {
            text: format::format_decisions(&filtered),
        }))
    }

    #[tool(
        name = "link_decision",
        description = "Link an Architecture Decision Record to a code symbol. Creates a traceability connection between the decision and the code it affects."
    )]
    async fn link_decision(
        &self,
        Parameters(params): Parameters<LinkDecisionParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        link_decision_to_symbol(
            &data_dir(),
            &params.repo_id,
            &params.decision_id,
            &params.symbol_name,
        )
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format!(
                "Linked ADR {} to symbol '{}'",
                params.decision_id, params.symbol_name
            ),
        }))
    }

    #[tool(
        name = "get_contracts",
        description = "Detect API contracts (OpenAPI, Protobuf) in the repository and match endpoints to handler symbols. Returns linked and unlinked endpoints."
    )]
    async fn get_contracts(
        &self,
        Parameters(params): Parameters<GetContractsParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let repo = registry
            .get(&repo_id)
            .ok_or_else(|| rmcp::ErrorData::internal_error("Repo not found".to_string(), None))?;

        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let report = detect_contracts(std::path::Path::new(&repo.path), &symbols);

        Ok(Json(TextOutput {
            text: format::format_contracts(&report),
        }))
    }

    #[tool(
        name = "snapshot_diff",
        description = "Compare two graph snapshots to see architectural changes over time. Shows added/removed symbols and relationships. Defaults to comparing the two most recent snapshots."
    )]
    async fn snapshot_diff(
        &self,
        Parameters(params): Parameters<SnapshotDiffParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let snapshots = list_snapshots(&data_dir(), &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        if snapshots.len() < 2 && params.from_snapshot.is_none() {
            return Ok(Json(TextOutput {
                text: "Need at least 2 snapshots to compare. Run analysis multiple times first."
                    .to_string(),
            }));
        }

        let from_snap = if let Some(from_id) = &params.from_snapshot {
            load_snapshot_by_id(&data_dir(), &repo_id, from_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        format!("Snapshot not found: {}", from_id),
                        None,
                    )
                })?
        } else {
            load_snapshot_by_id(&data_dir(), &repo_id, &snapshots[1].snapshot_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        "Could not load older snapshot".to_string(),
                        None,
                    )
                })?
        };

        let to_snap = if let Some(to_id) = &params.to_snapshot {
            load_snapshot_by_id(&data_dir(), &repo_id, to_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(format!("Snapshot not found: {}", to_id), None)
                })?
        } else {
            load_snapshot_by_id(&data_dir(), &repo_id, &snapshots[0].snapshot_id)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        "Could not load latest snapshot".to_string(),
                        None,
                    )
                })?
        };

        let diff = myceliums_core::diff_snapshots(&from_snap, &to_snap);

        let mut out = format!(
            "Snapshot diff: {} → {}\n\n",
            diff.previous_snapshot_at, diff.current_snapshot_at
        );

        if diff.added_symbols.is_empty()
            && diff.removed_symbols.is_empty()
            && diff.added_edges.is_empty()
            && diff.removed_edges.is_empty()
        {
            out.push_str("No changes detected.");
        } else {
            out.push_str(&format!(
                "Changes: +{} symbols, -{} symbols, +{} edges, -{} edges\n\n",
                diff.added_symbols.len(),
                diff.removed_symbols.len(),
                diff.added_edges.len(),
                diff.removed_edges.len(),
            ));
            if !diff.added_symbols.is_empty() {
                out.push_str("Added symbols:\n");
                for s in &diff.added_symbols {
                    out.push_str(&format!("  + {}\n", s.label));
                }
            }
            if !diff.removed_symbols.is_empty() {
                out.push_str("Removed symbols:\n");
                for s in &diff.removed_symbols {
                    out.push_str(&format!("  - {}\n", s.label));
                }
            }
        }

        Ok(Json(TextOutput { text: out }))
    }
}

impl MyceliumsMcp {
    fn resolve_symbol<'a>(
        symbols: &'a [myceliums_storage::CodeSymbol],
        name: &str,
    ) -> Result<&'a myceliums_storage::CodeSymbol, rmcp::ErrorData> {
        // Try qualified_name first
        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == name)
            .collect();
        if matches.len() == 1 {
            return Ok(matches[0]);
        }
        if matches.is_empty() {
            // Fall back to name
            let by_name: Vec<_> = symbols.iter().filter(|s| s.name == name).collect();
            match by_name.len() {
                0 => Err(rmcp::ErrorData::internal_error(
                    format!("Symbol not found: {}", name),
                    None,
                )),
                1 => Ok(by_name[0]),
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    Err(rmcp::ErrorData::internal_error(
                        format!("Ambiguous symbol '{}'. Matches: {}", name, names.join(", ")),
                        None,
                    ))
                }
            }
        } else {
            // Multiple qualified_name matches (unlikely but handle it)
            Ok(matches[0])
        }
    }

    fn resolve_repo_id(&self, repo_id: Option<&str>) -> Result<String, rmcp::ErrorData> {
        if let Some(id) = repo_id {
            return Ok(id.to_string());
        }
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Try to match current working directory to a registered repo
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(abs_cwd) = std::fs::canonicalize(&cwd) {
                let cwd_str = abs_cwd.to_string_lossy();
                for repo in registry.list().iter().rev() {
                    if let Ok(repo_path) = std::fs::canonicalize(&repo.path) {
                        let repo_str = repo_path.to_string_lossy();
                        if cwd_str.starts_with(repo_str.as_ref()) {
                            return Ok(repo.id.clone());
                        }
                    }
                }
            }
        }

        // Fallback: most recently analyzed repo
        let repos = registry.list();
        repos.last().map(|r| r.id.clone()).ok_or_else(|| {
            rmcp::ErrorData::internal_error("No repositories analyzed yet".to_string(), None)
        })
    }
}

// Resource reading implementation
impl MyceliumsMcp {
    async fn read_resource_inner(&self, uri: &str) -> Result<String, rmcp::ErrorData> {
        let mcp_err = |msg: String| rmcp::ErrorData::internal_error(msg, None);

        if uri == "repository://list" {
            let registry =
                RepoRegistry::load(&registry_path()).map_err(|e| mcp_err(format!("{}", e)))?;
            let repos: Vec<serde_json::Value> = registry
                .list()
                .iter()
                .map(|r| {
                    serde_json::json!({
                        "id": r.id,
                        "name": r.name,
                        "path": r.path,
                        "analyzed_at": r.analyzed_at,
                        "symbol_count": r.symbol_count,
                        "file_count": r.file_count,
                    })
                })
                .collect();
            return serde_json::to_string_pretty(&repos).map_err(|e| mcp_err(format!("{}", e)));
        }

        let path = uri
            .strip_prefix("repository://")
            .ok_or_else(|| mcp_err(format!("Unknown resource URI scheme: {}", uri)))?;
        let parts: Vec<&str> = path.splitn(3, '/').collect();
        if parts.is_empty() {
            return Err(mcp_err(format!("Invalid resource URI: {}", uri)));
        }

        let repo_id = parts[0];
        let registry =
            RepoRegistry::load(&registry_path()).map_err(|e| mcp_err(format!("{}", e)))?;
        let _repo_info = registry
            .get(repo_id)
            .ok_or_else(|| mcp_err(format!("Repository not found: {}", repo_id)))?;
        let db_path = RepoRegistry::repo_db_path(&data_dir(), repo_id);
        let store = Store::open(&db_path, repo_id)
            .await
            .map_err(|e| mcp_err(format!("{}", e)))?;
        let sub_path = if parts.len() > 1 { parts[1] } else { "" };

        match sub_path {
            "schema" => {
                let symbols = store
                    .get_symbols()
                    .await
                    .map_err(|e| mcp_err(format!("{}", e)))?;
                let files = store
                    .get_files()
                    .await
                    .map_err(|e| mcp_err(format!("{}", e)))?;
                let relationships = store
                    .get_relationships()
                    .await
                    .map_err(|e| mcp_err(format!("{}", e)))?;
                let communities = store
                    .get_communities()
                    .await
                    .map_err(|e| mcp_err(format!("{}", e)))?;
                let processes = store
                    .get_processes()
                    .await
                    .map_err(|e| mcp_err(format!("{}", e)))?;
                let mut kind_counts: HashMap<String, usize> = HashMap::new();
                for s in &symbols {
                    *kind_counts.entry(s.kind.to_string()).or_default() += 1;
                }
                let mut rel_counts: HashMap<String, usize> = HashMap::new();
                for r in &relationships {
                    *rel_counts.entry(r.kind.to_string()).or_default() += 1;
                }
                let result = serde_json::json!({
                    "repo_id": repo_id, "symbol_count": symbols.len(), "file_count": files.len(),
                    "relationship_count": relationships.len(), "community_count": communities.len(),
                    "process_count": processes.len(), "symbols_by_kind": kind_counts, "relationships_by_kind": rel_counts,
                });
                serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
            }
            "map" => {
                let communities = store
                    .get_communities()
                    .await
                    .map_err(|e| mcp_err(format!("{}", e)))?;
                let result: Vec<serde_json::Value> = communities.iter().map(|c| {
                    serde_json::json!({"uid": c.uid, "label": c.label, "member_count": c.member_count, "top_symbols": c.top_symbols, "summary": c.summary})
                }).collect();
                serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
            }
            rest => {
                let sub_parts: Vec<&str> = rest.splitn(2, '/').collect();
                match sub_parts.as_slice() {
                    ["community", community_id] => {
                        let communities = store
                            .get_communities()
                            .await
                            .map_err(|e| mcp_err(format!("{}", e)))?;
                        let community = communities
                            .iter()
                            .find(|c| c.uid == *community_id)
                            .ok_or_else(|| {
                                mcp_err(format!("Community not found: {}", community_id))
                            })?;
                        let result = serde_json::json!({"uid": community.uid, "label": community.label, "member_count": community.member_count, "top_symbols": community.top_symbols, "summary": community.summary});
                        serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
                    }
                    ["process", process_id] => {
                        let processes = store
                            .get_processes()
                            .await
                            .map_err(|e| mcp_err(format!("{}", e)))?;
                        let process = processes
                            .iter()
                            .find(|p| p.uid == *process_id)
                            .ok_or_else(|| mcp_err(format!("Process not found: {}", process_id)))?;
                        let result = serde_json::json!({"uid": process.uid, "name": process.name, "entry_point": process.entry_point, "step_count": process.step_count, "description": process.description});
                        serde_json::to_string_pretty(&result).map_err(|e| mcp_err(format!("{}", e)))
                    }
                    _ => Err(mcp_err(format!("Unknown resource path: {}", uri))),
                }
            }
        }
    }
}

impl ServerHandler for MyceliumsMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_instructions(
            "Myceliums is a code knowledge graph that indexes the current codebase.\n\n\
             WHEN TO USE MYCELIUMS (always try first for these tasks):\n\
             - Understanding the project: 'What does this codebase do?' → context_search + get_processes\n\
             - Finding code: 'Where is authentication handled?' → context_search (not grep)\n\
             - Understanding dependencies: 'What calls this function?' → symbol_context\n\
             - Before modifying code: 'What will break if I change X?' → detect_impact\n\
             - Reviewing changes: 'Summarize what this diff affects' → get_review_context\n\
             - Architecture questions: 'How does data flow from API to DB?' → get_processes\n\
             - Structural queries: 'Find all classes that implement Y' → cypher_query\n\n\
             WHEN TO USE grep/read INSTEAD:\n\
             - Reading specific file contents or line ranges\n\
             - Searching inside string literals, comments, or config files\n\
             - Editing or writing files (myceliums is read-only)\n\n\
             RULE: For any question about code structure, symbols, or architecture — use myceliums first. \
             It returns structured results with exact locations, types, and relationships in one call \
             instead of multiple grep+read rounds."
        )
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let mut resources = vec![Annotated::new(
            RawResource::new("repository://list", "Repository List")
                .with_description("List all analyzed repositories")
                .with_mime_type("application/json"),
            None,
        )];
        for repo in registry.list() {
            resources.push(Annotated::new(
                RawResource::new(
                    format!("repository://{}/schema", repo.id),
                    format!("{} - Schema", repo.name),
                )
                .with_description(format!("Schema and stats for {}", repo.name))
                .with_mime_type("application/json"),
                None,
            ));
            resources.push(Annotated::new(
                RawResource::new(
                    format!("repository://{}/map", repo.id),
                    format!("{} - Map", repo.name),
                )
                .with_description(format!("High-level community map for {}", repo.name))
                .with_mime_type("application/json"),
                None,
            ));
        }
        Ok(ListResourcesResult {
            resources,
            ..Default::default()
        })
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, rmcp::ErrorData> {
        let templates = vec![
            Annotated::new(
                RawResourceTemplate::new("repository://{repo_id}/schema", "Repository Schema")
                    .with_description("Schema and statistics for a specific repository")
                    .with_mime_type("application/json"),
                None,
            ),
            Annotated::new(
                RawResourceTemplate::new("repository://{repo_id}/map", "Repository Map")
                    .with_description("High-level community map of a repository")
                    .with_mime_type("application/json"),
                None,
            ),
            Annotated::new(
                RawResourceTemplate::new(
                    "repository://{repo_id}/community/{community_id}",
                    "Community Detail",
                )
                .with_description("Details of a specific community within a repository")
                .with_mime_type("application/json"),
                None,
            ),
            Annotated::new(
                RawResourceTemplate::new(
                    "repository://{repo_id}/process/{process_id}",
                    "Process Detail",
                )
                .with_description("Details of a specific execution process within a repository")
                .with_mime_type("application/json"),
                None,
            ),
        ];
        Ok(ListResourceTemplatesResult {
            resource_templates: templates,
            ..Default::default()
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let text = self.read_resource_inner(&request.uri).await?;
        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            text,
            &request.uri,
        )
        .with_mime_type("application/json")]))
    }
}

// Helper function to check if a symbol is a content symbol (Email, Document, Section)
fn is_content_symbol(kind: &myceliums_storage::SymbolKind) -> bool {
    matches!(
        kind,
        myceliums_storage::SymbolKind::Email
            | myceliums_storage::SymbolKind::Document
            | myceliums_storage::SymbolKind::Section
    )
}

// Helper struct to represent parsed mention metadata
struct MentionMatch {
    line: u32,
    context: String,
    confidence: f64,
}

// Parse mentions metadata JSON string
fn parse_mentions_metadata(metadata: &str) -> Vec<MentionMatch> {
    if metadata.is_empty() {
        return vec![];
    }

    // Try to parse as JSON
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(metadata) {
        if let Some(matches) = json.get("matches").and_then(|m| m.as_array()) {
            return matches
                .iter()
                .filter_map(|m| {
                    let line = m.get("line").and_then(|l| l.as_u64()).unwrap_or(0) as u32;
                    let context = m
                        .get("context")
                        .and_then(|c| c.as_str())
                        .unwrap_or("")
                        .to_string();
                    let method = m.get("method").and_then(|m| m.as_str()).unwrap_or("regex");

                    // Set confidence: 1.0 for regex, variable for llm
                    let confidence = match method {
                        "regex" => 1.0,
                        "llm" => 0.9, // Default LLM confidence
                        _ => 0.8,
                    };

                    if line > 0 && !context.is_empty() {
                        Some(MentionMatch {
                            line,
                            context,
                            confidence,
                        })
                    } else {
                        None
                    }
                })
                .collect();
        }
    }

    // Fallback: return empty list if parsing fails
    vec![]
}

// Helper function to generate suggested review questions from code graph
fn generate_suggested_questions(
    diff_text: &str,
    symbols: &[myceliums_storage::CodeSymbol],
    relationships: &[myceliums_storage::Relationship],
    _files: &[myceliums_storage::FileNode],
    limit: usize,
) -> SuggestedQuestionsOutput {
    let mut questions = Vec::new();
    use myceliums_storage::{RelationshipKind, SymbolKind};
    use std::collections::HashMap;

    // Parse diff to find changed files and functions
    let changed_files: Vec<&str> = diff_text
        .lines()
        .filter_map(|line| {
            if line.starts_with("diff --git a/") {
                Some(line.split(" b/").nth(1).unwrap_or(""))
            } else {
                None
            }
        })
        .collect();

    if changed_files.is_empty() && !diff_text.trim().is_empty() {
        // Fallback: treat entire diff as changes
        // This is a simple heuristic
    }

    // Build a map of symbol names to their call counts
    let mut caller_counts: HashMap<String, usize> = HashMap::new();
    for rel in relationships {
        if rel.kind == RelationshipKind::Calls {
            *caller_counts.entry(rel.target_uid.clone()).or_insert(0) += 1;
        }
    }

    // Check for functions with high caller counts
    for (symbol_uid, count) in &caller_counts {
        if *count > 10 {
            if let Some(symbol) = symbols.iter().find(|s| &s.uid == symbol_uid) {
                questions.push(SuggestedQuestion {
                    question: format!(
                        "This {} has {} callers — have all call sites been updated?",
                        symbol.kind.to_string().to_lowercase(),
                        count
                    ),
                    severity: if *count > 20 { "high" } else { "medium" }.to_string(),
                    category: "callers".to_string(),
                    references: vec![symbol.qualified_name.clone()],
                    rationale: "Functions with many callers have a higher risk of breaking changes. Ensure all dependent code has been tested.".to_string(),
                });
            }
        }
    }

    // Check for symbols with no test coverage (files without "test" or "spec" in their path)
    for symbol in symbols {
        if matches!(symbol.kind, SymbolKind::Function | SymbolKind::Method)
            && !symbol.file_path.contains("test")
            && !symbol.file_path.contains("spec")
        {
            let has_related_test = symbols
                .iter()
                .any(|s| s.file_path.contains("test") || s.file_path.contains("spec"));
            if !has_related_test {
                questions.push(SuggestedQuestion {
                    question: format!(
                        "The file `{}` has no test coverage — should tests be added?",
                        symbol.file_path
                    ),
                    severity: "medium".to_string(),
                    category: "coverage".to_string(),
                    references: vec![symbol.file_path.clone()],
                    rationale: "Adding tests for critical business logic reduces regressions and improves maintainability.".to_string(),
                });
                break; // Only report once per file
            }
        }
    }

    // Check for symbols crossing community boundaries (simplified check)
    // Build symbol-to-file mapping
    let mut symbol_to_file: HashMap<&str, &str> = HashMap::new();
    for symbol in symbols {
        symbol_to_file.insert(&symbol.uid, &symbol.file_path);
    }

    // Check symbols that call across file boundaries
    let mut cross_boundary_symbols = Vec::new();
    for rel in relationships {
        if rel.kind == RelationshipKind::Calls {
            if let (Some(source_file), Some(target_file)) = (
                symbol_to_file.get(rel.source_uid.as_str()),
                symbol_to_file.get(rel.target_uid.as_str()),
            ) {
                let source_parts: Vec<&str> = source_file.split('/').collect();
                let target_parts: Vec<&str> = target_file.split('/').collect();

                // Check if they're in different top-level directories
                if !source_parts.is_empty()
                    && !target_parts.is_empty()
                    && source_parts[0] != target_parts[0]
                {
                    cross_boundary_symbols.push((rel.source_uid.clone(), rel.target_uid.clone()));
                }
            }
        }
    }

    if !cross_boundary_symbols.is_empty() {
        questions.push(SuggestedQuestion {
            question: format!(
                "Found {} calls crossing module/community boundaries — are API contracts maintained?",
                cross_boundary_symbols.len()
            ),
            severity: "medium".to_string(),
            category: "api_contract".to_string(),
            references: cross_boundary_symbols
                .iter()
                .map(|(_, target)| target.clone())
                .collect::<Vec<_>>()
                .into_iter()
                .take(3)
                .collect(),
            rationale: "Cross-boundary calls can indicate coupling issues. Verify API stability and consider public/private contracts.".to_string(),
        });
    }

    // Check for complexity: files with many symbols
    let mut file_symbol_counts: HashMap<&str, usize> = HashMap::new();
    for symbol in symbols {
        *file_symbol_counts.entry(&symbol.file_path).or_insert(0) += 1;
    }

    for (file_path, count) in &file_symbol_counts {
        if *count > 30 {
            questions.push(SuggestedQuestion {
                question: format!(
                    "File `{}` contains {} symbols — consider refactoring or splitting?",
                    file_path, count
                ),
                severity: "low".to_string(),
                category: "complexity".to_string(),
                references: vec![file_path.to_string()],
                rationale: "Large files with many symbols can be harder to understand and maintain. Consider splitting by responsibility.".to_string(),
            });
        }
    }

    // Sort questions by severity (high > medium > low) then by specificity
    questions.sort_by(|a, b| {
        let severity_order = |sev: &str| match sev {
            "high" => 0,
            "medium" => 1,
            _ => 2,
        };
        severity_order(&a.severity).cmp(&severity_order(&b.severity))
    });

    // Limit to requested count
    questions.truncate(limit);

    SuggestedQuestionsOutput { questions }
}

pub async fn run_mcp_server() -> Result<()> {
    info!("Starting Myceliums MCP server");
    let server = MyceliumsMcp::new();

    // Build router with tool routes from the #[tool_router] macro
    let router =
        rmcp::handler::server::router::Router::new(server).with_tools(MyceliumsMcp::tool_router());

    let transport = rmcp::transport::io::stdio();
    let running = router
        .serve(transport)
        .await
        .map_err(|e| anyhow::anyhow!("MCP serve error: {}", e))?;
    running.waiting().await?;
    Ok(())
}

/// Start the MCP server over Streamable HTTP transport.
///
/// This uses rmcp's native `StreamableHttpService` which implements the
/// MCP Streamable HTTP transport specification (SSE-based).
/// Clients connect via `POST /mcp` with JSON-RPC messages.
pub async fn run_mcp_http_server(addr: &str) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
    };
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    use tower_http::cors::{Any, CorsLayer};

    info!("Starting Myceliums MCP HTTP server on {}", addr);

    let ct = CancellationToken::new();

    let mut config = StreamableHttpServerConfig::default();
    config.stateful_mode = true;
    config.json_response = false;
    config.sse_keep_alive = Some(std::time::Duration::from_secs(15));
    config.cancellation_token = ct.child_token();

    let service: StreamableHttpService<
        rmcp::handler::server::router::Router<MyceliumsMcp>,
        LocalSessionManager,
    > = StreamableHttpService::new(
        || {
            let server = MyceliumsMcp::new();
            let router = rmcp::handler::server::router::Router::new(server)
                .with_tools(MyceliumsMcp::tool_router());
            Ok(router)
        },
        Arc::new(LocalSessionManager::default()),
        config,
    );

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = axum::Router::new()
        .nest_service("/mcp", service)
        .layer(cors);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!("MCP server listening on http://{}/mcp", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, SymbolKind};

    #[test]
    fn test_generate_suggested_questions_high_caller_count() {
        let symbols = vec![
            CodeSymbol {
                uid: "sym1".to_string(),
                name: "processPayment".to_string(),
                qualified_name: "payments.processPayment".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/payments.ts".to_string(),
                start_line: 10,
                end_line: 30,
                signature: "function processPayment()".to_string(),
                content: "// process payment".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym2".to_string(),
                name: "validateUser".to_string(),
                qualified_name: "auth.validateUser".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/auth.ts".to_string(),
                start_line: 5,
                end_line: 15,
                signature: "function validateUser()".to_string(),
                content: "// validate user".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
        ];

        // Create relationships: 15 calls to processPayment
        let mut relationships = Vec::new();
        for i in 0..15 {
            relationships.push(Relationship {
                uid: format!("rel{}", i),
                source_uid: format!("caller{}", i),
                target_uid: "sym1".to_string(),
                kind: RelationshipKind::Calls,
                repo_id: "test-repo".to_string(),
                metadata: "".to_string(),
            });
        }

        let files = vec![];
        let diff_text = "diff --git a/src/payments.ts b/src/payments.ts\n+function update(){}";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 5);

        // Should have at least one question about high caller count
        assert!(!output.questions.is_empty());
        let has_caller_question = output
            .questions
            .iter()
            .any(|q| q.category == "callers" && q.question.contains("15 callers"));
        assert!(
            has_caller_question,
            "Expected a question about high caller count"
        );
    }

    #[test]
    fn test_generate_suggested_questions_uncovered_file() {
        let symbols = vec![CodeSymbol {
            uid: "sym1".to_string(),
            name: "calculateTax".to_string(),
            qualified_name: "tax.calculateTax".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/tax.ts".to_string(),
            start_line: 1,
            end_line: 20,
            signature: "function calculateTax()".to_string(),
            content: "// calculate tax".to_string(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        }];

        let relationships = vec![];
        let files = vec![];
        let diff_text = "diff --git a/src/tax.ts b/src/tax.ts\n+new function";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 5);

        // Should have a question about missing test coverage
        let has_coverage_question = output
            .questions
            .iter()
            .any(|q| q.category == "coverage" && q.question.contains("test coverage"));
        assert!(
            has_coverage_question,
            "Expected a question about test coverage"
        );
    }

    #[test]
    fn test_generate_suggested_questions_cross_boundary_calls() {
        let symbols = vec![
            CodeSymbol {
                uid: "sym1".to_string(),
                name: "apiCall".to_string(),
                qualified_name: "api.apiCall".to_string(),
                kind: SymbolKind::Function,
                file_path: "api/handler.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function apiCall()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym2".to_string(),
                name: "dbQuery".to_string(),
                qualified_name: "db.dbQuery".to_string(),
                kind: SymbolKind::Function,
                file_path: "db/connection.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function dbQuery()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
        ];

        // Create cross-boundary call (api -> db)
        let relationships = vec![Relationship {
            uid: "rel1".to_string(),
            source_uid: "sym1".to_string(),
            target_uid: "sym2".to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test-repo".to_string(),
            metadata: "".to_string(),
        }];

        let files = vec![];
        let diff_text = "diff --git a/api/handler.ts b/api/handler.ts";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 5);

        // Should have a question about API contracts
        let has_api_question = output
            .questions
            .iter()
            .any(|q| q.category == "api_contract");
        assert!(has_api_question, "Expected a question about API contracts");
    }

    #[test]
    fn test_generate_suggested_questions_respects_limit() {
        let symbols = vec![
            CodeSymbol {
                uid: "sym1".to_string(),
                name: "func1".to_string(),
                qualified_name: "func1".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/file1.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function func1()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "sym2".to_string(),
                name: "func2".to_string(),
                qualified_name: "func2".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/file2.ts".to_string(),
                start_line: 1,
                end_line: 10,
                signature: "function func2()".to_string(),
                content: "".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            },
        ];

        // Create many relationships to generate multiple questions
        let mut relationships = Vec::new();
        for i in 0..20 {
            relationships.push(Relationship {
                uid: format!("rel{}", i),
                source_uid: format!("caller{}", i),
                target_uid: "sym1".to_string(),
                kind: RelationshipKind::Calls,
                repo_id: "test-repo".to_string(),
                metadata: "".to_string(),
            });
        }

        let files = vec![];
        let diff_text = "";

        let output = generate_suggested_questions(diff_text, &symbols, &relationships, &files, 2);

        // Should respect the limit
        assert!(output.questions.len() <= 2, "Should not exceed limit of 2");
    }

    /// A tempdir index with un-embedded (zero-vector) symbols must produce a
    /// partial-index warning that search responses can surface. Regression
    /// guard for issue #32: partial indexes were invisible at query time.
    #[tokio::test]
    async fn partial_index_warning_surfaces_from_zero_vector_rows() {
        use myceliums_core::EmbeddingStats;
        use myceliums_storage::{CodeSymbol, SymbolKind};

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "test-repo").await.unwrap();

        // Three symbols indexed; each starts life as a zero-vector placeholder.
        let symbols: Vec<CodeSymbol> = (0..3)
            .map(|i| CodeSymbol {
                uid: format!("sym{i}"),
                name: format!("fn{i}"),
                qualified_name: format!("m.fn{i}"),
                kind: SymbolKind::Function,
                file_path: "src/lib.rs".to_string(),
                start_line: 1,
                end_line: 2,
                signature: format!("fn fn{i}()"),
                content: "body".to_string(),
                repo_id: "test-repo".to_string(),
                metadata: None,
            })
            .collect();
        store.store_symbols(&symbols).await.unwrap();

        // Only one of three symbols got a real vector — the other two remain
        // zero-vector placeholders (a deliberately partial index).
        let dim = myceliums_storage::schema::DEFAULT_EMBEDDING_DIM as usize;
        store
            .store_embeddings(vec![("sym0".to_string(), vec![0.5f32; dim])])
            .await
            .unwrap();
        EmbeddingStats {
            symbols_total: 3,
            symbols_embedded: 1,
            embedding_failures: 2,
        }
        .record(&store)
        .await
        .unwrap();

        let warning = partial_index_warning(&store)
            .await
            .expect("partial index must warn");
        assert!(warning.contains("1 of 3 symbols"));

        // The warning is prefixed onto the rendered search body.
        let body = format::with_index_warning(Some(warning), "results table".to_string());
        assert!(body.starts_with("⚠ "));
        assert!(body.contains("results table"));
    }

    /// A fully-embedded tempdir index produces no warning.
    #[tokio::test]
    async fn complete_index_has_no_partial_warning() {
        use myceliums_core::EmbeddingStats;

        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path(), "test-repo").await.unwrap();
        EmbeddingStats::complete(2, 2).record(&store).await.unwrap();

        assert_eq!(partial_index_warning(&store).await, None);
    }
}
