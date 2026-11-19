//! Test-facing wrappers (feature `test-support`).

use crate::params::*;
use crate::{MyceliumsMcp, Toolsets};
use rmcp::handler::server::wrapper::Parameters;

/// Public, test-facing wrappers around the private `#[tool]` handlers.
///
/// The tool handlers are private because their only production caller is the
/// rmcp tool router. Integration tests, however, need to exercise the exact
/// handler logic (store wiring, result shaping, error mapping) without standing
/// up a full stdio transport. These thin delegates expose that surface behind
/// the `test-support` feature so the shipped API stays narrow.
///
/// Each wrapper returns the text content on success (structured tools: their
/// JSON), or the
/// agent-visible error text on failure (an `isError` result's text or a
/// protocol error's message) — the two things an integration test asserts
/// on. The exact wire shape of each tool call is covered end to end by
/// `tests/tool_contract.rs`.
impl MyceliumsMcp {
    /// The tool definitions the server advertises on `tools/list` for
    /// `toolsets` — names, descriptions, input/output schemas and `_meta`,
    /// sorted by name. Built from the same router [`run_mcp_server`] serves,
    /// so a snapshot of this list pins the agent-visible tool surface.
    pub fn tool_list_for_test(toolsets: &Toolsets) -> Vec<rmcp::model::Tool> {
        Self::tool_router_for(toolsets).list_all()
    }

    /// The MCP router with every toolset (deprecated aliases included), for
    /// serving this server over an in-process transport in end-to-end tests.
    pub fn into_router_for_test(
        self,
    ) -> crate::Observed<rmcp::handler::server::router::Router<Self>> {
        self.into_router_with_toolsets_for_test(&Toolsets::all())
    }

    /// The MCP service exposing `toolsets` only: the router wrapped in the
    /// per-call tracing and statistics, exactly as `myc mcp` serves it.
    pub fn into_router_with_toolsets_for_test(
        self,
        toolsets: &Toolsets,
    ) -> crate::Observed<rmcp::handler::server::router::Router<Self>> {
        self.into_service(toolsets)
    }

    /// Invoke the `context_search` handler.
    pub async fn context_search_for_test(&self, params: SearchParams) -> Result<String, String> {
        self.context_search(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `hybrid_search` handler.
    pub async fn hybrid_search_for_test(
        &self,
        params: HybridSearchParams,
    ) -> Result<String, String> {
        self.hybrid_search(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `detect_impact` handler.
    pub async fn detect_impact_for_test(
        &self,
        params: DetectImpactParams,
    ) -> Result<String, String> {
        self.detect_impact_tool(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `rename_symbol` handler.
    pub async fn rename_symbol_for_test(&self, params: RenameParams) -> Result<String, String> {
        self.rename_symbol(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `semantic_search` handler.
    pub async fn semantic_search_for_test(&self, params: SearchParams) -> Result<String, String> {
        self.semantic_search(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `delete` handler.
    pub async fn delete_for_test(&self, params: DeleteParams) -> Result<String, String> {
        self.delete(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `map_service` handler.
    pub async fn map_service_for_test(&self, params: MapServiceParams) -> Result<String, String> {
        self.map_service(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `record_decision` handler.
    pub async fn record_decision_for_test(
        &self,
        params: RecordDecisionParams,
    ) -> Result<String, String> {
        self.record_decision(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `link_decision` handler.
    pub async fn link_decision_for_test(
        &self,
        params: LinkDecisionParams,
    ) -> Result<String, String> {
        self.link_decision(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `detect_architecture_drift` handler.
    pub async fn detect_architecture_drift_for_test(
        &self,
        params: DetectDriftParams,
    ) -> Result<String, String> {
        self.detect_architecture_drift(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Read a `repository://...` resource.
    pub async fn read_resource_for_test(&self, uri: &str) -> Result<String, String> {
        self.read_resource_inner(uri)
            .await
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `list_repos` handler (rendered as JSON).
    pub async fn list_repos_for_test(&self) -> Result<String, String> {
        self.list_repos(Parameters(Default::default()))
            .await
            .map(|json| serde_json::to_string(&json.0).expect("serialize"))
            .map_err(|e| e.message())
    }

    /// Invoke the `symbol_context` handler.
    pub async fn symbol_context_for_test(
        &self,
        params: SymbolContextParams,
    ) -> Result<String, String> {
        self.symbol_context(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `cypher_query` handler.
    pub async fn cypher_query_for_test(&self, params: CypherQueryParams) -> Result<String, String> {
        self.cypher_query(Parameters(params))
            .await
            .map_err(|e| e.message())
    }

    /// Invoke the `get_callers` handler (rendered as JSON).
    pub async fn get_callers_for_test(&self, params: GetCallersParams) -> Result<String, String> {
        self.get_callers(Parameters(params))
            .await
            .map(|json| serde_json::to_string(&json.0).expect("serialize"))
            .map_err(|e| e.message())
    }

    /// Invoke the `get_stats` handler (rendered as JSON).
    pub async fn get_stats_for_test(&self, params: GetStatsParams) -> Result<String, String> {
        self.get_stats(Parameters(params))
            .await
            .map(|json| serde_json::to_string(&json.0).expect("serialize"))
            .map_err(|e| e.message())
    }

    /// Invoke the `analyze` handler (no progress notifications, never
    /// cancelled).
    pub async fn analyze_for_test(&self, params: AnalyzeParams) -> Result<String, String> {
        self.analyze_observed_for_test(params, None, Default::default())
            .await
    }

    /// Invoke the `analyze` handler with an observer that sees every phase
    /// the analyzer reports, and a cancellation token (cancelled: the run
    /// stops before the swap).
    pub async fn analyze_observed_for_test(
        &self,
        params: AnalyzeParams,
        observer: Option<std::sync::Arc<dyn myceliums_core::ProgressReporter>>,
        ct: tokio_util::sync::CancellationToken,
    ) -> Result<String, String> {
        let control = crate::analysis::RunControl {
            sink: None,
            observer,
            ct,
        };
        self.analyze_with(params, control)
            .await
            .map_err(|e| e.message())
    }

    /// How many graph tables the server has read from disk so far (see
    /// `GraphService::table_loads`).
    pub fn graph_table_loads_for_test(&self) -> usize {
        self.service.table_loads()
    }

    /// How many index fingerprints the server has read so far (see
    /// `GraphService::fingerprint_reads`): one per graph snapshot, plus one
    /// per table read from disk.
    pub fn graph_fingerprint_reads_for_test(&self) -> usize {
        self.service.fingerprint_reads()
    }

    /// Whether the server currently holds a cached graph for `repo_id`.
    pub fn graph_is_cached_for_test(&self, repo_id: &str) -> bool {
        self.service.is_cached(repo_id)
    }
}
