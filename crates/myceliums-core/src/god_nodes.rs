use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind};
use std::cmp::Reverse;
use std::collections::HashMap;

pub struct GodNodeItem {
    pub uid: String,
    pub name: String,
    pub qualified_name: String,
    pub kind: String,
    pub file_path: String,
    pub degree: u32,
    pub in_degree: u32,
    pub out_degree: u32,
    pub is_high_coupling: bool,
}

/// Compute the top-N highest-degree symbols by counting CALLS edges.
///
/// `coupling_threshold` marks symbols with more connections than this value
/// as high-coupling candidates for refactoring (default 20, checked as > threshold).
pub fn compute_god_nodes(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    top_n: usize,
    coupling_threshold: u32,
) -> Vec<GodNodeItem> {
    let mut in_degree: HashMap<&str, u32> = HashMap::new();
    let mut out_degree: HashMap<&str, u32> = HashMap::new();

    for rel in relationships
        .iter()
        .filter(|r| r.kind == RelationshipKind::Calls)
    {
        *out_degree.entry(rel.source_uid.as_str()).or_insert(0) += 1;
        *in_degree.entry(rel.target_uid.as_str()).or_insert(0) += 1;
    }

    let mut items: Vec<GodNodeItem> = symbols
        .iter()
        .map(|sym| {
            let in_d = in_degree.get(sym.uid.as_str()).copied().unwrap_or(0);
            let out_d = out_degree.get(sym.uid.as_str()).copied().unwrap_or(0);
            let degree = in_d + out_d;
            GodNodeItem {
                uid: sym.uid.clone(),
                name: sym.name.clone(),
                qualified_name: sym.qualified_name.clone(),
                kind: sym.kind.to_string(),
                file_path: sym.file_path.clone(),
                degree,
                in_degree: in_d,
                out_degree: out_d,
                is_high_coupling: degree > coupling_threshold,
            }
        })
        .collect();

    items.sort_by_key(|b| Reverse(b.degree));
    items.truncate(top_n);
    items
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
        use myceliums_storage::Relationship;
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
    fn test_degree_centrality_computation() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
        ];
        // a -> b, a -> c, b -> c  => a out=2, b out=1 in=1, c in=2
        let rels = vec![
            make_call("a", "b"),
            make_call("a", "c"),
            make_call("b", "c"),
        ];

        let nodes = compute_god_nodes(&symbols, &rels, 3, 20);
        assert_eq!(nodes.len(), 3);

        // gamma (c) has degree 2 (in=2, out=0), alpha (a) has degree 2 (in=0, out=2)
        // beta (b) has degree 2 (in=1, out=1)
        // all degree 2, order may vary but all should be present
        let total_degrees: u32 = nodes.iter().map(|n| n.degree).sum();
        assert_eq!(total_degrees, 6);
    }

    #[test]
    fn test_top_n_selection() {
        let symbols = vec![
            make_symbol("a", "alpha"),
            make_symbol("b", "beta"),
            make_symbol("c", "gamma"),
            make_symbol("d", "delta"),
        ];
        // Make 'a' the god node with the most connections
        let rels = vec![
            make_call("a", "b"),
            make_call("a", "c"),
            make_call("a", "d"),
            make_call("b", "c"),
        ];
        let nodes = compute_god_nodes(&symbols, &rels, 2, 20);
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].name, "alpha"); // 'a' has out=3 (degree 3)
    }

    #[test]
    fn test_coupling_threshold_flag() {
        let symbols = vec![make_symbol("hub", "HubNode")];
        // Build 25 edges into 'hub' to exceed default threshold of 20
        let targets: Vec<CodeSymbol> = (0..25)
            .map(|i| make_symbol(&format!("t{}", i), &format!("target{}", i)))
            .collect();
        let mut all_syms = symbols;
        all_syms.extend(targets);

        let rels: Vec<Relationship> = (0..25)
            .map(|i| make_call(&format!("t{}", i), "hub"))
            .collect();

        let nodes = compute_god_nodes(&all_syms, &rels, 1, 20);
        assert_eq!(nodes.len(), 1);
        assert!(nodes[0].is_high_coupling);
        assert_eq!(nodes[0].in_degree, 25);
    }

    #[test]
    fn test_empty_graph_returns_all_zeros() {
        let symbols = vec![make_symbol("a", "alpha"), make_symbol("b", "beta")];
        let nodes = compute_god_nodes(&symbols, &[], 10, 20);
        assert_eq!(nodes.len(), 2);
        for n in &nodes {
            assert_eq!(n.degree, 0);
            assert!(!n.is_high_coupling);
        }
    }
}
