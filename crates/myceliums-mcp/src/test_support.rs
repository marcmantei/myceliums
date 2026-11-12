//! Test-facing wrappers (feature `test-support`).

use crate::params::*;
use crate::MyceliumsMcp;
use rmcp::handler::server::wrapper::Parameters;

/// Public, test-facing wrappers around the private `#[tool]` handlers.
///
/// The tool handlers are private because their only production caller is the
/// rmcp tool router. Integration tests, however, need to exercise the exact
/// handler logic (store wiring, result shaping, error mapping) without standing
/// up a full stdio transport. These thin delegates expose that surface behind
/// the `test-support` feature so the shipped API stays narrow.
///
/// Each wrapper returns the rendered `text` payload on success, or the mapped
/// error message on failure — the two things an integration test asserts on.
impl MyceliumsMcp {
    /// The tool definitions the server advertises on `tools/list` — names,
    /// descriptions, and input/output schemas, sorted by name. Built from the
    /// same router [`run_mcp_server`] serves, so a snapshot of this list pins
    /// the agent-visible tool surface.
    pub fn tool_list_for_test() -> Vec<rmcp::model::Tool> {
        Self::tool_router().list_all()
    }

    /// Invoke the `context_search` handler.
    pub async fn context_search_for_test(&self, params: SearchParams) -> Result<String, String> {
        self.context_search(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `hybrid_search` handler.
    pub async fn hybrid_search_for_test(
        &self,
        params: HybridSearchParams,
    ) -> Result<String, String> {
        self.hybrid_search(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `detect_impact` handler.
    pub async fn detect_impact_for_test(
        &self,
        params: DetectImpactParams,
    ) -> Result<String, String> {
        self.detect_impact_tool(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `rename_symbol` handler.
    pub async fn rename_symbol_for_test(&self, params: RenameParams) -> Result<String, String> {
        self.rename_symbol(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `semantic_search` handler.
    pub async fn semantic_search_for_test(&self, params: SearchParams) -> Result<String, String> {
        self.semantic_search(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `delete` handler.
    pub async fn delete_for_test(&self, params: DeleteParams) -> Result<String, String> {
        self.delete(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `map_service` handler.
    pub async fn map_service_for_test(&self, params: MapServiceParams) -> Result<String, String> {
        self.map_service(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `record_decision` handler.
    pub async fn record_decision_for_test(
        &self,
        params: RecordDecisionParams,
    ) -> Result<String, String> {
        self.record_decision(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `link_decision` handler.
    pub async fn link_decision_for_test(
        &self,
        params: LinkDecisionParams,
    ) -> Result<String, String> {
        self.link_decision(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Read a `repository://...` resource.
    pub async fn read_resource_for_test(&self, uri: &str) -> Result<String, String> {
        self.read_resource_inner(uri)
            .await
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `list_repos` handler.
    pub async fn list_repos_for_test(&self) -> Result<String, String> {
        self.list_repos()
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `symbol_context` handler.
    pub async fn symbol_context_for_test(
        &self,
        params: SymbolContextParams,
    ) -> Result<String, String> {
        self.symbol_context(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }

    /// Invoke the `cypher_query` handler.
    pub async fn cypher_query_for_test(&self, params: CypherQueryParams) -> Result<String, String> {
        self.cypher_query(Parameters(params))
            .await
            .map(|json| json.0.text)
            .map_err(|e| e.message.to_string())
    }
}
