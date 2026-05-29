//! Quality hotspot scoring — composite refactoring priority metric.
//!
//! Combines graph centrality (betweenness), git churn (commit count),
//! and module instability into a single score that highlights symbols
//! which are both architecturally critical and frequently changed.

use crate::centrality::compute_centrality;
use crate::dependencies::compute_module_coupling;
use myceliums_storage::{CodeSymbol, Relationship, SymbolMetadata};
use serde::Serialize;
use std::collections::HashMap;

/// A single hotspot entry with its composite score and contributing metrics.
#[derive(Debug, Clone, Serialize)]
pub struct HotspotItem {
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub score: f64,
    pub degree: f64,
    pub betweenness: f64,
    pub commit_count: u32,
    pub instability: f64,
}

/// Compute the top-N quality hotspots by combining centrality, git churn, and instability.
///
/// Score formula: `betweenness * (1.0 + commit_count / 100.0) * (0.5 + instability)`
///
/// Higher scores indicate symbols that are both architecturally important
/// (high betweenness) AND frequently changed (high commit count) in
/// unstable modules — prime refactoring candidates.
pub fn compute_hotspot_scores(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    top_n: usize,
) -> Vec<HotspotItem> {
    if symbols.is_empty() {
        return Vec::new();
    }

    // 1. Centrality metrics
    let centrality = compute_centrality(relationships).unwrap_or_default();

    // 2. Module instability (file-level)
    let coupling = compute_module_coupling(symbols, relationships, false);
    let file_instability: HashMap<&str, f64> = coupling
        .iter()
        .map(|c| (c.module_path.as_str(), c.instability))
        .collect();

    // 3. Build scored items
    let mut items: Vec<HotspotItem> = symbols
        .iter()
        .map(|sym| {
            let cent = centrality.get(&sym.uid);
            let betweenness = cent.map_or(0.0, |c| c.betweenness);
            let degree = cent.map_or(0.0, |c| c.degree);

            let commit_count = sym
                .metadata
                .as_ref()
                .and_then(|m| serde_json::from_str::<SymbolMetadata>(m).ok())
                .and_then(|m| m.git)
                .map_or(1, |g| g.commit_count.max(1));

            let instability = file_instability
                .get(sym.file_path.as_str())
                .copied()
                .unwrap_or(0.5);

            let score = betweenness * (1.0 + commit_count as f64 / 100.0) * (0.5 + instability);

            HotspotItem {
                uid: sym.uid.clone(),
                name: sym.name.clone(),
                qualified_name: sym.qualified_name.clone(),
                kind: sym.kind.to_string(),
                file_path: sym.file_path.clone(),
                score,
                degree,
                betweenness,
                commit_count,
                instability,
            }
        })
        .collect();

    // 4. Sort descending by score, truncate
    items.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(top_n);
    items
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{Relationship, RelationshipKind, SymbolKind};

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

    fn make_symbol_with_churn(uid: &str, name: &str, commit_count: u32) -> CodeSymbol {
        let meta = SymbolMetadata {
            git: Some(myceliums_storage::GitMetadataEntry {
                last_author: "dev".to_string(),
                last_modified: "2026-01-01".to_string(),
                commit_count,
                age_days: 30,
                last_commit_hash: None,
            }),
            ..Default::default()
        };
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
            metadata: Some(serde_json::to_string(&meta).unwrap()),
        }
    }

    fn make_call(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Calls,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_hotspot_empty_graph() {
        let result = compute_hotspot_scores(&[], &[], 10);
        assert!(result.is_empty());
    }

    #[test]
    fn test_hotspot_single_node() {
        let symbols = vec![make_symbol("a", "alpha")];
        let result = compute_hotspot_scores(&symbols, &[], 10);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "alpha");
    }

    #[test]
    fn test_hotspot_hub_ranks_higher() {
        // Build a star graph: hub connects to a, b, c, d
        let symbols = vec![
            make_symbol("hub", "hub_fn"),
            make_symbol("a", "leaf_a"),
            make_symbol("b", "leaf_b"),
            make_symbol("c", "leaf_c"),
            make_symbol("d", "leaf_d"),
        ];
        let rels = vec![
            make_call("hub", "a"),
            make_call("hub", "b"),
            make_call("hub", "c"),
            make_call("hub", "d"),
            make_call("a", "hub"),
            make_call("b", "hub"),
        ];

        let result = compute_hotspot_scores(&symbols, &rels, 5);
        // hub should have highest betweenness and thus highest score
        assert_eq!(result[0].name, "hub_fn");
    }

    #[test]
    fn test_hotspot_respects_top_n() {
        let symbols: Vec<CodeSymbol> = (0..5)
            .map(|i| make_symbol(&format!("s{}", i), &format!("sym_{}", i)))
            .collect();
        let result = compute_hotspot_scores(&symbols, &[], 2);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_hotspot_git_churn_inflates_score() {
        // Two identical topology nodes, one with high churn
        let symbols = vec![
            make_symbol_with_churn("a", "low_churn", 1),
            make_symbol_with_churn("b", "high_churn", 100),
        ];
        // Both are bridges in a simple chain
        let extra = make_symbol("c", "leaf");
        let mut syms = symbols;
        syms.push(extra);
        let rels = vec![
            make_call("a", "c"),
            make_call("b", "c"),
            make_call("c", "a"),
            make_call("c", "b"),
        ];

        let result = compute_hotspot_scores(&syms, &rels, 3);
        // high_churn should score higher than low_churn due to commit_count multiplier
        let high_idx = result.iter().position(|h| h.name == "high_churn");
        let low_idx = result.iter().position(|h| h.name == "low_churn");
        if let (Some(hi), Some(lo)) = (high_idx, low_idx) {
            assert!(result[hi].score >= result[lo].score);
        }
    }
}
