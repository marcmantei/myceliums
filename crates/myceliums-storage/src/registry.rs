use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::models::RepoInfo;

/// A JSON-backed registry of analyzed repositories, keyed by repo id.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RepoRegistry {
    /// Known repositories, keyed by their stable id.
    pub repos: HashMap<String, RepoInfo>,
    #[serde(skip)]
    path: PathBuf,
}

impl RepoRegistry {
    /// Loads the registry from `path`, returning an empty registry if the file
    /// does not yet exist.
    pub fn load(path: &Path) -> Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path).context("Failed to read repos.json")?;
            let mut registry: RepoRegistry =
                serde_json::from_str(&content).context("Failed to parse repos.json")?;
            registry.path = path.to_path_buf();
            Ok(registry)
        } else {
            Ok(Self {
                repos: HashMap::new(),
                path: path.to_path_buf(),
            })
        }
    }

    /// Persists the registry to its backing JSON file, creating parent
    /// directories as needed.
    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    /// Inserts or replaces a repository entry.
    pub fn register(&mut self, info: RepoInfo) {
        self.repos.insert(info.id.clone(), info);
    }

    /// Removes a repository by id, returning its entry if present.
    pub fn remove(&mut self, repo_id: &str) -> Option<RepoInfo> {
        self.repos.remove(repo_id)
    }

    /// Looks up a repository by its id.
    pub fn get(&self, repo_id: &str) -> Option<&RepoInfo> {
        self.repos.get(repo_id)
    }

    /// Finds the first repository whose path matches `path`.
    pub fn find_by_path(&self, path: &str) -> Option<&RepoInfo> {
        self.repos.values().find(|r| r.path == path)
    }

    /// Lists all repositories, sorted by name.
    pub fn list(&self) -> Vec<&RepoInfo> {
        let mut repos: Vec<&RepoInfo> = self.repos.values().collect();
        repos.sort_by(|a, b| a.name.cmp(&b.name));
        repos
    }

    /// Returns the on-disk database directory for a repository.
    ///
    /// This joins `repo_id` without checking it, so it is only for ids the
    /// caller produced itself (e.g. the id `analyze` derives from a project
    /// path). Any id that came from a user, a tool call or a URL must go
    /// through [`RepoRegistry::registered_db_path`] instead.
    pub fn repo_db_path(data_dir: &Path, repo_id: &str) -> PathBuf {
        data_dir.join("data").join(repo_id)
    }

    /// Checks that `repo_id` is safe to use as one path component.
    ///
    /// Repo ids are derived from a directory name plus a hash, so they can
    /// hold spaces and non-ASCII letters. What they can never hold is anything
    /// that changes which directory `data/<repo_id>` points at: path
    /// separators, `.`/`..`, a drive prefix, NUL or other control characters.
    pub fn validate_repo_id(repo_id: &str) -> Result<(), RepoIdError> {
        let invalid = |reason: &'static str| {
            Err(RepoIdError::Invalid {
                id: repo_id.to_string(),
                reason,
            })
        };
        if repo_id.is_empty() {
            return invalid("it is empty");
        }
        if repo_id.len() > MAX_REPO_ID_BYTES {
            return invalid("it is longer than 255 bytes");
        }
        if repo_id == "." || repo_id == ".." {
            return invalid("it is a relative directory reference");
        }
        if repo_id
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
        {
            return invalid("it contains a path separator, ':' or a control character");
        }
        Ok(())
    }

    /// The single choke point for caller-supplied repo ids.
    ///
    /// Returns the registry entry when `repo_id` passes
    /// [`RepoRegistry::validate_repo_id`] **and** is registered. An unknown id
    /// yields [`RepoIdError::Unknown`], whose message lists the known ids so
    /// the caller can correct the request.
    pub fn require(&self, repo_id: &str) -> Result<&RepoInfo, RepoIdError> {
        Self::validate_repo_id(repo_id)?;
        self.repos.get(repo_id).ok_or_else(|| RepoIdError::Unknown {
            id: repo_id.to_string(),
            known: self.list().iter().map(|r| r.id.clone()).collect(),
        })
    }

    /// Database directory for a caller-supplied repo id, after
    /// [`RepoRegistry::require`] has accepted it.
    ///
    /// Read paths use this instead of [`RepoRegistry::repo_db_path`], so an
    /// unknown or malformed id is rejected before anything touches the disk.
    pub fn registered_db_path(
        &self,
        data_dir: &Path,
        repo_id: &str,
    ) -> Result<PathBuf, RepoIdError> {
        self.require(repo_id)?;
        Ok(Self::repo_db_path(data_dir, repo_id))
    }
}

/// Upper bound for a repo id; matches the common file-name limit.
const MAX_REPO_ID_BYTES: usize = 255;

/// Why a caller-supplied repo id was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RepoIdError {
    /// The id can not name a single directory below the data dir.
    #[error("invalid repo_id {id:?}: {reason}")]
    Invalid {
        /// The rejected id.
        id: String,
        /// What is wrong with it.
        reason: &'static str,
    },
    /// The id is well-formed but no repository with that id is registered.
    #[error("unknown repo_id {id:?}. {}", known_ids_hint(known))]
    Unknown {
        /// The rejected id.
        id: String,
        /// The ids that are registered, sorted by repository name.
        known: Vec<String>,
    },
}

fn known_ids_hint(known: &[String]) -> String {
    if known.is_empty() {
        "No repositories are registered yet: analyze a project first \
         (MCP tool `analyze` or `myc analyze <path>`)."
            .to_string()
    } else {
        format!(
            "Known repo ids: {}. Use one of these (the MCP tool `list_repos` or `myc list --json` shows them).",
            known.join(", ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry_with(ids: &[&str]) -> RepoRegistry {
        let mut registry = RepoRegistry::default();
        for id in ids {
            registry.register(RepoInfo {
                id: id.to_string(),
                name: id.to_string(),
                path: format!("/src/{id}"),
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
    fn accepts_ids_that_analyze_produces() {
        for id in [
            "myceliums-1a2b3c4d",
            "My Project-1a2b3c4d",
            "größe-ü-1a2b3c4d",
            "proj_🦀-1a2b3c4d",
            ".dotfiles-1a2b3c4d",
        ] {
            assert_eq!(RepoRegistry::validate_repo_id(id), Ok(()), "{id}");
        }
    }

    #[test]
    fn rejects_ids_that_escape_the_data_dir() {
        for id in [
            "",
            ".",
            "..",
            "../etc",
            "a/b",
            "..\\x",
            "C:evil",
            "nul\0byte",
            "line\nbreak",
        ] {
            assert!(
                matches!(
                    RepoRegistry::validate_repo_id(id),
                    Err(RepoIdError::Invalid { .. })
                ),
                "{id:?} must be rejected"
            );
        }
        let long = "a".repeat(256);
        assert!(RepoRegistry::validate_repo_id(&long).is_err());
    }

    #[test]
    fn require_lists_known_ids_for_unknown_id() {
        let registry = registry_with(&["beta-2", "alpha-1"]);
        let err = registry.require("nope").unwrap_err();
        assert_eq!(
            err,
            RepoIdError::Unknown {
                id: "nope".into(),
                known: vec!["alpha-1".into(), "beta-2".into()],
            }
        );
        let msg = err.to_string();
        assert!(msg.contains("unknown repo_id \"nope\""), "{msg}");
        assert!(msg.contains("alpha-1, beta-2"), "{msg}");
        assert!(msg.contains("list_repos"), "{msg}");
    }

    #[test]
    fn require_on_empty_registry_says_to_analyze_first() {
        let msg = RepoRegistry::default()
            .require("x")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("No repositories are registered yet"), "{msg}");
    }

    #[test]
    fn require_checks_charset_before_membership() {
        // Even a registry entry with a bad id must not be turned into a path.
        let registry = registry_with(&["../evil"]);
        assert!(matches!(
            registry.require("../evil"),
            Err(RepoIdError::Invalid { .. })
        ));
    }

    #[test]
    fn registered_db_path_joins_only_known_ids() {
        let registry = registry_with(&["known-1"]);
        let data = Path::new("/data-home");
        assert_eq!(
            registry.registered_db_path(data, "known-1").unwrap(),
            PathBuf::from("/data-home/data/known-1")
        );
        assert!(registry.registered_db_path(data, "../known-1").is_err());
        assert!(registry.registered_db_path(data, "other").is_err());
    }
}
