//! Myceliums MCP server: exposes the code knowledge graph to AI agents.
//!
//! Layout:
//! * `params` / `output` — tool input and output types (re-exported here);
//!   `format` renders outputs to the text agents read.
//! * `tools` — the `#[tool]` handlers, one module per tool group, merged into
//!   one router.
//! * `server` — `ServerHandler`, stdio and HTTP transports; `resources` —
//!   `repository://` resources.
//! * `repo` / `errors` / `index_status` — shared helpers.

mod errors;
mod format;
pub mod http_security;
mod index_status;
mod output;
mod params;
mod repo;
mod resources;
mod server;
#[cfg(feature = "test-support")]
mod test_support;
mod tools;

pub use http_security::McpHttpOptions;
pub use output::*;
pub use params::*;
pub use server::{run_mcp_http_server, run_mcp_server};

use myceliums_core::GraphService;
use rmcp::handler::server::tool::ToolRouter;
use std::sync::Arc;

pub struct MyceliumsMcp {
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
    /// Owns store lifecycle, repo resolution, the per-repo graph cache, and
    /// graph assembly so handlers stay thin transport adapters. Shared by
    /// every HTTP session, so the cache is per process, not per session.
    service: Arc<GraphService>,
}

impl Default for MyceliumsMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl MyceliumsMcp {
    pub fn new() -> Self {
        Self::with_service(Arc::new(GraphService::new(repo::data_dir())))
    }

    /// A server that reads through an existing (shared) [`GraphService`].
    pub(crate) fn with_service(service: Arc<GraphService>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            service,
        }
    }
}
