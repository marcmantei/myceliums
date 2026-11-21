//! The `myc serve` dashboard: a loopback-only HTTP server with a
//! single-page graph visualization and its JSON API.
//!
//! Every read of a repository's index goes through
//! [`myceliums_core::GraphService`]; requests forged by web pages (DNS
//! rebinding, cross-origin) are refused before any handler runs.

#![warn(missing_docs)]

use crate::routes::build_router;
use crate::state::{data_dir, AppState};
use anyhow::Result;
use std::sync::Arc;

mod error;
mod handlers;
mod routes;
mod security;
mod state;
#[cfg(test)]
mod test_support;

/// Start the HTTP server on the given port.
///
/// If `repo_id` is provided, it will be pre-selected in the UI.
pub async fn start_server(port: u16, repo_id: Option<String>) -> Result<()> {
    let state = Arc::new(AppState::new(data_dir(), repo_id));
    let app = build_router(state);

    // Served on loopback only; the SPA is same-origin so no CORS layer is added.
    // A wildcard CORS layer here previously let any website read the API graph and
    // the (now redacted) LLM config.
    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port)).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
