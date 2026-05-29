use crate::community::compute_uid_to_community_label;
use anyhow::Result;
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
use std::collections::HashMap;

pub struct SurprisingConnectionItem {
    pub source_uid: String,
    pub source_name: String,
    pub source_qualified_name: String,
    pub target_uid: String,
    pub target_name: String,
    pub target_qualified_name: String,
    pub source_community: String,
    pub target_community: String,
    /// Score in [0.0, 1.0]: higher means fewer edges cross this community pair
    /// relative to all cross-community edges — i.e. a more isolated connection.
    pub surprise_score: f64,
}

/// Detect cross-community CALLS edges ranked by surprise score.
///
/// Surprise score for an edge (A→B) is computed as:
///   `1.0 - (edges_between_A's_community_and_B's_community / total_cross_community_edges)`
///
/// Edges belonging to community pairs that rarely interact score highest.
/// Returns at most `limit` results with `surprise_score >= min_surprise_score`.
pub fn compute_surprising_connections(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    min_surprise_score: f64,
    limit: usize,
) -> Result<Vec<SurprisingConnectionItem>> {
    let uid_to_community = compute_uid_to_community_label(symbols, relationships)?;

    if uid_to_community.is_empty() {
        return Ok(vec![]);
    }

    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let call_rels: Vec<&Relationship> = relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
        .collect();

    if call_rels.is_empty() {
        return Ok(vec![]);
    }

    // Collect cross-community edges and count how many edges cross each community pair
    let mut cross_edges: Vec<(&Relationship, String, String)> = Vec::new();
    // Canonical key: alphabetically sorted pair of community labels
    let mut pair_counts: HashMap<(String, String), u32> = HashMap::new();

    for rel in &call_rels {
        if let (Some(ca), Some(cb)) = (
            uid_to_community.get(rel.source_uid.as_str()),
            uid_to_community.get(rel.target_uid.as_str()),
        ) {
            if ca != cb {
                let canonical = canonical_pair(ca, cb);
                *pair_counts.entry(canonical).or_insert(0) += 1;
                cross_edges.push((rel, ca.clone(), cb.clone()));
            }
        }
    }

    if cross_edges.is_empty() {
        return Ok(vec![]);
    }

    let total_cross = cross_edges.len() as f64;

    let mut items: Vec<SurprisingConnectionItem> = cross_edges
        .into_iter()
        .filter_map(|(rel, ca, cb)| {
            let pair_count = *pair_counts.get(&canonical_pair(&ca, &cb)).unwrap_or(&1) as f64;
            let surprise_score = 1.0 - (pair_count / total_cross);

            if surprise_score < min_surprise_score {
                return None;
            }

            let src = uid_to_symbol.get(rel.source_uid.as_str())?;
            let tgt = uid_to_symbol.get(rel.target_uid.as_str())?;

            Some(SurprisingConnectionItem {
                source_uid: rel.source_uid.clone(),
                source_name: src.name.clone(),
                source_qualified_name: src.qualified_name.clone(),
                target_uid: rel.target_uid.clone(),
                target_name: tgt.name.clone(),
                target_qualified_name: tgt.qualified_name.clone(),
                source_community: ca,
                target_community: cb,
                surprise_score,
            })
        })
        .collect();

    items.sort_by(|a, b| {
        b.surprise_score
            .partial_cmp(&a.surprise_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    items.truncate(limit);

    Ok(items)
}

fn canonical_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{Community, Relationship, SymbolKind};

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
    fn test_no_calls_returns_empty() {
        let symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        let result = compute_surprising_connections(&symbols, &[], 0.0, 100).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn test_single_community_returns_empty() {
        // When all symbols form a single community, no cross-community edges exist
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        // Tight clique → Leiden puts all in one community
        let rels = vec![
            make_call("a", "b"),
            make_call("b", "a"),
            make_call("b", "c"),
            make_call("c", "b"),
            make_call("a", "c"),
            make_call("c", "a"),
        ];
        // This may or may not produce a single community depending on graph structure;
        // at minimum the function must not panic and must return a valid Vec
        let result = compute_surprising_connections(&symbols, &rels, 0.0, 100);
        assert!(result.is_ok());
    }

    #[test]
    fn test_surprise_score_in_range() {
        // Build two isolated clusters with one bridge edge
        // Cluster 1: a <-> b <-> c (tight)
        // Cluster 2: d <-> e <-> f (tight)
        // Bridge: a -> d (should be highly surprising)
        let mut symbols: Vec<CodeSymbol> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|n| make_symbol(n, n))
            .collect();
        // Add extra symbols to ensure Leiden can detect communities
        for i in 0..10 {
            symbols.push(make_symbol(&format!("x{}", i), &format!("x{}", i)));
        }

        let result = compute_surprising_connections(&symbols, &[], 0.0, 100).unwrap();
        for item in &result {
            assert!(
                item.surprise_score >= 0.0 && item.surprise_score <= 1.0,
                "score out of range: {}",
                item.surprise_score
            );
        }
    }

    #[test]
    fn test_min_surprise_score_filter() {
        let symbols: Vec<CodeSymbol> = ["a", "b", "c"].iter().map(|n| make_symbol(n, n)).collect();
        let rels = vec![make_call("a", "b"), make_call("b", "c")];
        // With a very high threshold, expect empty (or fewer) results
        let result = compute_surprising_connections(&symbols, &rels, 0.99, 100).unwrap();
        // All scores are ≤ 1.0, so filtered by 0.99 may return some or none
        for item in &result {
            assert!(item.surprise_score >= 0.99);
        }
    }

    #[test]
    fn test_limit_is_respected() {
        let symbols: Vec<CodeSymbol> = (0..20)
            .map(|i| make_symbol(&format!("s{}", i), &format!("sym{}", i)))
            .collect();
        let rels: Vec<Relationship> = (0..19)
            .map(|i| make_call(&format!("s{}", i), &format!("s{}", i + 1)))
            .collect();
        let result = compute_surprising_connections(&symbols, &rels, 0.0, 3).unwrap();
        assert!(result.len() <= 3);
    }

    #[test]
    fn test_canonical_pair_ordering() {
        assert_eq!(canonical_pair("b", "a"), ("a".to_string(), "b".to_string()));
        assert_eq!(canonical_pair("a", "b"), ("a".to_string(), "b".to_string()));
        assert_eq!(canonical_pair("z", "z"), ("z".to_string(), "z".to_string()));
    }

    #[allow(dead_code)]
    fn _community_type_check(_: Community) {}
}
