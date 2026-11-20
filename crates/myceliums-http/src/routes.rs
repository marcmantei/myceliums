//! The axum router: every route and the middleware stack.

use crate::handlers::analytics::{
    get_centrality, get_community_metrics, get_cycles, get_module_coupling,
};
use crate::handlers::assets::{favicon_handler, index_handler};
use crate::handlers::graph::{get_graph, get_processes, get_stats, get_symbol, list_repos};
use crate::handlers::graph_v1::{graph_v1_node, graph_v1_overview, graph_v1_search};
use crate::handlers::knowledge::query_knowledge;
use crate::handlers::llm_config::{get_llm_config, set_llm_config};
use crate::security::local_only;
use crate::state::AppState;
use axum::{
    http::Request,
    middleware::Next,
    response::Response,
    routing::{get, post},
    Router,
};
use std::sync::Arc;
use std::time::Instant;
use tracing::info;

/// Middleware that logs request latency for API endpoints.
/// Logs path, method, status, and duration in milliseconds.
async fn latency_logger(request: Request<axum::body::Body>, next: Next) -> Response {
    let method = request.method().clone();
    let path = request.uri().path().to_string();
    let start = Instant::now();

    let response = next.run(request).await;

    let duration = start.elapsed();
    let status = response.status().as_u16();

    // Only log API requests (skip static assets)
    if path.starts_with("/api") {
        info!(
            method = %method,
            path = %path,
            status = status,
            duration_ms = duration.as_millis() as u64,
            "request completed"
        );
    }

    response
}

/// Builds the dashboard router with all routes and middleware.
pub(crate) fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/", get(index_handler))
        .route("/favicon.ico", get(favicon_handler))
        .route("/api/repos", get(list_repos))
        .route("/api/repos/{repo_id}/graph", get(get_graph))
        .route("/api/repos/{repo_id}/symbol/{uid}", get(get_symbol))
        .route("/api/repos/{repo_id}/stats", get(get_stats))
        .route("/api/repos/{repo_id}/processes", get(get_processes))
        .route("/api/repos/{repo_id}/knowledge", post(query_knowledge))
        // Graph analytics endpoints
        .route("/api/repos/{repo_id}/centrality", get(get_centrality))
        .route(
            "/api/repos/{repo_id}/community-metrics",
            get(get_community_metrics),
        )
        .route("/api/repos/{repo_id}/cycles", get(get_cycles))
        .route(
            "/api/repos/{repo_id}/module-coupling",
            get(get_module_coupling),
        )
        // LLM provider configuration endpoints
        .route("/api/llm/config", get(get_llm_config))
        .route("/api/llm/config", post(set_llm_config))
        // Graph API v1: embedding-based visualization endpoints
        .route("/api/v1/graph/overview/{repo_id}", get(graph_v1_overview))
        .route(
            "/api/v1/graph/nodes/{repo_id}/{node_id}",
            get(graph_v1_node),
        )
        .route("/api/v1/graph/search/{repo_id}", get(graph_v1_search))
        .layer(axum::middleware::from_fn(local_only))
        .layer(axum::middleware::from_fn(latency_logger))
        .with_state(state)
}
