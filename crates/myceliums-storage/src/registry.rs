use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::models::RepoInfo;

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct RepoRegistry {
    pub repos: HashMap<String, RepoInfo>,
    #[serde(skip)]
    path: PathBuf,
}

impl RepoRegistry {
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

    pub fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(&self)?;
        std::fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn register(&mut self, info: RepoInfo) {
        self.repos.insert(info.id.clone(), info);
    }

    pub fn remove(&mut self, repo_id: &str) -> Option<RepoInfo> {
        self.repos.remove(repo_id)
    }

    pub fn get(&self, repo_id: &str) -> Option<&RepoInfo> {
        self.repos.get(repo_id)
    }

    pub fn find_by_path(&self, path: &str) -> Option<&RepoInfo> {
        self.repos.values().find(|r| r.path == path)
    }

    pub fn list(&self) -> Vec<&RepoInfo> {
        let mut repos: Vec<&RepoInfo> = self.repos.values().collect();
        repos.sort_by(|a, b| a.name.cmp(&b.name));
        repos
    }

    pub fn repo_db_path(data_dir: &Path, repo_id: &str) -> PathBuf {
        data_dir.join("data").join(repo_id)
    }
}
