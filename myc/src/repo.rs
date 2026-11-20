//! The myceliums data directory and resolution of a repository argument to a registered repo.

use anyhow::Result;
use myceliums_storage::{RepoInfo, RepoRegistry};
use std::path::PathBuf;

/// The myceliums data home. `MYCELIUMS_DATA_DIR` overrides `~/.myceliums`,
/// the same test/dev seam the MCP server honours, so the CLI and hooks read
/// the index the server in the same environment reads.
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

/// Resolves a user-supplied repository reference (registered id or project
/// path) to a registered repo id.
///
/// Ids go through [`RepoRegistry::require`], the shared choke point, so a
/// malformed id is never turned into a path and an unknown one yields an
/// error that lists the known ids.
pub(crate) fn lookup_repo_id(registry: &RepoRegistry, repo: &str) -> Result<String> {
    match registry.require(repo) {
        Ok(info) => Ok(info.id.clone()),
        Err(err) => match registry.find_by_path(repo) {
            Some(info) => {
                RepoRegistry::validate_registered_repo_id(&info.id)?;
                Ok(info.id.clone())
            }
            None => Err(anyhow::anyhow!(
                "{err} A registered project path is accepted too."
            )),
        },
    }
}

pub(crate) fn resolve_repo(repo: &str) -> Result<(String, RepoInfo)> {
    let registry = RepoRegistry::load(&registry_path())?;
    let repo_id = lookup_repo_id(&registry, repo)?;
    let info = registry
        .get(&repo_id)
        .cloned()
        .expect("lookup_repo_id returns registered ids");
    Ok((repo_id, info))
}

pub(crate) fn resolve_repo_or_latest(repo: Option<&str>) -> Result<(String, RepoInfo)> {
    if let Some(r) = repo {
        return resolve_repo(r);
    }

    let registry = RepoRegistry::load(&registry_path())?;

    // Try to find a repo that contains the current working directory
    if let Ok(cwd) = std::env::current_dir() {
        if let Ok(cwd_canonical) = cwd.canonicalize() {
            for repo_info in registry.list() {
                let repo_path = PathBuf::from(&repo_info.path);
                if let Ok(repo_canonical) = repo_path.canonicalize() {
                    // Check if cwd is within repo_path or is repo_path itself
                    if cwd_canonical.starts_with(&repo_canonical) {
                        eprintln!("Resolved repository: {} ({})", repo_info.name, repo_info.id);
                        return Ok((repo_info.id.clone(), repo_info.clone()));
                    }
                }
            }
        }
    }

    // Fall back to the latest repo if cwd is not in any indexed repo
    let repos = registry.list();
    if let Some(latest) = repos.last() {
        eprintln!(
            "Using latest repository as fallback: {} ({})",
            latest.name, latest.id
        );
        return Ok((latest.id.clone(), (*latest).clone()));
    }
    anyhow::bail!("No repositories analyzed yet. Run: myc analyze <path>");
}

#[cfg(test)]
mod tests {
    use super::*;

    use myceliums_storage::{RepoInfo, RepoRegistry};

    fn registry_with(ids: &[(&str, &str)]) -> RepoRegistry {
        let mut registry = RepoRegistry::default();
        for (id, path) in ids {
            registry.register(RepoInfo {
                id: id.to_string(),
                name: id.to_string(),
                path: path.to_string(),
                analyzed_at: String::new(),
                symbol_count: 0,
                file_count: 0,
                analyzed_commit: None,
                vector_geometry_version: 0,
            });
        }
        registry
    }

    #[test]
    fn lookup_repo_id_accepts_registered_id_or_path() {
        let registry = registry_with(&[("proj-1a2b", "/src/proj")]);
        assert_eq!(lookup_repo_id(&registry, "proj-1a2b").unwrap(), "proj-1a2b");
        assert_eq!(lookup_repo_id(&registry, "/src/proj").unwrap(), "proj-1a2b");
    }

    #[test]
    fn lookup_repo_id_rejects_unknown_and_traversal_with_known_ids() {
        let registry = registry_with(&[("proj-1a2b", "/src/proj")]);
        for bad in ["nope", "../proj-1a2b", "/elsewhere"] {
            let msg = lookup_repo_id(&registry, bad).unwrap_err().to_string();
            assert!(
                msg.contains("proj-1a2b") || msg.contains("invalid"),
                "{msg}"
            );
        }
        let msg = lookup_repo_id(&registry, "nope").unwrap_err().to_string();
        assert!(msg.contains("Known repo ids: proj-1a2b"), "{msg}");
    }

    #[test]
    fn lookup_repo_id_never_returns_a_path_like_registered_id() {
        // A tampered registry entry must not reach `remove_dir_all`.
        let registry = registry_with(&[("../../victim", "/src/victim")]);
        assert!(lookup_repo_id(&registry, "/src/victim").is_err());
        assert!(lookup_repo_id(&registry, "../../victim").is_err());
    }
}
