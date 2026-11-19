//! The MCP tools, one module per tool group.
//!
//! Each module holds one `#[tool_router(router = <group>_router)]` impl block;
//! [`MyceliumsMcp::tool_router`] merges them into one router, and
//! [`MyceliumsMcp::tool_router_for`] keeps the selected toolsets of it (see
//! [`crate::toolsets`]). Tool names, descriptions, and schemas come from the
//! `#[tool]` attributes (plus the toolset decoration), so where a tool lives
//! does not change `tools/list` (pinned by `tests/tools_list_snapshot.rs`).
//!
//! Output: a tool whose answer is prose or a report returns
//! [`TextResult`] — plain text content, no `structuredContent`, no output
//! schema. A tool whose answer is a record or a list returns
//! `Result<Json<T>, ToolError>` — typed `structuredContent` with the output
//! schema derived from `T` (rmcp also sends the same JSON as text content,
//! as the MCP spec recommends).

mod admin;
mod architecture;
mod conversation;
mod cross_repo;
mod decisions;
mod graph;
mod impact;
mod knowledge;
mod search;

use crate::errors::ToolError;
use crate::toolsets::Toolsets;
use crate::MyceliumsMcp;
use rmcp::handler::server::tool::ToolRouter;

/// Imports shared by every tool module.
mod prelude {
    #![allow(unused_imports)]
    pub(crate) use anyhow::Result;
    pub(crate) use myceliums_core::analyzer::{self, Analyzer};
    pub(crate) use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
    pub(crate) use myceliums_core::{
        attach_graph_edges, build_snapshot, compute_centrality, compute_community_metrics,
        compute_file_dependencies, compute_god_nodes, compute_hotspot_scores,
        compute_module_coupling, compute_ownership, compute_surprising_connections,
        detect_contracts, detect_cycles, detect_drift, detect_impact, diff_snapshots,
        export_mermaid, generate_architecture_diagram, hybrid_search as core_hybrid_search,
        hybrid_search_explain as core_hybrid_search_explain, link_decision_to_symbol,
        lint_architecture, list_snapshots, load_decisions, load_service_mappings, load_snapshot,
        load_snapshot_by_id, parse_codeowners, rerank_results, run_git_diff, save_decision,
        save_service_mapping, save_snapshot, search_symbols, AdrStatus, ArchDecisionRecord,
        CommunityDetector, GraphService, MermaidDiagramType, Ontology, ProcessFilter,
        ProcessTracer, RenamePlan,
    };
    pub(crate) use myceliums_storage::{RepoIdError, RepoInfo, RepoRegistry, Store};
    pub(crate) use rmcp::handler::server::tool::ToolRouter;
    pub(crate) use rmcp::handler::server::wrapper::{Json, Parameters};
    pub(crate) use rmcp::model::*;
    pub(crate) use rmcp::schemars;
    pub(crate) use rmcp::service::{RequestContext, RoleServer};
    pub(crate) use rmcp::{tool, tool_router, ServerHandler, ServiceExt};
    pub(crate) use serde::{Deserialize, Serialize};
    pub(crate) use std::collections::HashMap;
    pub(crate) use std::path::PathBuf;
    pub(crate) use tracing::info;

    pub(crate) use super::{require_query, text, TextResult};
    pub(crate) use crate::errors::{repo_id_error, service_error, ToolError};
    pub(crate) use crate::format::{self, untrusted};
    pub(crate) use crate::index_status::{load_embedding_stats, partial_index_warning};
    pub(crate) use crate::navigation;
    pub(crate) use crate::navigation::resolve as resolve_symbol;
    pub(crate) use crate::output::*;
    pub(crate) use crate::params::*;
    pub(crate) use crate::repo::{data_dir, registry, repo_info};
    pub(crate) use crate::MyceliumsMcp;
}

/// Shared response shaping: every graph result starts with the index
/// freshness line.
impl MyceliumsMcp {
    /// One line on how current `repo_id`'s index is (see
    /// [`crate::index_status::freshness_line`]).
    pub(crate) async fn index_status(&self, repo_id: &str) -> Result<String, ToolError> {
        let info = crate::repo::repo_info(repo_id)?;
        let indexed = self.files(repo_id).await?.shared();
        Ok(crate::index_status::freshness_line(&info, indexed).await)
    }

    /// A text graph result: freshness line, then `body`.
    pub(crate) async fn graph_text(&self, repo_id: &str, body: String) -> TextResult {
        let status = self.index_status(repo_id).await?;
        text(format!("{status}\n\n{body}"))
    }
}

/// The result of a text tool: plain text content on success.
pub(crate) type TextResult = Result<String, ToolError>;

/// A plain text result (no graph read, so no freshness line).
pub(crate) fn text(text: impl Into<String>) -> TextResult {
    Ok(text.into())
}

/// Rejects an empty query string up front: it is bad input, not "no results".
pub(crate) fn require_query(field: &str, value: &str) -> Result<(), ToolError> {
    if value.trim().is_empty() {
        Err(ToolError::invalid(format!("`{field}` must not be empty")))
    } else {
        Ok(())
    }
}

impl MyceliumsMcp {
    /// The tools of the selected toolsets, decorated for agents (see
    /// [`crate::toolsets::select`]).
    pub(crate) fn tool_router_for(toolsets: &Toolsets) -> ToolRouter<Self> {
        crate::toolsets::select(Self::tool_router(), toolsets)
    }

    /// Every tool, undecorated: the union of the per-group routers.
    pub(crate) fn tool_router() -> ToolRouter<Self> {
        Self::admin_router()
            + Self::search_router()
            + Self::graph_router()
            + Self::impact_router()
            + Self::architecture_router()
            + Self::knowledge_router()
            + Self::conversation_router()
            + Self::cross_repo_router()
            + Self::decisions_router()
    }
}
