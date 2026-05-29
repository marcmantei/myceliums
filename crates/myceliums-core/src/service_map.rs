//! Service-to-code mapping — assign human-readable service names to communities.
//!
//! Persists community→service name mappings to disk so architecture diagrams
//! and other features can use meaningful labels.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A single community-to-service mapping.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceEntry {
    pub community_label: String,
    pub service_name: String,
}

/// All service mappings for a repository.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceMapping {
    pub repo_id: String,
    pub mappings: Vec<ServiceEntry>,
}

/// Return the path for service mappings: `~/.myceliums/services/{repo_id}.json`
pub fn service_map_path(data_dir: &Path, repo_id: &str) -> PathBuf {
    data_dir.join("services").join(format!("{}.json", repo_id))
}

/// Save or update a service mapping for a community.
///
/// If the community already has a mapping, it is overwritten.
pub fn save_service_mapping(
    data_dir: &Path,
    repo_id: &str,
    community_label: &str,
    service_name: &str,
) -> anyhow::Result<()> {
    let mut mapping = load_service_mappings(data_dir, repo_id)?;

    // Upsert
    if let Some(entry) = mapping
        .mappings
        .iter_mut()
        .find(|e| e.community_label == community_label)
    {
        entry.service_name = service_name.to_string();
    } else {
        mapping.mappings.push(ServiceEntry {
            community_label: community_label.to_string(),
            service_name: service_name.to_string(),
        });
    }

    let path = service_map_path(data_dir, repo_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&mapping)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load all service mappings for a repository.
pub fn load_service_mappings(data_dir: &Path, repo_id: &str) -> anyhow::Result<ServiceMapping> {
    let path = service_map_path(data_dir, repo_id);
    if !path.exists() {
        return Ok(ServiceMapping {
            repo_id: repo_id.to_string(),
            mappings: Vec::new(),
        });
    }
    let json = std::fs::read_to_string(&path)?;
    let mapping: ServiceMapping = serde_json::from_str(&json)?;
    Ok(mapping)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_save_and_load() {
        let dir = TempDir::new().unwrap();
        save_service_mapping(dir.path(), "repo1", "community_a", "Auth Service").unwrap();

        let mapping = load_service_mappings(dir.path(), "repo1").unwrap();
        assert_eq!(mapping.mappings.len(), 1);
        assert_eq!(mapping.mappings[0].community_label, "community_a");
        assert_eq!(mapping.mappings[0].service_name, "Auth Service");
    }

    #[test]
    fn test_update_existing() {
        let dir = TempDir::new().unwrap();
        save_service_mapping(dir.path(), "repo1", "community_a", "Old Name").unwrap();
        save_service_mapping(dir.path(), "repo1", "community_a", "New Name").unwrap();

        let mapping = load_service_mappings(dir.path(), "repo1").unwrap();
        assert_eq!(mapping.mappings.len(), 1);
        assert_eq!(mapping.mappings[0].service_name, "New Name");
    }

    #[test]
    fn test_load_nonexistent() {
        let dir = TempDir::new().unwrap();
        let mapping = load_service_mappings(dir.path(), "repo1").unwrap();
        assert!(mapping.mappings.is_empty());
    }

    #[test]
    fn test_multiple_mappings() {
        let dir = TempDir::new().unwrap();
        save_service_mapping(dir.path(), "repo1", "comm_a", "Auth").unwrap();
        save_service_mapping(dir.path(), "repo1", "comm_b", "Data").unwrap();
        save_service_mapping(dir.path(), "repo1", "comm_c", "API").unwrap();

        let mapping = load_service_mappings(dir.path(), "repo1").unwrap();
        assert_eq!(mapping.mappings.len(), 3);
    }
}
