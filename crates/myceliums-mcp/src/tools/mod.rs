//! The MCP tools, one module per tool group.
//!
//! Each module holds one `#[tool_router(router = <group>_router)]` impl block;
//! [`MyceliumsMcp::tool_router`] merges them into the single router the
//! server exposes. Tool names, descriptions, and schemas come from the
//! `#[tool]` attributes alone, so where a tool lives does not change
//! `tools/list` (pinned by `tests/tools_list_snapshot.rs`).

mod admin;
mod architecture;
mod conversation;
mod cross_repo;
mod decisions;
mod graph;
mod impact;
mod knowledge;
mod search;

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

    pub(crate) use crate::errors::{repo_id_error, service_error};
    pub(crate) use crate::format;
    pub(crate) use crate::index_status::{load_embedding_stats, partial_index_warning};
    pub(crate) use crate::output::*;
    pub(crate) use crate::params::*;
    pub(crate) use crate::repo::{data_dir, registered_db_path, registry_path};
    pub(crate) use crate::MyceliumsMcp;
}

impl MyceliumsMcp {
    /// Every tool the server exposes: the union of the per-group routers.
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
