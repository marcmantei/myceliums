//! Architecture drift detection — compare current graph against a saved snapshot.
//!
//! Computes a drift score (0–100, higher = less drift) and identifies
//! structural changes: added/removed symbols, community size changes.

use crate::snapshot::{build_snapshot, diff_snapshots, GraphSnapshot};
use myceliums_storage::{CodeSymbol, Community, Relationship};
use serde::Serialize;

/// Classification of how a community changed.
#[derive(Debug, Clone, Serialize)]
pub struct CommunityChange {
    pub label: String,
    /// One of: "added", "removed", "grown", "shrunk", "stable"
    pub change_type: String,
    pub old_count: Option<u32>,
    pub new_count: Option<u32>,
}

/// Drift detection report.
#[derive(Debug, Clone, Serialize)]
pub struct DriftReport {
    pub drift_score: f64,
    pub added_symbols: usize,
    pub removed_symbols: usize,
    pub added_edges: usize,
    pub removed_edges: usize,
    pub community_changes: Vec<CommunityChange>,
    pub summary: String,
}

/// Detect architectural drift between a baseline snapshot and the current graph.
///
/// Returns a `DriftReport` with a score from 0 (completely different) to 100
/// (identical) and details of what changed.
pub fn detect_drift(
    baseline: &GraphSnapshot,
    current_symbols: &[CodeSymbol],
    current_relationships: &[Relationship],
    current_communities: &[Community],
    baseline_communities: &[Community],
) -> DriftReport {
    let current = build_snapshot(&baseline.repo_id, current_symbols, current_relationships);
    let diff = diff_snapshots(baseline, &current);

    let total_entities = baseline.symbols.len().max(current.symbols.len())
        + baseline.edges.len().max(current.edges.len());

    let total_changes = diff.added_symbols.len()
        + diff.removed_symbols.len()
        + diff.added_edges.len()
        + diff.removed_edges.len();

    let drift_score = if total_entities == 0 {
        100.0
    } else {
        (100.0 - (total_changes as f64 / total_entities as f64 * 100.0)).max(0.0)
    };

    // Compare communities by label
    let mut community_changes = Vec::new();
    let baseline_by_label: std::collections::HashMap<&str, u32> = baseline_communities
        .iter()
        .map(|c| (c.label.as_str(), c.member_count))
        .collect();
    let current_by_label: std::collections::HashMap<&str, u32> = current_communities
        .iter()
        .map(|c| (c.label.as_str(), c.member_count))
        .collect();

    for (label, &old_count) in &baseline_by_label {
        match current_by_label.get(label) {
            None => community_changes.push(CommunityChange {
                label: label.to_string(),
                change_type: "removed".to_string(),
                old_count: Some(old_count),
                new_count: None,
            }),
            Some(&new_count) if new_count > old_count => {
                community_changes.push(CommunityChange {
                    label: label.to_string(),
                    change_type: "grown".to_string(),
                    old_count: Some(old_count),
                    new_count: Some(new_count),
                });
            }
            Some(&new_count) if new_count < old_count => {
                community_changes.push(CommunityChange {
                    label: label.to_string(),
                    change_type: "shrunk".to_string(),
                    old_count: Some(old_count),
                    new_count: Some(new_count),
                });
            }
            Some(_) => {
                community_changes.push(CommunityChange {
                    label: label.to_string(),
                    change_type: "stable".to_string(),
                    old_count: Some(old_count),
                    new_count: current_by_label.get(label).copied(),
                });
            }
        }
    }

    for (label, &new_count) in &current_by_label {
        if !baseline_by_label.contains_key(label) {
            community_changes.push(CommunityChange {
                label: label.to_string(),
                change_type: "added".to_string(),
                old_count: None,
                new_count: Some(new_count),
            });
        }
    }

    let summary = format!(
        "Drift score: {:.1}/100 — {} symbols added, {} removed, {} edges added, {} removed",
        drift_score,
        diff.added_symbols.len(),
        diff.removed_symbols.len(),
        diff.added_edges.len(),
        diff.removed_edges.len(),
    );

    DriftReport {
        drift_score,
        added_symbols: diff.added_symbols.len(),
        removed_symbols: diff.removed_symbols.len(),
        added_edges: diff.added_edges.len(),
        removed_edges: diff.removed_edges.len(),
        community_changes,
        summary,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{RelationshipKind, SymbolKind};

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_community(label: &str, count: u32) -> Community {
        Community {
            uid: label.to_string(),
            label: label.to_string(),
            repo_id: "test".to_string(),
            member_count: count,
            top_symbols: String::new(),
            summary: String::new(),
        }
    }

    #[test]
    fn test_drift_no_change() {
        let symbols = vec![make_symbol("a", "alpha")];
        let rels = vec![];
        let baseline = build_snapshot("test", &symbols, &rels);
        let communities = vec![make_community("mod1", 1)];

        let report = detect_drift(&baseline, &symbols, &rels, &communities, &communities);
        assert!((report.drift_score - 100.0).abs() < 0.01);
        assert_eq!(report.added_symbols, 0);
        assert_eq!(report.removed_symbols, 0);
    }

    #[test]
    fn test_drift_complete_change() {
        let old_symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        let baseline = build_snapshot("test", &old_symbols, &[]);

        let new_symbols = vec![make_symbol("x", "xray"), make_symbol("y", "yankee")];
        let report = detect_drift(&baseline, &new_symbols, &[], &[], &[]);
        assert!(report.drift_score < 50.0);
        assert_eq!(report.added_symbols, 2);
        assert_eq!(report.removed_symbols, 2);
    }

    #[test]
    fn test_drift_partial_change() {
        let symbols: Vec<CodeSymbol> = (0..10)
            .map(|i| make_symbol(&format!("s{}", i), &format!("sym{}", i)))
            .collect();
        let baseline = build_snapshot("test", &symbols, &[]);

        // Add 2 new symbols
        let mut new_symbols = symbols.clone();
        new_symbols.push(make_symbol("s10", "sym10"));
        new_symbols.push(make_symbol("s11", "sym11"));

        let report = detect_drift(&baseline, &new_symbols, &[], &[], &[]);
        assert!(report.drift_score > 50.0);
        assert_eq!(report.added_symbols, 2);
        assert_eq!(report.removed_symbols, 0);
    }

    #[test]
    fn test_drift_community_changes() {
        let symbols = vec![make_symbol("a", "alpha")];
        let baseline = build_snapshot("test", &symbols, &[]);

        let old_communities = vec![make_community("auth", 5), make_community("data", 3)];
        let new_communities = vec![
            make_community("auth", 8), // grown
            make_community("api", 4),  // added (data removed)
        ];

        let report = detect_drift(&baseline, &symbols, &[], &new_communities, &old_communities);
        assert!(report
            .community_changes
            .iter()
            .any(|c| c.change_type == "grown"));
        assert!(report
            .community_changes
            .iter()
            .any(|c| c.change_type == "removed"));
        assert!(report
            .community_changes
            .iter()
            .any(|c| c.change_type == "added"));
    }
}
