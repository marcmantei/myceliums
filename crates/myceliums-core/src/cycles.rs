use anyhow::{anyhow, Result};
use graphrs::algorithms::components::strongly_connected_components;
use graphrs::{Edge, Graph, GraphSpecs, Node};
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A single circular dependency cycle found via Tarjan's SCC algorithm.
#[derive(Debug, Clone)]
pub struct DependencyCycle {
    /// Symbol UIDs in this cycle
    pub member_uids: Vec<String>,
    /// Human-readable names
    pub member_names: Vec<String>,
    /// Number of symbols in the cycle
    pub size: usize,
    /// Files involved
    pub files: Vec<String>,
}

/// Detect circular dependencies using strongly connected components.
///
/// Builds a directed graph from CALLS and/or IMPORTS edges, then runs
/// Tarjan's SCC algorithm. Any SCC with more than one node is a cycle.
pub fn detect_cycles(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    include_calls: bool,
    include_imports: bool,
    min_cycle_size: usize,
) -> Result<Vec<DependencyCycle>> {
    let edges_with_kind: Vec<&Relationship> = relationships
        .iter()
        .filter(|r| {
            (include_calls && r.kind == RelationshipKind::Calls)
                || (include_imports && r.kind == RelationshipKind::Imports)
        })
        .filter(|r| r.source_uid != r.target_uid)
        .collect();

    if edges_with_kind.is_empty() {
        return Ok(vec![]);
    }

    let mut uids: HashSet<String> = HashSet::new();
    for rel in &edges_with_kind {
        uids.insert(rel.source_uid.clone());
        uids.insert(rel.target_uid.clone());
    }

    let nodes: Vec<Arc<Node<String, ()>>> = uids
        .iter()
        .map(|uid| Node::from_name(uid.clone()))
        .collect();

    let edges: Vec<Arc<Edge<String, ()>>> = edges_with_kind
        .iter()
        .map(|rel| Edge::with_weight(rel.source_uid.clone(), rel.target_uid.clone(), 1.0))
        .collect();

    let graph = Graph::<String, ()>::new_from_nodes_and_edges(nodes, edges, GraphSpecs::directed())
        .map_err(|e| anyhow!("Failed to create graph: {:?}", e))?;

    let sccs = strongly_connected_components(&graph)
        .map_err(|e| anyhow!("SCC computation failed: {:?}", e))?;

    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut cycles: Vec<DependencyCycle> = sccs
        .into_iter()
        .filter(|scc: &HashSet<String>| scc.len() >= min_cycle_size.max(2))
        .map(|scc: HashSet<String>| {
            let member_uids: Vec<String> = scc.into_iter().collect();
            let member_names: Vec<String> = member_uids
                .iter()
                .filter_map(|uid: &String| uid_to_symbol.get(uid.as_str()).map(|s| s.name.clone()))
                .collect();
            let files: Vec<String> = member_uids
                .iter()
                .filter_map(|uid: &String| {
                    uid_to_symbol.get(uid.as_str()).map(|s| s.file_path.clone())
                })
                .collect::<HashSet<_>>()
                .into_iter()
                .collect();
            let size = member_uids.len();
            DependencyCycle {
                member_uids,
                member_names,
                size,
                files,
            }
        })
        .collect();

    // Sort by size descending — largest cycles first
    cycles.sort_by_key(|c| std::cmp::Reverse(c.size));

    Ok(cycles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

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
    fn test_simple_cycle() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        // a -> b -> c -> a (cycle of 3)
        let rels = vec![
            make_call("a", "b"),
            make_call("b", "c"),
            make_call("c", "a"),
        ];

        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].size, 3);
    }

    #[test]
    fn test_no_cycle() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        // a -> b -> c (no cycle)
        let rels = vec![make_call("a", "b"), make_call("b", "c")];

        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_multiple_cycles() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
            make_symbol("d", "delta"),
        ];
        // Cycle 1: a <-> b
        // Cycle 2: c <-> d
        let rels = vec![
            make_call("a", "b"),
            make_call("b", "a"),
            make_call("c", "d"),
            make_call("d", "c"),
        ];

        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert_eq!(cycles.len(), 2);
    }

    #[test]
    fn test_min_cycle_size_filter() {
        let symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        // a <-> b (cycle of 2)
        let rels = vec![make_call("a", "b"), make_call("b", "a")];

        // min_cycle_size = 3 should filter this out
        let cycles = detect_cycles(&symbols, &rels, true, false, 3).unwrap();
        assert!(cycles.is_empty());

        // min_cycle_size = 2 should include it
        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert_eq!(cycles.len(), 1);
    }

    #[test]
    fn test_empty_graph() {
        let cycles = detect_cycles(&[], &[], true, true, 2).unwrap();
        assert!(cycles.is_empty());
    }

    fn make_import(source: &str, target: &str) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind: RelationshipKind::Imports,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_cycle_with_imports_only() {
        let symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        let rels = vec![make_import("a", "b"), make_import("b", "a")];

        // include_calls=false, include_imports=true
        let cycles = detect_cycles(&symbols, &rels, false, true, 2).unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].size, 2);
    }

    #[test]
    fn test_cycle_mixed_calls_and_imports() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        // a calls b, b imports c, c calls a → cycle
        let rels = vec![
            make_call("a", "b"),
            make_import("b", "c"),
            make_call("c", "a"),
        ];

        let cycles = detect_cycles(&symbols, &rels, true, true, 2).unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].size, 3);
    }

    #[test]
    fn test_calls_only_flag_ignores_imports() {
        let symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        // Import cycle only
        let rels = vec![make_import("a", "b"), make_import("b", "a")];

        // include_calls=true, include_imports=false → should find no cycles
        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert!(cycles.is_empty());
    }

    #[test]
    fn test_large_cycle() {
        let n = 10;
        let symbols: Vec<CodeSymbol> = (0..n)
            .map(|i| make_symbol(&format!("s{}", i), &format!("sym_{}", i)))
            .collect();
        // Chain: s0->s1->s2->...->s9->s0
        let mut rels: Vec<Relationship> = (0..n - 1)
            .map(|i| make_call(&format!("s{}", i), &format!("s{}", i + 1)))
            .collect();
        rels.push(make_call(&format!("s{}", n - 1), "s0"));

        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert_eq!(cycles.len(), 1);
        assert_eq!(cycles[0].size, n);
    }

    #[test]
    fn test_cycle_files_deduplication() {
        // Two symbols in same file form a cycle
        let symbols = vec![
            CodeSymbol {
                uid: "a".to_string(),
                name: "alpha".to_string(),
                qualified_name: "alpha".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/shared.rs".to_string(),
                start_line: 1,
                end_line: 10,
                signature: String::new(),
                content: String::new(),
                repo_id: "test".to_string(),
                metadata: None,
            },
            CodeSymbol {
                uid: "b".to_string(),
                name: "beta".to_string(),
                qualified_name: "beta".to_string(),
                kind: SymbolKind::Function,
                file_path: "src/shared.rs".to_string(),
                start_line: 11,
                end_line: 20,
                signature: String::new(),
                content: String::new(),
                repo_id: "test".to_string(),
                metadata: None,
            },
        ];
        let rels = vec![make_call("a", "b"), make_call("b", "a")];

        let cycles = detect_cycles(&symbols, &rels, true, false, 2).unwrap();
        assert_eq!(cycles.len(), 1);
        // Files list should be deduplicated — only one "src/shared.rs"
        assert_eq!(
            cycles[0].files.len(),
            1,
            "Files should be deduplicated: {:?}",
            cycles[0].files
        );
    }
}
