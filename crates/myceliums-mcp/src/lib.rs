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
//! * `observe` — one tracing span per tool call, at the dispatch layer;
//!   `stats` — the local call statistics behind `server_stats` and
//!   `--stats-file`.
//! * `analysis` — the full re-analysis behind `analyze`: off the runtime,
//!   built beside the live index and swapped in, with progress
//!   notifications.

mod analysis;
mod errors;
mod format;
pub mod http_security;
mod index_status;
mod navigation;
mod observe;
mod output;
mod params;
mod repo;
mod resources;
mod server;
mod stats;
#[cfg(feature = "test-support")]
mod test_support;
mod tools;
mod toolsets;

pub use http_security::McpHttpOptions;
pub use observe::Observed;
pub use output::*;
pub use params::*;
pub use server::{run_mcp_http_server, run_mcp_server, ServeOptions};
pub use stats::{StatsSnapshot, ToolSnapshot};
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
    /// Call statistics, filled by [`observe::Observed`] and read by the
    /// `server_stats` tool. Shared by every HTTP session.
    stats: Arc<stats::CallStats>,
}

impl Default for MyceliumsMcp {
    fn default() -> Self {
        Self::new()
    }
}

impl MyceliumsMcp {
    pub fn new() -> Self {
        Self::with_service(Arc::new(repo::graph_service()))
    }

    /// A server that reads through an existing (shared) [`GraphService`].
    pub(crate) fn with_service(service: Arc<GraphService>) -> Self {
        Self::with_shared(service, Arc::default())
    }

    /// A server that shares the graph service and the call statistics.
    pub(crate) fn with_shared(service: Arc<GraphService>, stats: Arc<stats::CallStats>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            service,
            stats,
        }
    }

    /// The rmcp router for `toolsets`, wrapped so that every tool call is
    /// traced and counted.
    pub(crate) fn into_service(
        self,
        toolsets: &Toolsets,
    ) -> Observed<rmcp::handler::server::router::Router<Self>> {
        let stats = self.stats.clone();
        Observed::new(
            rmcp::handler::server::router::Router::new(self)
                .with_tools(Self::tool_router_for(toolsets)),
            stats,
        )
    }
}
