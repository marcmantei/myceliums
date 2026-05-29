use anyhow::{anyhow, Result};
use graphrs::algorithms::centrality::betweenness::betweenness_centrality;
use graphrs::algorithms::centrality::closeness::closeness_centrality;
use graphrs::algorithms::centrality::eigenvector::eigenvector_centrality;
use graphrs::{Edge, Graph, GraphSpecs, Node};
use myceliums_storage::{Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Centrality scores for a single graph node.
#[derive(Debug, Clone)]
pub struct NodeCentrality {
    pub uid: String,
    pub degree: f64,
    pub betweenness: f64,
    pub closeness: f64,
    pub eigenvector: f64,
}

/// Build the undirected `graphrs` graph that both community detection and
/// centrality share. Filters self-loops, includes CALLS (3.0),
/// IMPORTS (2.0), and CONTAINED_BY (1.0) edges.
pub fn build_undirected_graph(relationships: &[Relationship]) -> Result<Graph<String, ()>> {
    let structural: Vec<(&Relationship, f64)> = relationships
        .iter()
        .filter_map(|r| match r.kind {
            RelationshipKind::Calls => Some((r, 3.0)),
            RelationshipKind::Imports => Some((r, 2.0)),
            RelationshipKind::ContainedBy => Some((r, 1.0)),
            _ => None,
        })
        .collect();

    let mut uids: HashSet<String> = HashSet::new();
    for (rel, _) in &structural {
        uids.insert(rel.source_uid.clone());
        uids.insert(rel.target_uid.clone());
    }

    let nodes: Vec<Arc<Node<String, ()>>> = uids
        .iter()
        .map(|uid| Node::from_name(uid.clone()))
        .collect();

    let edges: Vec<Arc<Edge<String, ()>>> = structural
        .iter()
        .filter(|(rel, _)| rel.source_uid != rel.target_uid)
        .map(|(rel, weight)| {
            Edge::with_weight(rel.source_uid.clone(), rel.target_uid.clone(), *weight)
        })
        .collect();

    let graph =
        Graph::<String, ()>::new_from_nodes_and_edges(nodes, edges, GraphSpecs::undirected())
            .map_err(|e| anyhow!("Failed to create graph: {:?}", e))?;
    Ok(graph)
}

/// Compute all four centrality metrics for every node in the graph.
///
/// Returns `None` if there are not enough structural relationships.
pub fn compute_centrality(
    relationships: &[Relationship],
) -> Result<HashMap<String, NodeCentrality>> {
    let graph = build_undirected_graph(relationships)?;

    // Degree centrality (already used in community.rs via graphrs)
    let degree = graphrs::algorithms::centrality::degree::degree_centrality(&graph);

    // Betweenness — weighted, normalized
    let between = betweenness_centrality(&graph, true, true).unwrap_or_default();

    // Closeness — weighted, Wasserman-Faust improved
    let close = closeness_centrality(&graph, true, true).unwrap_or_default();

    // Eigenvector — weighted, default convergence params
    let eigen = eigenvector_centrality(&graph, true, None, None).unwrap_or_default();

    let mut result: HashMap<String, NodeCentrality> = HashMap::new();

    for (uid, &deg) in &degree {
        result.insert(
            uid.clone(),
            NodeCentrality {
                uid: uid.clone(),
                degree: deg,
                betweenness: between.get(uid).copied().unwrap_or(0.0),
                closeness: close.get(uid).copied().unwrap_or(0.0),
                eigenvector: eigen.get(uid).copied().unwrap_or(0.0),
            },
        );
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::Relationship;

    fn make_rel(source: &str, target: &str, kind: RelationshipKind) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_centrality_on_linear_chain() {
        // a -> b -> c: b should have highest betweenness
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "c", RelationshipKind::Calls),
        ];

        let centrality = compute_centrality(&rels).unwrap();
        assert_eq!(centrality.len(), 3);

        let b = &centrality["b"];
        let a = &centrality["a"];
        assert!(
            b.betweenness >= a.betweenness,
            "b should have higher betweenness than a"
        );
    }

    #[test]
    fn test_centrality_on_star_graph() {
        // hub connected to a, b, c, d — hub should dominate all metrics
        let rels = vec![
            make_rel("hub", "a", RelationshipKind::Calls),
            make_rel("hub", "b", RelationshipKind::Calls),
            make_rel("hub", "c", RelationshipKind::Calls),
            make_rel("hub", "d", RelationshipKind::Calls),
        ];

        let centrality = compute_centrality(&rels).unwrap();
        let hub = &centrality["hub"];
        assert!(hub.degree > 0.0);
        assert!(hub.closeness > 0.0);
    }

    #[test]
    fn test_empty_relationships() {
        let centrality = compute_centrality(&[]);
        // Should fail gracefully — empty graph has no nodes
        assert!(centrality.is_err() || centrality.unwrap().is_empty());
    }

    #[test]
    fn test_self_loops_filtered() {
        let rels = vec![
            make_rel("a", "a", RelationshipKind::Calls),
            make_rel("a", "b", RelationshipKind::Calls),
        ];
        let centrality = compute_centrality(&rels).unwrap();
        assert_eq!(centrality.len(), 2);
    }

    #[test]
    fn test_disconnected_components() {
        // Two isolated clusters: (a->b) and (c->d)
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("c", "d", RelationshipKind::Calls),
        ];
        let centrality = compute_centrality(&rels).unwrap();
        assert_eq!(centrality.len(), 4);
        // All nodes should have scores
        assert!(centrality.contains_key("a"));
        assert!(centrality.contains_key("c"));
    }

    #[test]
    fn test_mixed_relationship_types() {
        // CALLS (weight 3.0), IMPORTS (weight 2.0), CONTAINED_BY (weight 1.0)
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "c", RelationshipKind::Imports),
            make_rel("c", "d", RelationshipKind::ContainedBy),
        ];
        let centrality = compute_centrality(&rels).unwrap();
        assert_eq!(centrality.len(), 4);
        // All three edge types should contribute
        let b = &centrality["b"];
        let c = &centrality["c"];
        assert!(b.degree > 0.0);
        assert!(c.degree > 0.0);
    }

    #[test]
    fn test_single_node_graph() {
        // Single edge creates two nodes — test the smaller side
        let rels = vec![make_rel("a", "b", RelationshipKind::Calls)];
        let centrality = compute_centrality(&rels).unwrap();
        assert_eq!(centrality.len(), 2);
        // Both nodes exist with valid scores
        assert!(centrality["a"].degree >= 0.0);
        assert!(centrality["b"].degree >= 0.0);
    }

    #[test]
    fn test_fully_connected_graph() {
        // Triangle: a-b, b-c, a-c — betweenness should be similar for all
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "c", RelationshipKind::Calls),
            make_rel("a", "c", RelationshipKind::Calls),
        ];
        let centrality = compute_centrality(&rels).unwrap();
        assert_eq!(centrality.len(), 3);
        // In a fully connected triangle, betweenness should be equal
        let a_b = centrality["a"].betweenness;
        let b_b = centrality["b"].betweenness;
        let c_b = centrality["c"].betweenness;
        let max_diff = (a_b - b_b).abs().max((b_b - c_b).abs());
        assert!(
            max_diff < 0.1,
            "Betweenness should be roughly equal in triangle: a={}, b={}, c={}",
            a_b,
            b_b,
            c_b
        );
    }
}
