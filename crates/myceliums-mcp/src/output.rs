//! Structured tool output types, rendered to text by [`crate::format`].

use rmcp::schemars;
use serde::Serialize;

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
    /// RRF fusion score (rank-based, order of ~1/k). Present for every result.
    pub combined_score: f64,
    /// Cross-encoder rerank score. Only present when reranking ran; a separate
    /// scale from `combined_score` (issue #29).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f64>,
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
