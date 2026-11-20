//! `myc serve`.

use crate::repo::{registry_path, resolve_repo};
use anyhow::Result;
use myceliums_core::analyzer::{self};
use myceliums_storage::RepoRegistry;

pub(crate) async fn cmd_serve(port: u16, repo: Option<String>) -> Result<()> {
    // Resolve repo_id: explicit flag > auto-detect from cwd > None (show all repos)
    let repo_id = if let Some(r) = repo {
        let (id, _) = resolve_repo(&r)?;
        Some(id)
    } else {
        // Try to auto-detect from current working directory
        let cwd = std::env::current_dir()?;
        let cwd = std::fs::canonicalize(&cwd).unwrap_or(cwd);
        let candidate_id = analyzer::repo_id_from_path(&cwd);
        let registry = RepoRegistry::load(&registry_path())?;
        if registry.get(&candidate_id).is_some() {
            Some(candidate_id)
        } else {
            None
        }
    };

    if let Some(ref id) = repo_id {
        println!("Serving repo: {}", id);
    }
    println!("Visualization ready at http://localhost:{}", port);

    myceliums_http::start_server(port, repo_id).await
}
