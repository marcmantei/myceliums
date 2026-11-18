//! Tool parameter types (the `inputSchema` of each tool).
//!
//! Conventions: enum-like options are real enums (an unknown value is
//! `invalid_params` before the handler runs); numeric options carry their
//! bounds in the schema, and handlers clamp into the same bounds. Every
//! list-returning tool takes `limit` and `offset` (see `output::Page`).

use rmcp::schemars;
use serde::de::{DeserializeOwned, Deserializer, Error as _};
use serde::Deserialize;

// ── Enum-like options ────────────────────────────────────────────────────

/// Knowledge-gap category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum GapCategory {
    Untested,
    Isolated,
    Undocumented,
    SinglePointOfFailure,
}

impl GapCategory {
    /// The category name used in the output.
    pub fn as_str(self) -> &'static str {
        match self {
            GapCategory::Untested => "untested",
            GapCategory::Isolated => "isolated",
            GapCategory::Undocumented => "undocumented",
            GapCategory::SinglePointOfFailure => "single_point_of_failure",
        }
    }
}

/// Centrality metric to rank by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum CentralityMetric {
    Degree,
    #[default]
    Betweenness,
    Closeness,
    Eigenvector,
}

impl CentralityMetric {
    /// The metric name used in the output.
    pub fn as_str(self) -> &'static str {
        match self {
            CentralityMetric::Degree => "degree",
            CentralityMetric::Betweenness => "betweenness",
            CentralityMetric::Closeness => "closeness",
            CentralityMetric::Eigenvector => "eigenvector",
        }
    }
}

/// Architecture Decision Record status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum DecisionStatus {
    #[default]
    Proposed,
    Accepted,
    Deprecated,
    Superseded,
}

/// Mermaid diagram flavour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum DiagramType {
    #[default]
    Flowchart,
    Class,
    Graph,
}

/// Which way an adaptation plan points.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum AdaptationDirection {
    // Adapt the target to match the source. (Plain comments: doc comments
    // on variants would turn the schema's compact `enum` into a `oneOf`.)
    #[default]
    SourceToTarget,
    // Adapt the source to match the target.
    TargetToSource,
}

impl AdaptationDirection {
    /// The direction name `plan_adaptation` expects.
    pub fn as_str(self) -> &'static str {
        match self {
            AdaptationDirection::SourceToTarget => "source_to_target",
            AdaptationDirection::TargetToSource => "target_to_source",
        }
    }
}

/// Architecture lint rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
#[schemars(inline)]
pub enum LintRule {
    CircularDependency,
    GodNode,
    HighFanOut,
    UnstableDependency,
}

impl LintRule {
    /// The rule id `lint_architecture` expects.
    pub fn as_str(self) -> &'static str {
        match self {
            LintRule::CircularDependency => "circular_dependency",
            LintRule::GodNode => "god_node",
            LintRule::HighFanOut => "high_fan_out",
            LintRule::UnstableDependency => "unstable_dependency",
        }
    }
}

/// Accepts a JSON array or, for callers of the older string form, a
/// comma-separated string of enum values.
fn list_or_comma_string<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = Option::<serde_json::Value>::deserialize(deserializer)?;
    let items = match raw {
        None | Some(serde_json::Value::Null) => return Ok(None),
        Some(serde_json::Value::String(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(|p| serde_json::Value::String(p.to_string()))
            .collect(),
        Some(serde_json::Value::Array(items)) => items,
        Some(other) => return Err(D::Error::custom(format!("expected a list, got {other}"))),
    };
    items
        .into_iter()
        .map(|v| serde_json::from_value(v).map_err(D::Error::custom))
        .collect::<Result<Vec<T>, _>>()
        .map(Some)
}

// ── Tool parameter types ─────────────────────────────────────────────────

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
    /// Must be true: deletion is irreversible (re-run analyze to rebuild)
    #[serde(default)]
    pub confirm: bool,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ListReposParams {
    /// Max repos per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Repos to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SearchParams {
    /// Search query string
    pub query: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max results per page (default 20, semantic_search 10; max 100)
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// Results to skip, for the next page (default 0)
    pub offset: Option<usize>,
    /// Show scoring breakdown and graph paths for each result (default: false)
    pub explain: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SymbolContextParams {
    /// Name or qualified name of the symbol to look up
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
    /// Max rows per page (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Rows to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct RenameParams {
    /// Name of the symbol to rename
    pub symbol_name: String,
    /// New name for the symbol
    pub new_name: String,
    /// Repository ID
    pub repo_id: String,
    /// Max edits per page (default 100, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Edits to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DetectImpactParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Diff string (optional -- if omitted, runs git diff HEAD in repo path)
    pub diff: Option<String>,
    /// Graph traversal depth (default: 2)
    #[schemars(range(min = 1, max = 10))]
    pub depth: Option<u32>,
    /// Max affected symbols per page (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Affected symbols to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct HybridSearchParams {
    /// Search query string
    pub query: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max results per page (default 20, max 100)
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// Results to skip, for the next page (default 0)
    pub offset: Option<usize>,
    /// Apply cross-encoder reranking to results (default: false)
    pub rerank: Option<bool>,
    /// Show scoring breakdown and graph paths for each result (default: false)
    pub explain: Option<bool>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ReviewContextParams {
    /// Diff string (optional — if omitted, runs git diff HEAD in repo path)
    pub diff: Option<String>,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Graph traversal depth for blast radius (default: 1)
    #[schemars(range(min = 1, max = 10))]
    pub depth: Option<u32>,
    /// Include full source code of changed symbols (default: false)
    pub include_source: Option<bool>,
    /// Max callers and max callees listed (default 50, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetProcessesParams {
    /// Repository ID
    pub repo_id: String,
    /// Filter by entry point name (case-insensitive substring match)
    pub entry: Option<String>,
    /// Filter by keyword in process description/flow (case-insensitive substring match)
    pub filter: Option<String>,
    /// Max processes per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Processes to skip, for the next page (default 0)
    pub offset: Option<usize>,
    /// Show only processes with N or more steps
    pub min_steps: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCommunitiesParams {
    /// Repository ID
    pub repo_id: String,
    /// Max communities per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Communities to skip, for the next page (default 0)
    pub offset: Option<usize>,
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
    /// Comma-separated substrings; symbols whose name contains one are skipped (e.g. known entry points)
    pub exclude_patterns: Option<String>,
    /// Max symbols per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Symbols to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCallersParams {
    /// Repository ID
    pub repo_id: String,
    /// Symbol name or qualified name to find callers for
    pub symbol_name: String,
    /// Maximum transitive depth (default: 3)
    #[schemars(range(min = 1, max = 10))]
    pub max_depth: Option<u32>,
    /// Max callers per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Callers to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCalleesParams {
    /// Repository ID
    pub repo_id: String,
    /// Symbol name or qualified name to find callees for
    pub symbol_name: String,
    /// Maximum transitive depth (default: 3)
    #[schemars(range(min = 1, max = 10))]
    pub max_depth: Option<u32>,
    /// Max callees per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Callees to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetFileSymbolsParams {
    /// Repository ID
    pub repo_id: String,
    /// File path to list symbols for
    pub file_path: String,
    /// Max symbols per page (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Symbols to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetStatsParams {
    /// Repository ID
    pub repo_id: String,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetGodNodesParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max nodes per page (default 10, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub top_n: Option<usize>,
    /// Nodes to skip, for the next page (default 0)
    pub offset: Option<usize>,
    /// Coupling threshold: nodes with degree > this are flagged as high-coupling (default: 20)
    #[schemars(range(min = 1))]
    pub coupling_threshold: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSurprisingConnectionsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Minimum surprise score in [0.0, 1.0] to include in results (default: 0.1)
    #[schemars(range(min = 0.0, max = 1.0))]
    pub min_surprise_score: Option<f64>,
    /// Max connections per page (default 50, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Connections to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct FindPathParams {
    /// Symbol name or qualified name for the start of the path
    pub from_symbol: String,
    /// Symbol name or qualified name for the end of the path
    pub to_symbol: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Maximum BFS depth (default: 10)
    #[schemars(range(min = 1, max = 30))]
    pub max_depth: Option<u32>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetKnowledgeGapsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Only this gap category (default: all)
    pub category: Option<GapCategory>,
    /// Max gaps per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Gaps to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetRationaleParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Symbol name or qualified name to get rationale for (optional — provide this OR file_path)
    pub symbol_name: Option<String>,
    /// File path to get all rationale comments for (optional — provide this OR symbol_name)
    pub file_path: Option<String>,
    /// Max comments per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Comments to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetSuggestedQuestionsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Diff string (optional — if omitted, runs git diff HEAD in repo path)
    pub diff: Option<String>,
    /// Max questions per page (default 5, max 50)
    #[schemars(range(min = 1, max = 50))]
    pub limit: Option<usize>,
    /// Questions to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetGraphDiffParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max changes per page, across all change lists (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Changes to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct SearchEmailsParams {
    /// Search keyword to match in email subject or body
    pub query: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Filter by person email address (optional)
    pub person: Option<String>,
    /// Filter by date (ISO 8601 prefix, e.g. "2026-04") (optional)
    pub date: Option<String>,
    /// Max emails per page (default 20, max 100)
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// Emails to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetConversationParams {
    /// Conversation symbol UID
    pub conversation_uid: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max emails per page (default 20, max 100)
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// Emails to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetPersonContextParams {
    /// Person email address or name to search for
    pub person: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max emails per page (default 50, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Emails to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct QueryKnowledgeParams {
    /// Natural language query or symbol name to search for across all knowledge
    pub query: String,
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Include source citations with line numbers and context (default: true)
    pub include_sources: Option<bool>,
    /// Max mentions per page (default 20, max 100)
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// Mentions to skip, for the next page (default 0)
    pub offset: Option<usize>,
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
    #[schemars(range(min = 1, max = 200))]
    pub max_symbols: Option<usize>,
    /// Call graph expansion depth (default: 2)
    #[schemars(range(min = 1, max = 5))]
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
    /// Similarity threshold for symbol alignment (default: 0.65)
    #[schemars(range(min = 0.0, max = 1.0))]
    pub similarity_threshold: Option<f64>,
    /// Maximum symbols per slice (default: 50)
    #[schemars(range(min = 1, max = 200))]
    pub max_symbols: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetGitContextParams {
    /// Symbol name to look up git metadata for (case-insensitive substring match)
    pub symbol_name: String,
    /// Repository ID
    pub repo_id: String,
    /// Max symbols per page (default 10, max 100)
    #[schemars(range(min = 1, max = 100))]
    pub limit: Option<usize>,
    /// Symbols to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct PlanAdaptationParams {
    /// Natural language description of the intent
    pub intent: String,
    /// Repository whose approach to adapt FROM
    pub source_repo_id: String,
    /// Repository whose approach to adapt TO
    pub target_repo_id: String,
    /// Which side to adapt (default: source_to_target, i.e. adapt the target to match the source)
    pub direction: Option<AdaptationDirection>,
    /// Maximum symbols per slice (default: 50)
    #[schemars(range(min = 1, max = 200))]
    pub max_symbols: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCentralityParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Centrality metric to sort by (default: betweenness)
    pub metric: Option<CentralityMetric>,
    /// Max nodes per page (default 15, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub top_n: Option<usize>,
    /// Nodes to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetCommunityMetricsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max rows per table (cohesion, coupling) (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Rows to skip in each table, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DetectCyclesParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Include CALLS edges in cycle detection (default: true)
    pub include_calls: Option<bool>,
    /// Include IMPORTS edges in cycle detection (default: true)
    pub include_imports: Option<bool>,
    /// Minimum number of symbols in a cycle to report (default: 2)
    #[schemars(range(min = 2))]
    pub min_cycle_size: Option<usize>,
    /// Max cycles per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Cycles to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetDependenciesParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// File path to analyze dependencies for
    pub file_path: String,
    /// Maximum transitive depth (default: unlimited)
    #[schemars(range(min = 1))]
    pub max_depth: Option<u32>,
    /// Max files per list (direct, transitive, dependents) (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Files to skip in each list, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetModuleCouplingParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Group by directory instead of individual files (default: false)
    pub group_by_directory: Option<bool>,
    /// Max modules per page (default 30, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Modules to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct QualityHotspotsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max hotspots per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub top_n: Option<usize>,
    /// Hotspots to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ArchitectureLintParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Rules to check (default: all)
    #[serde(default, deserialize_with = "list_or_comma_string")]
    pub rules: Option<Vec<LintRule>>,
    /// God node degree threshold (default: 20)
    #[schemars(range(min = 1))]
    pub god_node_threshold: Option<u32>,
    /// Max findings per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Findings to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ArchitectureViewParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct DetectDriftParams2 {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetOwnershipParams {
    /// Repository ID
    pub repo_id: String,
    /// Max files per list (owned, unowned) (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Files to skip in each list, for the next page (default 0)
    pub offset: Option<usize>,
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
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max mappings per page (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Mappings to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct RecordDecisionParams {
    /// Repository ID
    pub repo_id: String,
    /// ADR title
    pub title: String,
    /// Status (default: proposed)
    pub status: Option<DecisionStatus>,
    /// Context and motivation
    pub context: String,
    /// The decision made
    pub decision: String,
    /// Expected consequences (optional)
    pub consequences: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetDecisionsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Only decisions with this status (optional)
    pub status: Option<DecisionStatus>,
    /// Max decisions per page (default 20, max 200)
    #[schemars(range(min = 1, max = 200))]
    pub limit: Option<usize>,
    /// Decisions to skip, for the next page (default 0)
    pub offset: Option<usize>,
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
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Snapshot ID to compare FROM (default: second-to-latest)
    pub from_snapshot: Option<String>,
    /// Snapshot ID to compare TO (default: latest)
    pub to_snapshot: Option<String>,
    /// Max changes per page, across added and removed (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Changes to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct GetContractsParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Max unlinked endpoints listed (default 50, max 500)
    #[schemars(range(min = 1, max = 500))]
    pub limit: Option<usize>,
    /// Unlinked endpoints to skip, for the next page (default 0)
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema, Default)]
pub struct ExportMermaidParams {
    /// Repository ID (default: repo of the working directory, else the most recent)
    pub repo_id: Option<String>,
    /// Diagram type (default: flowchart)
    pub diagram_type: Option<DiagramType>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lint_rules_accept_a_list_or_the_legacy_comma_string() {
        let from_list: ArchitectureLintParams =
            serde_json::from_value(serde_json::json!({"rules": ["god_node", "high_fan_out"]}))
                .unwrap();
        let from_text: ArchitectureLintParams =
            serde_json::from_value(serde_json::json!({"rules": "god_node, high_fan_out"})).unwrap();
        let expected = Some(vec![LintRule::GodNode, LintRule::HighFanOut]);
        assert_eq!(from_list.rules, expected);
        assert_eq!(from_text.rules, expected);
        let absent: ArchitectureLintParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(absent.rules, None);
        assert!(serde_json::from_value::<ArchitectureLintParams>(
            serde_json::json!({"rules": "god_node,bogus"})
        )
        .is_err());
    }

    #[test]
    fn unknown_enum_values_are_rejected() {
        assert!(serde_json::from_value::<GetCentralityParams>(
            serde_json::json!({"metric": "pagerank"})
        )
        .is_err());
        let ok: GetCentralityParams =
            serde_json::from_value(serde_json::json!({"metric": "closeness"})).unwrap();
        assert_eq!(ok.metric, Some(CentralityMetric::Closeness));
    }
}
