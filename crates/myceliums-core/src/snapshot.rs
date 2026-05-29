//! Graph snapshot storage and diff comparison.
//!
//! After each analysis, a lightweight snapshot (symbol UIDs + edge UIDs) is
//! saved to `~/.myceliums/snapshots/<repo_id>.json`. The [`diff_snapshots`]
//! function compares two snapshots and returns the sets of added/removed
//! symbols and relationships.

use myceliums_storage::{CodeSymbol, Relationship};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// A lightweight graph snapshot — just UIDs and display-friendly names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphSnapshot {
    pub repo_id: String,
    pub created_at: String,
    /// Map of symbol UID -> human label  (e.g. "Function  MyClass.foo")
    pub symbols: BTreeMap<String, String>,
    /// Map of edge UID -> human label    (e.g. "CALLS  foo -> bar")
    pub edges: BTreeMap<String, String>,
}

/// Result of comparing two snapshots.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphDiff {
    pub repo_id: String,
    pub previous_snapshot_at: String,
    pub current_snapshot_at: String,
    pub added_symbols: Vec<DiffEntry>,
    pub removed_symbols: Vec<DiffEntry>,
    pub added_edges: Vec<DiffEntry>,
    pub removed_edges: Vec<DiffEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffEntry {
    pub uid: String,
    pub label: String,
}

/// Build a snapshot from the current store contents.
pub fn build_snapshot(
    repo_id: &str,
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
) -> GraphSnapshot {
    let mut sym_map = BTreeMap::new();
    for s in symbols {
        let label = format!("{}  {}", s.kind, s.qualified_name);
        sym_map.insert(s.uid.clone(), label);
    }

    let mut edge_map = BTreeMap::new();
    // Build a quick uid->name lookup for edge labels
    let uid_to_name: BTreeMap<&str, &str> = symbols
        .iter()
        .map(|s| (s.uid.as_str(), s.name.as_str()))
        .collect();

    for r in relationships {
        let src = uid_to_name.get(r.source_uid.as_str()).unwrap_or(&"?");
        let tgt = uid_to_name.get(r.target_uid.as_str()).unwrap_or(&"?");
        let label = format!("{}  {} -> {}", r.kind, src, tgt);
        edge_map.insert(r.uid.clone(), label);
    }

    GraphSnapshot {
        repo_id: repo_id.to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        symbols: sym_map,
        edges: edge_map,
    }
}

/// Return the path where snapshots are stored for a repo.
pub fn snapshot_path(data_dir: &Path, repo_id: &str) -> PathBuf {
    data_dir.join("snapshots").join(format!("{}.json", repo_id))
}

/// Save a snapshot to disk, overwriting any previous one.
pub fn save_snapshot(data_dir: &Path, snapshot: &GraphSnapshot) -> anyhow::Result<()> {
    let path = snapshot_path(data_dir, &snapshot.repo_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(snapshot)?;
    std::fs::write(&path, json)?;
    Ok(())
}

/// Load a previously saved snapshot (returns None if no snapshot exists).
pub fn load_snapshot(data_dir: &Path, repo_id: &str) -> anyhow::Result<Option<GraphSnapshot>> {
    let path = snapshot_path(data_dir, repo_id);
    if !path.exists() {
        return Ok(None);
    }
    let json = std::fs::read_to_string(&path)?;
    let snap: GraphSnapshot = serde_json::from_str(&json)?;
    Ok(Some(snap))
}

/// Compare the previous snapshot against the current one.
pub fn diff_snapshots(previous: &GraphSnapshot, current: &GraphSnapshot) -> GraphDiff {
    let prev_sym_uids: BTreeSet<&str> = previous.symbols.keys().map(|s| s.as_str()).collect();
    let curr_sym_uids: BTreeSet<&str> = current.symbols.keys().map(|s| s.as_str()).collect();

    let added_symbols: Vec<DiffEntry> = curr_sym_uids
        .difference(&prev_sym_uids)
        .map(|uid| DiffEntry {
            uid: uid.to_string(),
            label: current.symbols.get(*uid).cloned().unwrap_or_default(),
        })
        .collect();

    let removed_symbols: Vec<DiffEntry> = prev_sym_uids
        .difference(&curr_sym_uids)
        .map(|uid| DiffEntry {
            uid: uid.to_string(),
            label: previous.symbols.get(*uid).cloned().unwrap_or_default(),
        })
        .collect();

    let prev_edge_uids: BTreeSet<&str> = previous.edges.keys().map(|s| s.as_str()).collect();
    let curr_edge_uids: BTreeSet<&str> = current.edges.keys().map(|s| s.as_str()).collect();

    let added_edges: Vec<DiffEntry> = curr_edge_uids
        .difference(&prev_edge_uids)
        .map(|uid| DiffEntry {
            uid: uid.to_string(),
            label: current.edges.get(*uid).cloned().unwrap_or_default(),
        })
        .collect();

    let removed_edges: Vec<DiffEntry> = prev_edge_uids
        .difference(&curr_edge_uids)
        .map(|uid| DiffEntry {
            uid: uid.to_string(),
            label: previous.edges.get(*uid).cloned().unwrap_or_default(),
        })
        .collect();

    GraphDiff {
        repo_id: current.repo_id.clone(),
        previous_snapshot_at: previous.created_at.clone(),
        current_snapshot_at: current.created_at.clone(),
        added_symbols,
        removed_symbols,
        added_edges,
        removed_edges,
    }
}

// ── Versioned snapshot support ────────────────────────────────────────

/// Summary of a stored snapshot (lightweight, without full data).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotSummary {
    pub snapshot_id: String,
    pub created_at: String,
    pub commit_sha: Option<String>,
    pub symbol_count: usize,
    pub edge_count: usize,
}

/// Return the directory for versioned snapshots.
pub fn snapshot_dir(data_dir: &Path, repo_id: &str) -> PathBuf {
    data_dir.join("snapshots").join(repo_id)
}

/// Save a versioned snapshot with a timestamped filename. Returns the snapshot ID.
pub fn save_versioned_snapshot(
    data_dir: &Path,
    snapshot: &GraphSnapshot,
    commit_sha: Option<&str>,
) -> anyhow::Result<String> {
    let dir = snapshot_dir(data_dir, &snapshot.repo_id);
    std::fs::create_dir_all(&dir)?;

    // Use a filesystem-safe timestamp as the ID
    let snapshot_id = snapshot.created_at.replace(':', "-").replace('+', "p");
    let filename = format!("{}.json", snapshot_id);

    // Wrap with optional commit_sha in a container
    let wrapper = VersionedSnapshotWrapper {
        commit_sha: commit_sha.map(|s| s.to_string()),
        snapshot: snapshot.clone(),
    };

    let json = serde_json::to_string_pretty(&wrapper)?;
    std::fs::write(dir.join(filename), json)?;

    // Also save as the "latest" single-file snapshot for backward compat
    save_snapshot(data_dir, snapshot)?;

    Ok(snapshot_id)
}

/// List all snapshots for a repo, sorted newest first.
pub fn list_snapshots(data_dir: &Path, repo_id: &str) -> anyhow::Result<Vec<SnapshotSummary>> {
    let dir = snapshot_dir(data_dir, repo_id);
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        let wrapper: VersionedSnapshotWrapper = serde_json::from_str(&content)?;
        let snapshot_id = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        summaries.push(SnapshotSummary {
            snapshot_id,
            created_at: wrapper.snapshot.created_at,
            commit_sha: wrapper.commit_sha,
            symbol_count: wrapper.snapshot.symbols.len(),
            edge_count: wrapper.snapshot.edges.len(),
        });
    }

    // Sort newest first
    summaries.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    Ok(summaries)
}

/// Load a specific snapshot by its ID.
pub fn load_snapshot_by_id(
    data_dir: &Path,
    repo_id: &str,
    snapshot_id: &str,
) -> anyhow::Result<Option<GraphSnapshot>> {
    let path = snapshot_dir(data_dir, repo_id).join(format!("{}.json", snapshot_id));
    if !path.exists() {
        return Ok(None);
    }
    let content = std::fs::read_to_string(&path)?;
    let wrapper: VersionedSnapshotWrapper = serde_json::from_str(&content)?;
    Ok(Some(wrapper.snapshot))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VersionedSnapshotWrapper {
    commit_sha: Option<String>,
    snapshot: GraphSnapshot,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_diff_detects_additions_and_removals() {
        let prev = GraphSnapshot {
            repo_id: "test".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            symbols: [("s1".into(), "Function  foo".into())]
                .into_iter()
                .collect(),
            edges: [("e1".into(), "CALLS  foo -> bar".into())]
                .into_iter()
                .collect(),
        };
        let curr = GraphSnapshot {
            repo_id: "test".into(),
            created_at: "2024-01-02T00:00:00Z".into(),
            symbols: [("s2".into(), "Function  baz".into())]
                .into_iter()
                .collect(),
            edges: [("e2".into(), "CALLS  baz -> qux".into())]
                .into_iter()
                .collect(),
        };

        let diff = diff_snapshots(&prev, &curr);

        assert_eq!(diff.added_symbols.len(), 1);
        assert_eq!(diff.added_symbols[0].uid, "s2");
        assert_eq!(diff.removed_symbols.len(), 1);
        assert_eq!(diff.removed_symbols[0].uid, "s1");
        assert_eq!(diff.added_edges.len(), 1);
        assert_eq!(diff.removed_edges.len(), 1);
    }

    #[test]
    fn test_diff_no_changes() {
        let snap = GraphSnapshot {
            repo_id: "test".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            symbols: [("s1".into(), "Function  foo".into())]
                .into_iter()
                .collect(),
            edges: BTreeMap::new(),
        };

        let diff = diff_snapshots(&snap, &snap);
        assert!(diff.added_symbols.is_empty());
        assert!(diff.removed_symbols.is_empty());
        assert!(diff.added_edges.is_empty());
        assert!(diff.removed_edges.is_empty());
    }

    #[test]
    fn test_save_and_list_versioned() {
        let dir = tempfile::TempDir::new().unwrap();

        for i in 0..3 {
            let snap = GraphSnapshot {
                repo_id: "test".into(),
                created_at: format!("2024-01-0{}T00:00:00Z", i + 1),
                symbols: [(format!("s{}", i), format!("Function  fn{}", i))]
                    .into_iter()
                    .collect(),
                edges: BTreeMap::new(),
            };
            save_versioned_snapshot(dir.path(), &snap, Some(&format!("abc{}", i))).unwrap();
        }

        let summaries = list_snapshots(dir.path(), "test").unwrap();
        assert_eq!(summaries.len(), 3);
        // Newest first
        assert!(summaries[0].created_at > summaries[1].created_at);
    }

    #[test]
    fn test_load_by_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let snap = GraphSnapshot {
            repo_id: "test".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            symbols: [("s1".into(), "Function  foo".into())]
                .into_iter()
                .collect(),
            edges: BTreeMap::new(),
        };

        let id = save_versioned_snapshot(dir.path(), &snap, None).unwrap();
        let loaded = load_snapshot_by_id(dir.path(), "test", &id)
            .unwrap()
            .expect("should exist");
        assert_eq!(loaded.repo_id, "test");
        assert_eq!(loaded.symbols.len(), 1);
    }

    #[test]
    fn test_backward_compat() {
        let dir = tempfile::TempDir::new().unwrap();
        let snap = GraphSnapshot {
            repo_id: "test".into(),
            created_at: "2024-01-01T00:00:00Z".into(),
            symbols: [("s1".into(), "Function  foo".into())]
                .into_iter()
                .collect(),
            edges: BTreeMap::new(),
        };

        // Save using old single-file method
        save_snapshot(dir.path(), &snap).unwrap();
        // Load using old method should still work
        let loaded = load_snapshot(dir.path(), "test")
            .unwrap()
            .expect("should exist");
        assert_eq!(loaded.repo_id, "test");
    }
}
