//! Myceliums MCP server: exposes the code knowledge graph to AI agents.
//!
//! Layout:
//! * `params` / `output` — tool input and output types (re-exported here);
//!   `format` renders outputs to the text agents read.
//! * `tools` — the `#[tool]` handlers, one module per tool group, merged into
//!   one router.
//! * `server` — `ServerHandler`, stdio and HTTP transports; `resources` —
//!   `repository://` resources.
//! * `errors` — the tool error contract (`isError` results vs JSON-RPC
//!   errors); `output::Page` — the shared output budget (limit/offset and
//!   truncation notices); `index_status` — the freshness line on every graph
//!   result; `format::untrusted` — framing of indexed text.
//! * `repo` — repository and symbol resolution; `navigation` — the
//!   caller/callee/dependents/call-chain walks behind the core tools.
//! * `toolsets` — which tools a server exposes (`core` by default).

mod errors;
mod format;
pub mod http_security;
mod index_status;
mod navigation;
mod output;
mod params;
mod repo;
mod resources;
mod server;
#[cfg(feature = "test-support")]
mod test_support;
mod tools;
mod toolsets;

pub use http_security::McpHttpOptions;
pub use output::*;
pub use params::*;
pub use server::{run_mcp_http_server, run_mcp_server};
pub use toolsets::{Toolset, Toolsets};

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
