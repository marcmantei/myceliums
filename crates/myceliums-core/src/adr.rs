//! Architecture Decision Records (ADRs) — first-class graph entities.
//!
//! Store architectural decisions with context, rationale, and links to
//! code symbols they affect. Supports supersession chains.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Status of an ADR.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum AdrStatus {
    Proposed,
    Accepted,
    Deprecated,
    Superseded,
}

impl std::fmt::Display for AdrStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AdrStatus::Proposed => write!(f, "Proposed"),
            AdrStatus::Accepted => write!(f, "Accepted"),
            AdrStatus::Deprecated => write!(f, "Deprecated"),
            AdrStatus::Superseded => write!(f, "Superseded"),
        }
    }
}

/// An Architecture Decision Record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchDecisionRecord {
    pub id: String,
    pub title: String,
    pub status: AdrStatus,
    pub context: String,
    pub decision: String,
    pub consequences: String,
    pub linked_symbols: Vec<String>,
    pub superseded_by: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// Return the storage path for ADRs: `~/.myceliums/decisions/{repo_id}.json`
pub fn adr_path(data_dir: &Path, repo_id: &str) -> PathBuf {
    data_dir.join("decisions").join(format!("{}.json", repo_id))
}

/// Save or update an ADR (upserts by ID).
pub fn save_decision(
    data_dir: &Path,
    repo_id: &str,
    adr: &ArchDecisionRecord,
) -> anyhow::Result<()> {
    let mut decisions = load_decisions(data_dir, repo_id)?;

    // Upsert by ID
    if let Some(existing) = decisions.iter_mut().find(|d| d.id == adr.id) {
        *existing = adr.clone();
    } else {
        decisions.push(adr.clone());
    }

    let path = adr_path(data_dir, repo_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&decisions)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load all ADRs for a repository.
pub fn load_decisions(data_dir: &Path, repo_id: &str) -> anyhow::Result<Vec<ArchDecisionRecord>> {
    let path = adr_path(data_dir, repo_id);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let json = std::fs::read_to_string(&path)?;
    let decisions: Vec<ArchDecisionRecord> = serde_json::from_str(&json)?;
    Ok(decisions)
}

/// Link an ADR to a symbol by adding the symbol name to `linked_symbols`.
pub fn link_decision_to_symbol(
    data_dir: &Path,
    repo_id: &str,
    decision_id: &str,
    symbol_name: &str,
) -> anyhow::Result<()> {
    let mut decisions = load_decisions(data_dir, repo_id)?;
    let adr = decisions
        .iter_mut()
        .find(|d| d.id == decision_id)
        .ok_or_else(|| anyhow::anyhow!("ADR not found: {}", decision_id))?;

    if !adr.linked_symbols.contains(&symbol_name.to_string()) {
        adr.linked_symbols.push(symbol_name.to_string());
        adr.updated_at = chrono::Utc::now().to_rfc3339();
    }

    let path = adr_path(data_dir, repo_id);
    let json = serde_json::to_string_pretty(&decisions)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_adr(id: &str, title: &str, status: AdrStatus) -> ArchDecisionRecord {
        ArchDecisionRecord {
            id: id.to_string(),
            title: title.to_string(),
            status,
            context: "Some context".to_string(),
            decision: "We decided X".to_string(),
            consequences: "This means Y".to_string(),
            linked_symbols: Vec::new(),
            superseded_by: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_save_and_load() {
        let dir = TempDir::new().unwrap();
        let adr = make_adr("adr-001", "Use PostgreSQL", AdrStatus::Accepted);
        save_decision(dir.path(), "repo1", &adr).unwrap();

        let decisions = load_decisions(dir.path(), "repo1").unwrap();
        assert_eq!(decisions.len(), 1);
        assert_eq!(decisions[0].title, "Use PostgreSQL");
        assert_eq!(decisions[0].status, AdrStatus::Accepted);
    }

    #[test]
    fn test_link_symbol() {
        let dir = TempDir::new().unwrap();
        let adr = make_adr("adr-001", "Use PostgreSQL", AdrStatus::Accepted);
        save_decision(dir.path(), "repo1", &adr).unwrap();

        link_decision_to_symbol(dir.path(), "repo1", "adr-001", "db_connect").unwrap();

        let decisions = load_decisions(dir.path(), "repo1").unwrap();
        assert!(decisions[0]
            .linked_symbols
            .contains(&"db_connect".to_string()));
    }

    #[test]
    fn test_filter_by_status() {
        let dir = TempDir::new().unwrap();
        save_decision(
            dir.path(),
            "repo1",
            &make_adr("1", "ADR 1", AdrStatus::Proposed),
        )
        .unwrap();
        save_decision(
            dir.path(),
            "repo1",
            &make_adr("2", "ADR 2", AdrStatus::Accepted),
        )
        .unwrap();
        save_decision(
            dir.path(),
            "repo1",
            &make_adr("3", "ADR 3", AdrStatus::Deprecated),
        )
        .unwrap();

        let decisions = load_decisions(dir.path(), "repo1").unwrap();
        let accepted: Vec<_> = decisions
            .iter()
            .filter(|d| d.status == AdrStatus::Accepted)
            .collect();
        assert_eq!(accepted.len(), 1);
    }

    #[test]
    fn test_supersession() {
        let dir = TempDir::new().unwrap();
        let mut adr1 = make_adr("1", "Old approach", AdrStatus::Superseded);
        adr1.superseded_by = Some("2".to_string());
        save_decision(dir.path(), "repo1", &adr1).unwrap();

        let adr2 = make_adr("2", "New approach", AdrStatus::Accepted);
        save_decision(dir.path(), "repo1", &adr2).unwrap();

        let decisions = load_decisions(dir.path(), "repo1").unwrap();
        let old = decisions.iter().find(|d| d.id == "1").unwrap();
        assert_eq!(old.superseded_by, Some("2".to_string()));
    }

    #[test]
    fn test_empty_decisions() {
        let dir = TempDir::new().unwrap();
        let decisions = load_decisions(dir.path(), "repo1").unwrap();
        assert!(decisions.is_empty());
    }
}
