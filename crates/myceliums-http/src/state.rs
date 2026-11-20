//! Shared server state and access to the repository registry and stores.

use anyhow::Result;
use myceliums_storage::{RepoRegistry, Store};
use std::path::PathBuf;

pub(crate) struct AppState {
    pub(crate) data_dir: PathBuf,
    pub(crate) registry_path: PathBuf,
    /// Pre-selected repo_id (if the user passed --repo)
    pub(crate) default_repo_id: Option<String>,
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

pub(crate) fn registry_path() -> PathBuf {
    data_dir().join("repos.json")
}

/// Opens the store for a `repo_id` taken from the URL.
///
/// The id goes through the shared choke point
/// ([`RepoRegistry::registered_db_path`]): malformed or unregistered ids are
/// rejected (handlers answer 404) before a path is built, so a request can
/// neither escape the data dir nor create a directory for a made-up id.
pub(crate) async fn open_store(state: &AppState, repo_id: &str) -> Result<Store> {
    let db_path =
        RepoRegistry::load(&state.registry_path)?.registered_db_path(&state.data_dir, repo_id)?;
    Store::open(&db_path, repo_id).await
}
