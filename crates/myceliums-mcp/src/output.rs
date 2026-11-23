//! Structured tool output types, rendered to text by [`crate::format`], and
//! the shared output budget ([`Page`]) every list-returning tool applies.
//!
//! Items here carry no doc comments on purpose: `schemars` turns doc comments
//! into JSON-schema `description`s, so documenting a field or variant would
//! change the published tool contract (`tests/snapshots/tools_list*.json`).
//! The contract is documented in the tool descriptions instead.

#![allow(missing_docs)]

use rmcp::schemars;
use serde::Serialize;

// ── Output budget ────────────────────────────────────────────────────────

/// Default and hard maximum page size of a list-returning tool.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Budget {
    pub(crate) default: usize,
    pub(crate) max: usize,
}

/// Largest `offset` any tool accepts (larger values are clamped).
pub(crate) const MAX_OFFSET: usize = 100_000;

impl Budget {
    /// Most list tools: 20 per page, at most 200.
    pub(crate) const LIST: Budget = Budget::new(20, 200);
    /// Cypher result rows: 50 per page, at most 500.
    pub(crate) const ROWS: Budget = Budget::new(50, 500);

    /// A budget with its own default (tools that always had one keep it).
    pub(crate) const fn new(default: usize, max: usize) -> Budget {
        Budget { default, max }
    }
}

/// One page of a list result: `limit` items starting at `offset`.
///
/// Every list-returning tool cuts its results through a `Page`, so a cut is
/// never silent: [`Paged::notice`] says how much was shown and which `offset`
/// fetches the rest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Page {
    pub(crate) offset: usize,
    pub(crate) limit: usize,
}

impl Page {
    /// The requested page; `limit` falls back to the budget's default and is
    /// clamped into `1..=max` (the notice then shows what was cut).
    pub(crate) fn new(limit: Option<usize>, offset: Option<usize>, budget: Budget) -> Page {
        Page {
            offset: offset.unwrap_or(0).min(MAX_OFFSET),
            limit: limit.unwrap_or(budget.default).clamp(1, budget.max),
        }
    }

    /// How many leading items a top-k source must produce so that
    /// [`Page::take_open`] can fill this page and tell whether more exist.
    pub(crate) fn fetch(&self) -> usize {
        self.offset.saturating_add(self.limit).saturating_add(1)
    }

    /// Cuts this page out of the complete, ordered result list.
    pub(crate) fn take<T>(&self, items: Vec<T>) -> Paged<T> {
        let total = items.len();
        let items: Vec<T> = items
            .into_iter()
            .skip(self.offset)
            .take(self.limit)
            .collect();
        let notice = self.notice(items.len(), Some(total), false);
        Paged { items, notice }
    }

    /// Cuts this page out of the first [`Page::fetch`] items of a longer
    /// ranking whose total size is unknown.
    pub(crate) fn take_open<T>(&self, items: Vec<T>) -> Paged<T> {
        let more = items.len() > self.end();
        let items: Vec<T> = items
            .into_iter()
            .skip(self.offset)
            .take(self.limit)
            .collect();
        let notice = self.notice(items.len(), None, more);
        Paged { items, notice }
    }

    /// One past the last index of this page.
    fn end(&self) -> usize {
        self.offset.saturating_add(self.limit)
    }

    fn notice(&self, shown: usize, total: Option<usize>, more: bool) -> Option<String> {
        let first = self.offset.saturating_add(1);
        let last = self.offset.saturating_add(shown);
        match total {
            Some(total) if shown == 0 && self.offset > 0 && total > 0 => Some(format!(
                "No results at offset={} ({total} in total) — use a smaller offset.",
                self.offset
            )),
            Some(total) if last < total => Some(format!(
                "Showing {first}-{last} of {total} — call again with offset={last} for more."
            )),
            Some(total) if self.offset > 0 => {
                Some(format!("Showing {first}-{last} of {total} (end)."))
            }
            None if more => Some(format!(
                "Showing {first}-{last}; more exist — call again with offset={last}."
            )),
            None if shown == 0 && self.offset > 0 => Some(format!(
                "No results at offset={} — use a smaller offset.",
                self.offset
            )),
            None if self.offset > 0 => Some(format!("Showing {first}-{last} (end).")),
            _ => None,
        }
    }
}

impl Page {
    /// Pages several parallel lists (e.g. direct / transitive / dependents)
    /// with the same offset and limit, under one notice.
    pub(crate) fn take_lists<T>(&self, lists: Vec<Vec<T>>) -> (Vec<Vec<T>>, Option<String>) {
        let longest = lists.iter().map(Vec::len).max().unwrap_or(0);
        let more = longest > self.end();
        let paged: Vec<Vec<T>> = lists
            .into_iter()
            .map(|l| l.into_iter().skip(self.offset).take(self.limit).collect())
            .collect();
        let first = self.offset.saturating_add(1);
        let last = self.end();
        let notice = if more {
            Some(format!(
                "Lists show items {first}-{last} — call again with offset={last} for more."
            ))
        } else if self.offset > 0 {
            Some(format!("Lists show items from {first} (end)."))
        } else {
            None
        };
        (paged, notice)
    }
}

/// One page of results plus the notice describing the cut, if any.
#[derive(Debug)]
pub(crate) struct Paged<T> {
    pub(crate) items: Vec<T>,
    pub(crate) notice: Option<String>,
}

impl<T> Paged<T> {
    /// Appends the notice (if any) to a rendered body.
    pub(crate) fn with_notice(&self, body: String) -> String {
        with_notice(body, self.notice.as_deref())
    }
}

/// Appends a truncation notice to a rendered body on its own line.
pub(crate) fn with_notice(mut body: String, notice: Option<&str>) -> String {
    if let Some(notice) = notice {
        if !body.ends_with('\n') {
            body.push('\n');
        }
        body.push('\n');
        body.push_str(notice);
    }
    body
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
    /// Sizes of the cohesion and coupling tables before paging.
    pub totals: [usize; 2],
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
    /// Sizes of the three lists before paging: direct, transitive, dependents.
    pub totals: [usize; 3],
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
    /// `complete`, `partial` (the time or iteration budget ran out; the best
    /// partition found by then) or `skipped`. Absent for an index analyzed
    /// before it was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communities_state: Option<String>,
    /// Why the communities are partial or skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communities_notice: Option<String>,
    /// Wall time per phase (discover, parse, resolve, communities,
    /// processes, embed). Only for a fresh analysis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phases: Option<String>,
    pub processes: usize,
    /// Number of symbols that received an embedding vector.
    pub symbols_embedded: usize,
    /// Number of symbols that were candidates for embedding.
    pub symbols_total: usize,
    /// Number of symbols whose embedding failed (0 = fully embedded / skipped).
    pub embedding_failures: usize,
    /// Of `symbols_embedded`, the vectors kept from the previous index for
    /// unchanged symbols (0 for a cached result).
    pub embeddings_reused: usize,
    /// Symbols being embedded in the background after this call (#157).
    #[serde(skip_serializing_if = "is_zero")]
    pub embeddings_pending: usize,
    /// The embedding model of the background embedding, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub embedding_model: Option<String>,
    /// Call sites linked by a CALLS edge, and why the others were not: more
    /// candidates than the resolver links, a library target, a call through
    /// a variable, nothing in scope, outside any function. Absent for an
    /// index analyzed before these were recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub calls: Option<String>,
    /// Files left out of the index for a known reason (size, line length,
    /// minified, binary, parse budget), counted per reason, most frequent
    /// first. Only for a fresh analysis: the reasons are not stored.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skipped: Option<String>,
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
    /// Caller names (heuristic edges marked), at most one page.
    pub callers: Vec<String>,
    /// All callers, including those not listed.
    pub callers_total: usize,
    /// Callee names (heuristic edges marked), at most one page.
    pub callees: Vec<String>,
    /// All callees, including those not listed.
    pub callees_total: usize,
    /// The methods this one overrides or implements (#190), as
    /// `file:qualified_name` handles.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<String>,
    /// Concrete implementations of this method, at most one page.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub implementations: Vec<String>,
    /// All implementations, including those not listed.
    #[serde(skip_serializing_if = "is_zero")]
    pub implementations_total: usize,
    pub metadata: Option<String>,
    /// Set when `content` was cut to the source budget: how to get the rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_notice: Option<String>,
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

/// `list_repos`: the analyzed repositories.
#[derive(Serialize, schemars::JsonSchema)]
pub struct ReposOutput {
    /// One page of repositories, most recently analyzed first.
    pub repos: Vec<RepoItem>,
    pub total_count: usize,
    /// Set when the list was cut (how to fetch the next page) or is empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct RepoItem {
    /// Pass this as `repo_id` to the other tools.
    pub id: String,
    pub name: String,
    /// Absolute path of the project.
    pub path: String,
    /// When the index was built (RFC 3339).
    pub analyzed_at: String,
    /// Git commit the index was built from, if the project is a git checkout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub analyzed_commit: Option<String>,
    pub symbols: u32,
    pub files: u32,
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
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    /// Set when the index's communities are partial (the detection budget
    /// ran out) or were skipped, and why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communities_notice: Option<String>,
    pub communities: Vec<CommunityItem>,
    pub total_count: usize,
    /// Set when results were cut: how to fetch the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
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
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    /// Set when the index's communities are partial or were skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub communities_notice: Option<String>,
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
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    pub signature: String,
    /// Source text, framed as untrusted indexed content.
    pub content: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct DeadCodeOutput {
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    pub symbols: Vec<DeadCodeItem>,
    pub total_count: u32,
    /// Set when results were cut: how to fetch the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
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
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    pub symbol_name: String,
    pub callers: Vec<CallerItem>,
    pub total_count: u32,
    pub depth_limited: bool,
    /// Set when this is not the whole answer: why, and the call that continues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<Incomplete>,
    /// Set when results were cut: how to fetch the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

/// Why a graph walk is not the whole answer.
#[derive(Serialize, schemars::JsonSchema)]
pub struct Incomplete {
    /// One per cut: depth limit, page, calls or call sites without a CALLS edge.
    pub reasons: Vec<String>,
    /// Places not followed: symbols one more step would add, plus unlinked call sites.
    pub branches_cut: u32,
    /// The call that continues the walk.
    pub next: String,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CallerItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Calls between this symbol and the one asked about (1 = direct).
    pub depth: u32,
    /// For depth > 1: the name of the symbol one step closer to the one
    /// asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// How the CALLS edge reaching this symbol was resolved:
    /// exact | import | name-match | ambiguous.
    pub resolved: String,
    /// The declaration line; only with `detail: full` (#181).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CalleesOutput {
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    pub symbol_name: String,
    pub callees: Vec<CalleeItem>,
    pub total_count: u32,
    pub depth_limited: bool,
    /// Set when this is not the whole answer: why, and the call that continues.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incomplete: Option<Incomplete>,
    /// Set when results were cut: how to fetch the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct CalleeItem {
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: u32,
    pub end_line: u32,
    /// Calls between this symbol and the one asked about (1 = direct).
    pub depth: u32,
    /// For depth > 1: the name of the symbol one step closer to the one
    /// asked about.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// How the CALLS edge reaching this symbol was resolved:
    /// exact | import | name-match | ambiguous.
    pub resolved: String,
    /// The declaration line; only with `detail: full` (#181).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub signature: Option<String>,
}

#[derive(Serialize, schemars::JsonSchema)]
pub struct FileSymbolsOutput {
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
    pub file_path: String,
    pub symbols: Vec<FileSymbolItem>,
    pub total_count: u32,
    /// Set when results were cut: how to fetch the next page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
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
    /// Index freshness: analyzed commit vs current HEAD.
    pub index_status: String,
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

/// serde: leave a zero count out.
fn is_zero(n: &usize) -> bool {
    *n == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limit_defaults_and_is_clamped_to_the_budget() {
        let budget = Budget::new(20, 200);
        assert_eq!(
            Page::new(None, None, budget),
            Page {
                offset: 0,
                limit: 20
            }
        );
        assert_eq!(Page::new(Some(0), None, budget).limit, 1);
        assert_eq!(
            Page::new(Some(10_000), Some(7), budget),
            Page {
                offset: 7,
                limit: 200
            }
        );
        // A huge offset is clamped instead of overflowing the arithmetic.
        let far = Page::new(Some(200), Some(usize::MAX), budget);
        assert_eq!(far.offset, MAX_OFFSET);
        let past = far.take(vec![1, 2, 3]);
        assert!(past.items.is_empty());
        assert!(past.notice.unwrap().contains("smaller offset"));
        assert!(far.take_open(vec![1]).items.is_empty());
    }

    #[test]
    fn a_cut_list_says_what_was_shown_and_how_to_continue() {
        let items: Vec<u32> = (0..312).collect();
        let first = Page::new(Some(20), None, Budget::LIST).take(items.clone());
        assert_eq!(first.items, (0..20).collect::<Vec<_>>());
        assert_eq!(
            first.notice.as_deref(),
            Some("Showing 1-20 of 312 — call again with offset=20 for more.")
        );

        let last = Page::new(Some(20), Some(300), Budget::LIST).take(items.clone());
        assert_eq!(last.items.len(), 12);
        assert_eq!(
            last.notice.as_deref(),
            Some("Showing 301-312 of 312 (end).")
        );

        let past = Page::new(Some(20), Some(400), Budget::LIST).take(items.clone());
        assert!(past.items.is_empty());
        assert!(past.notice.unwrap().contains("smaller offset"));

        // Nothing cut, first page: no notice at all.
        let whole = Page::new(Some(500), None, Budget::ROWS).take(items);
        assert_eq!(whole.items.len(), 312);
        assert_eq!(whole.notice, None);
    }

    #[test]
    fn open_ended_rankings_report_more_without_a_total() {
        let page = Page::new(Some(10), Some(10), Budget::LIST);
        assert_eq!(page.fetch(), 21);
        let more = page.take_open((0..21).collect::<Vec<u32>>());
        assert_eq!(more.items, (10..20).collect::<Vec<_>>());
        assert_eq!(
            more.notice.as_deref(),
            Some("Showing 11-20; more exist — call again with offset=20.")
        );
        let end = page.take_open((0..15).collect::<Vec<u32>>());
        assert_eq!(end.items.len(), 5);
        assert_eq!(end.notice.as_deref(), Some("Showing 11-15 (end)."));
    }

    #[test]
    fn parallel_lists_share_one_notice() {
        let page = Page::new(Some(2), None, Budget::LIST);
        let (lists, notice) = page.take_lists(vec![vec![1, 2, 3], vec![4]]);
        assert_eq!(lists, vec![vec![1, 2], vec![4]]);
        assert_eq!(
            notice.as_deref(),
            Some("Lists show items 1-2 — call again with offset=2 for more.")
        );
    }

    #[test]
    fn notices_go_on_their_own_line() {
        assert_eq!(with_notice("body".into(), Some("n")), "body\n\nn");
        assert_eq!(with_notice("body\n".into(), Some("n")), "body\n\nn");
        assert_eq!(with_notice("body".into(), None), "body");
    }
}
