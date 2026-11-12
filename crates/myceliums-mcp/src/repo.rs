//! Data-home paths and repository/symbol resolution shared by the tools.

use crate::errors::repo_id_error;
use crate::MyceliumsMcp;
use myceliums_storage::RepoRegistry;
use std::path::PathBuf;

pub(crate) fn data_dir() -> PathBuf {
    // `MYCELIUMS_DATA_DIR` overrides the data home so the server can be pointed
    // at an isolated directory. This is a **test/dev-only seam** (the
    // integration harness sets it to a throwaway tempdir) and also serves
    // multi-tenant setups; it is intentionally not a security boundary — the
    // path is taken verbatim and not validated, so production deployments must
    // not expose it to untrusted input.
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

/// Database directory for a caller-supplied repo id.
///
/// This is the MCP side of the repo-id choke point
/// ([`RepoRegistry::registered_db_path`]): malformed or unregistered ids are
/// rejected with an error that lists the known ids, before any path is built
/// or any directory is created.
pub(crate) fn registered_db_path(repo_id: &str) -> Result<PathBuf, rmcp::ErrorData> {
    RepoRegistry::load(&registry_path())
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
        .registered_db_path(&data_dir(), repo_id)
        .map_err(repo_id_error)
}

impl MyceliumsMcp {
    pub(crate) fn resolve_symbol<'a>(
        symbols: &'a [myceliums_storage::CodeSymbol],
        name: &str,
    ) -> Result<&'a myceliums_storage::CodeSymbol, rmcp::ErrorData> {
        // Try qualified_name first
        let matches: Vec<_> = symbols
            .iter()
            .filter(|s| s.qualified_name == name)
            .collect();
        if matches.len() == 1 {
            return Ok(matches[0]);
        }
        if matches.is_empty() {
            // Fall back to name
            let by_name: Vec<_> = symbols.iter().filter(|s| s.name == name).collect();
            match by_name.len() {
                0 => Err(rmcp::ErrorData::internal_error(
                    format!("Symbol not found: {}", name),
                    None,
                )),
                1 => Ok(by_name[0]),
                _ => {
                    let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                    Err(rmcp::ErrorData::internal_error(
                        format!("Ambiguous symbol '{}'. Matches: {}", name, names.join(", ")),
                        None,
                    ))
                }
            }
        } else {
            // Multiple qualified_name matches (unlikely but handle it)
            Ok(matches[0])
        }
    }

    /// Turns an optional caller-supplied repo id into a registered one.
    ///
    /// An explicit id must pass [`RepoRegistry::require`]; without one, the
    /// repo containing the working directory (or else the most recently
    /// analyzed repo) is used.
    pub(crate) fn resolve_repo_id(&self, repo_id: Option<&str>) -> Result<String, rmcp::ErrorData> {
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        if let Some(id) = repo_id {
            registry.require(id).map_err(repo_id_error)?;
            return Ok(id.to_string());
        }

        // Try to match current working directory to a registered repo
        if let Ok(cwd) = std::env::current_dir() {
            if let Ok(abs_cwd) = std::fs::canonicalize(&cwd) {
                let cwd_str = abs_cwd.to_string_lossy();
                for repo in registry.list().iter().rev() {
                    if let Ok(repo_path) = std::fs::canonicalize(&repo.path) {
                        let repo_str = repo_path.to_string_lossy();
                        if cwd_str.starts_with(repo_str.as_ref()) {
                            return Ok(repo.id.clone());
                        }
                    }
                }
            }
        }

        // Fallback: most recently analyzed repo
        let repos = registry.list();
        repos.last().map(|r| r.id.clone()).ok_or_else(|| {
            rmcp::ErrorData::internal_error("No repositories analyzed yet".to_string(), None)
        })
    }
}
