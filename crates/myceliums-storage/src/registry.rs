use crate::error::{Context, Result};
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
    ///
    /// A legacy entry for the same project path whose id starts with `.`
    /// (ids made before such names were sanitized) is replaced by this one,
    /// so a re-analysis does not leave a stale twin behind; its index
    /// directory becomes an orphan (`myc clean --orphans`).
    pub fn register(&mut self, info: RepoInfo) {
        self.repos
            .retain(|id, old| !(id.starts_with('.') && old.path == info.path && *id != info.id));
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

    /// The registered repository whose directory contains `path` (or is
    /// `path`), preferring the deepest one when indexed repositories nest.
    ///
    /// Both sides are canonicalized, so symlinked paths and `..` segments
    /// match. Never falls back to some other repository: `None` means `path`
    /// lies outside every indexed project.
    pub fn find_containing(&self, path: &Path) -> Option<&RepoInfo> {
        let target = std::fs::canonicalize(path).ok()?;
        self.repos
            .values()
            .filter_map(|info| {
                let root = std::fs::canonicalize(&info.path).ok()?;
                target
                    .starts_with(&root)
                    .then(|| (root.components().count(), info))
            })
            .max_by(|(depth_a, a), (depth_b, b)| depth_a.cmp(depth_b).then(b.id.cmp(&a.id)))
            .map(|(_, info)| info)
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
    /// Repo ids are derived from a directory name plus a hash, so they hold
    /// whatever a folder name on this platform can hold: spaces, non-ASCII
    /// letters, and on Unix also `:` and `\` (Finder shows `/` as `:`, so a
    /// folder `Plan 2024/25` is stored as `Plan 2024:25`). What an id can never
    /// hold is anything that changes which directory `data/<repo_id>` points
    /// at: this platform's path separators, `.`/`..`, NUL, and on Windows `:`
    /// (drive prefixes and alternate data streams).
    ///
    /// New ids also never start with `.`: `data/` keeps its own entries
    /// (`.locks`, `.staging`) under dot names, and a repository must never be
    /// mistaken for one. [`RepoRegistry::sanitize_repo_name`] guarantees
    /// that. Ids registered before that rule are still accepted by
    /// [`RepoRegistry::require`] and [`RepoRegistry::validate_registered_repo_id`].
    pub fn validate_repo_id(repo_id: &str) -> Result<(), RepoIdError> {
        Self::validate_registered_repo_id(repo_id)?;
        if repo_id.starts_with('.') {
            return Err(RepoIdError::Invalid {
                id: repo_id.to_string(),
                reason: "it starts with '.'",
            });
        }
        Ok(())
    }

    /// [`RepoRegistry::validate_repo_id`] without the leading-dot rule, for
    /// ids read from the registry: those registered before the rule stay
    /// usable. Still rejects anything that leaves `data/`.
    pub fn validate_registered_repo_id(repo_id: &str) -> Result<(), RepoIdError> {
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
        if repo_id.chars().any(is_forbidden_id_char) {
            return invalid(FORBIDDEN_CHARS_REASON);
        }
        Ok(())
    }

    /// Turns a folder name into the name part of a repo id that
    /// [`RepoRegistry::validate_repo_id`] accepts.
    ///
    /// Characters that may not appear in an id are replaced by `_`, and the
    /// result is cut (on a character boundary) so that `<name>-<8 hex>` stays
    /// within the 255-byte limit. Deterministic, and a no-op for every name
    /// that already produced a valid id.
    pub fn sanitize_repo_name(name: &str) -> String {
        const SUFFIX_BYTES: usize = 9; // "-" + 8 hex digits
        let mut out = String::with_capacity(name.len());
        for (i, c) in name.chars().enumerate() {
            // A leading '.' would make the id look like one of data/'s own
            // dot entries (see `validate_repo_id`).
            let c = if is_forbidden_id_char(c) || (i == 0 && c == '.') {
                '_'
            } else {
                c
            };
            if out.len() + c.len_utf8() > MAX_REPO_ID_BYTES - SUFFIX_BYTES {
                break;
            }
            out.push(c);
        }
        out
    }

    /// The single choke point for caller-supplied repo ids.
    ///
    /// Returns the registry entry when `repo_id` passes
    /// [`RepoRegistry::validate_repo_id`] **and** is registered. An unknown id
    /// yields [`RepoIdError::Unknown`], whose message lists the known ids so
    /// the caller can correct the request.
    pub fn require(&self, repo_id: &str) -> Result<&RepoInfo, RepoIdError> {
        Self::validate_registered_repo_id(repo_id)?;
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

/// A character that would let an id name something other than one directory
/// below `data/` on this platform.
fn is_forbidden_id_char(c: char) -> bool {
    c == '\0' || std::path::is_separator(c) || (cfg!(windows) && c == ':')
}

#[cfg(windows)]
const FORBIDDEN_CHARS_REASON: &str = "it contains a path separator, ':' or NUL";
#[cfg(not(windows))]
const FORBIDDEN_CHARS_REASON: &str = "it contains a path separator or NUL";

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
            "_dotfiles-1a2b3c4d",
        ] {
            assert_eq!(RepoRegistry::validate_repo_id(id), Ok(()), "{id}");
        }
    }

    #[test]
    fn dot_ids_are_refused_for_new_indexes_but_legacy_ones_stay_readable() {
        for id in [
            ".dotfiles-1a2b3c4d",
            ".build-123-1a2b3c4d",
            ".locks",
            ".staging",
        ] {
            assert!(
                matches!(
                    RepoRegistry::validate_repo_id(id),
                    Err(RepoIdError::Invalid { .. })
                ),
                "{id}"
            );
        }
        assert_eq!(RepoRegistry::sanitize_repo_name(".build-123"), "_build-123");
        assert_eq!(RepoRegistry::sanitize_repo_name(".."), "_.");
        assert_eq!(RepoRegistry::sanitize_repo_name("a.b"), "a.b");

        let legacy = registry_with(&[".dotfiles-1a2b3c4d"]);
        assert!(legacy.require(".dotfiles-1a2b3c4d").is_ok());
        assert_eq!(
            legacy
                .registered_db_path(Path::new("/d"), ".dotfiles-1a2b3c4d")
                .unwrap(),
            PathBuf::from("/d/data/.dotfiles-1a2b3c4d")
        );
        assert!(legacy.require("..").is_err());
    }

    #[test]
    fn re_registering_a_project_replaces_its_legacy_dot_id() {
        let mut registry = registry_with(&[".dotfiles-1a2b3c4d", ".other-1a2b3c4d"]);
        let mut info = registry.get(".dotfiles-1a2b3c4d").unwrap().clone();
        info.id = "_dotfiles-1a2b3c4d".to_string();
        registry.register(info);
        let ids: Vec<&str> = registry.list().iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids.len(), 2, "{ids:?}");
        assert!(ids.contains(&"_dotfiles-1a2b3c4d") && ids.contains(&".other-1a2b3c4d"));
    }

    #[test]
    fn rejects_ids_that_escape_the_data_dir() {
        let mut bad = vec!["", ".", "..", "../etc", "a/b", "nul\0byte"];
        if cfg!(windows) {
            bad.extend(["..\\x", "C:evil", "stream:x"]);
        }
        for id in bad {
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

    /// macOS stores a folder typed as `Plan 2024/25` as `Plan 2024:25`;
    /// `analyze` derives the id from that name, so such ids (and legacy
    /// registrations) must stay usable on Unix.
    #[cfg(not(windows))]
    #[test]
    fn unix_accepts_colon_and_backslash_from_folder_names() {
        for id in [
            "Plan 2024:25-1a2b3c4d",
            "a\\b-1a2b3c4d",
            "tab\there-1a2b3c4d",
        ] {
            assert_eq!(RepoRegistry::validate_repo_id(id), Ok(()), "{id:?}");
        }
        let legacy = registry_with(&["Plan 2024:25-1a2b3c4d"]);
        assert!(legacy.require("Plan 2024:25-1a2b3c4d").is_ok());
        assert_eq!(
            legacy
                .registered_db_path(Path::new("/d"), "Plan 2024:25-1a2b3c4d")
                .unwrap(),
            PathBuf::from("/d/data/Plan 2024:25-1a2b3c4d")
        );
    }

    #[test]
    fn sanitized_names_always_form_valid_ids() {
        let long = "ä".repeat(200);
        let names = [
            "plain",
            "Plan 2024:25",
            "a\\b",
            "nul\0in name",
            "",
            ".",
            "..",
            ".build-123",
            long.as_str(),
        ];
        for name in names {
            let id = format!("{}-1a2b3c4d", RepoRegistry::sanitize_repo_name(name));
            assert_eq!(
                RepoRegistry::validate_repo_id(&id),
                Ok(()),
                "{name:?} -> {id:?}"
            );
        }
        // Names that were already valid are left alone.
        assert_eq!(RepoRegistry::sanitize_repo_name("My Repo ü"), "My Repo ü");
        assert_eq!(RepoRegistry::sanitize_repo_name("nul\0x"), "nul_x");
        assert!(RepoRegistry::sanitize_repo_name(&long).len() <= 255 - 9);
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

    fn repo_at(id: &str, path: &Path) -> RepoInfo {
        RepoInfo {
            id: id.to_string(),
            name: id.to_string(),
            path: path.to_string_lossy().into_owned(),
            analyzed_at: String::new(),
            symbol_count: 0,
            file_count: 0,
            analyzed_commit: None,
            vector_geometry_version: 0,
        }
    }

    #[test]
    fn find_containing_prefers_the_deepest_repo_and_never_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let outer = dir.path().join("outer");
        let inner = outer.join("vendor").join("inner");
        let other = dir.path().join("other");
        for p in [&inner, &other] {
            std::fs::create_dir_all(p).unwrap();
        }
        let mut registry = RepoRegistry::default();
        registry.register(repo_at("outer-1", &outer));
        registry.register(repo_at("inner-1", &inner));

        let id = |p: &Path| registry.find_containing(p).map(|r| r.id.clone());
        assert_eq!(id(&outer).as_deref(), Some("outer-1"));
        assert_eq!(id(&outer.join("vendor")).as_deref(), Some("outer-1"));
        assert_eq!(id(&inner).as_deref(), Some("inner-1"));
        assert_eq!(
            id(&inner.join("..").join("inner")).as_deref(),
            Some("inner-1")
        );
        assert_eq!(id(&other), None, "no fallback to an unrelated repo");
        assert_eq!(id(&dir.path().join("missing")), None);
    }
}
