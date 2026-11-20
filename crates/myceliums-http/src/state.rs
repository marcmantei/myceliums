//! Shared server state and access to the repository registry and stores.

use myceliums_core::{GlobalConfig, GraphCacheLimits, GraphService};
use std::path::PathBuf;

pub(crate) struct AppState {
    /// The registry file, for listing repositories.
    pub(crate) registry_path: PathBuf,
    /// Pre-selected repo_id (if the user passed --repo)
    pub(crate) default_repo_id: Option<String>,
    /// Every read of a repository's index goes through this service.
    ///
    /// A `repo_id` taken from the URL passes the registry's choke point
    /// ([`myceliums_storage::RepoRegistry::registered_db_path`]): malformed
    /// or unregistered ids are rejected (handlers answer 404) before a path
    /// is built, and the index is opened with `Store::open_existing`, so a
    /// request can neither escape the data dir nor create a directory. Graph
    /// tables stay cached across requests while the index on disk is
    /// unchanged.
    pub(crate) graph: GraphService,
}

impl AppState {
    /// State over the data home `data_dir`, with the graph cache bounded by
    /// the `[mcp]` `graph_cache_*` keys of its `config.toml` (as in the MCP
    /// server; defaults when the file is missing or unreadable).
    pub(crate) fn new(data_dir: PathBuf, default_repo_id: Option<String>) -> Self {
        let limits = match GlobalConfig::load(&data_dir) {
            Ok(config) => GraphCacheLimits::from_config(&config),
            Err(e) => {
                tracing::warn!(
                    "cannot read config.toml, using the default graph cache limits: {e:#}"
                );
                GraphCacheLimits::default()
            }
        };
        Self {
            registry_path: data_dir.join("repos.json"),
            default_repo_id,
            graph: GraphService::with_limits(data_dir, limits),
        }
    }
}

/// The myceliums data home. `MYCELIUMS_DATA_DIR` overrides `~/.myceliums`,
/// as in the CLI and the MCP server, so `myc serve` reads the same index.
pub(crate) fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("MYCELIUMS_DATA_DIR") {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .expect("Could not find home directory")
        .join(".myceliums")
}
