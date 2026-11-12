//! Tool parameter types (the `inputSchema` of each tool).

use rmcp::schemars;
use serde::Deserialize;

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
