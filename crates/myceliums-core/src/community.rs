use anyhow::Result;
use graphrs::algorithms::centrality::degree::degree_centrality;
use graphrs::algorithms::community::leiden::{leiden, QualityFunction};
use graphrs::{Edge, Graph, GraphSpecs, Node};
use myceliums_storage::{CodeSymbol, Community, Relationship, RelationshipKind};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

/// Detects code communities using the Leiden algorithm on the call graph.
///
/// Communities group tightly-connected symbols together, helping to
/// identify modules, features, or subsystems within a codebase.
pub struct CommunityDetector;

impl CommunityDetector {
    /// Generate a one-sentence summary from top symbols and their kinds.
    fn generate_summary(
        top_members: &[(&str, &CodeSymbol)],
        partition_uids: &[String],
        uid_to_symbol: &HashMap<&str, &CodeSymbol>,
    ) -> String {
        if top_members.is_empty() {
            return String::new();
        }

        // Extract top symbol names and kinds
        let top_symbols: Vec<String> = top_members
            .iter()
            .map(|(_, sym)| sym.name.clone())
            .collect();

        // Determine dominant kind (most common symbol type in top members)
        let mut kind_count: HashMap<String, u32> = HashMap::new();
        for (_, sym) in top_members {
            *kind_count.entry(sym.kind.to_string()).or_insert(0) += 1;
        }
        let dominant_kind = kind_count
            .iter()
            .max_by_key(|&(_, count)| count)
            .map(|(kind, _)| kind.clone())
            .unwrap_or_else(|| "module".to_string());

        // Infer purpose from file paths if possible
        let mut path_components: HashMap<String, u32> = HashMap::new();
        for uid in partition_uids.iter().take(20) {
            if let Some(sym) = uid_to_symbol.get(uid.as_str()) {
                // Extract directory names from path
                let path_parts: Vec<&str> = sym.file_path.split('/').collect();
                for part in path_parts {
                    if !part.is_empty() && part != "src" && !part.contains('.') {
                        *path_components.entry(part.to_lowercase()).or_insert(0) += 1;
                    }
                }
            }
        }

        let inferred_purpose = path_components
            .iter()
            .max_by_key(|&(_, count)| count)
            .map(|(kind, _)| kind.clone())
            .unwrap_or_else(|| "functionality".to_string());

        // Build the summary
        let symbols_str = if top_symbols.len() > 3 {
            format!(
                "{}, {}, ... ({})",
                top_symbols[0],
                top_symbols[1],
                top_symbols.len()
            )
        } else {
            top_symbols.join(", ")
        };

        format!(
            "{} module: {} — handles {}",
            dominant_kind, symbols_str, inferred_purpose
        )
    }

    pub fn detect(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
    ) -> Result<Vec<Community>> {
        // Build a graphrs undirected graph from structural relationships
        // Use CALLS (weight 3.0), IMPORTS (weight 2.0), and CONTAINED_BY (weight 1.0)
        // to give Leiden enough signal for partitioning
        let structural_rels: Vec<(&Relationship, f64)> = relationships
            .iter()
            .filter_map(|r| match r.kind {
                RelationshipKind::Calls => Some((r, 3.0)),
                RelationshipKind::Imports => Some((r, 2.0)),
                RelationshipKind::ContainedBy => Some((r, 1.0)),
                _ => None,
            })
            .collect();

        if structural_rels.is_empty() || symbols.len() < 2 {
            info!("Not enough data for community detection");
            return Ok(vec![]);
        }

        // Collect all symbol UIDs involved in structural relationships
        let mut symbol_uids: HashSet<String> = HashSet::new();
        for (rel, _) in &structural_rels {
            symbol_uids.insert(rel.source_uid.clone());
            symbol_uids.insert(rel.target_uid.clone());
        }

        // Build UID -> symbol index
        let uid_to_symbol: HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

        // Create graphrs nodes and edges with weighted relationships
        let nodes: Vec<Arc<Node<String, ()>>> = symbol_uids
            .iter()
            .map(|uid| Node::from_name(uid.clone()))
            .collect();

        let edges: Vec<Arc<Edge<String, ()>>> = structural_rels
            .iter()
            .filter(|(rel, _)| rel.source_uid != rel.target_uid) // Filter self-loops
            .map(|(rel, weight)| {
                Edge::with_weight(rel.source_uid.clone(), rel.target_uid.clone(), *weight)
            })
            .collect();

        // Create undirected graph for Leiden (it requires undirected)
        let graph =
            Graph::<String, ()>::new_from_nodes_and_edges(nodes, edges, GraphSpecs::undirected());

        let graph = match graph {
            Ok(g) => g,
            Err(e) => {
                info!("Failed to create graph: {:?}", e);
                return Ok(vec![]);
            }
        };

        // Run Leiden algorithm with Modularity (better for sparse code graphs than CPM)
        let partitions = leiden(
            &graph,
            true,                        // weighted
            QualityFunction::Modularity, // Modularity works better for sparse call graphs
            Some(1.0),                   // resolution
            None,                        // theta
            None,                        // gamma
        );

        let partitions = match partitions {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!("Leiden failed: {:?}, falling back to single community", e);
                let all_symbols: Vec<(&str, &CodeSymbol)> = symbols
                    .iter()
                    .take(5)
                    .map(|s| (s.uid.as_str(), s))
                    .collect();
                let summary = Self::generate_summary(
                    &all_symbols,
                    &symbols.iter().map(|s| s.uid.clone()).collect::<Vec<_>>(),
                    &uid_to_symbol,
                );
                return Ok(vec![Community {
                    uid: Uuid::new_v4().to_string(),
                    label: "all".to_string(),
                    repo_id: repo_id.to_string(),
                    member_count: symbols.len() as u32,
                    top_symbols: symbols
                        .iter()
                        .take(5)
                        .map(|s| s.name.clone())
                        .collect::<Vec<_>>()
                        .join(", "),
                    summary,
                }]);
            }
        };

        // Compute degree centrality for labeling
        let centrality = degree_centrality(&graph);

        // Convert partitions to Community structs
        let mut communities = Vec::new();
        for (idx, partition) in partitions.iter().enumerate() {
            if partition.is_empty() {
                continue;
            }

            // Find top symbols by degree centrality within this community
            let mut members_with_centrality: Vec<(&str, f64)> = partition
                .iter()
                .map(|uid| {
                    let c = centrality.get(uid).copied().unwrap_or(0.0);
                    (uid.as_str(), c)
                })
                .collect();
            members_with_centrality
                .sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let top_symbols_with_info: Vec<(&str, &CodeSymbol)> = members_with_centrality
                .iter()
                .take(5)
                .filter_map(|(uid, _)| uid_to_symbol.get(uid).map(|s| (*uid, *s)))
                .collect();

            let top_names: Vec<String> = top_symbols_with_info
                .iter()
                .map(|(_, s)| s.name.clone())
                .collect();

            let label = if top_names.is_empty() {
                format!("community-{}", idx)
            } else {
                top_names[0].clone()
            };

            let partition_vec: Vec<String> = partition.iter().cloned().collect();
            let summary =
                Self::generate_summary(&top_symbols_with_info, &partition_vec, &uid_to_symbol);

            communities.push(Community {
                uid: Uuid::new_v4().to_string(),
                label,
                repo_id: repo_id.to_string(),
                member_count: partition.len() as u32,
                top_symbols: top_names.join(", "),
                summary,
            });
        }

        info!("Detected {} communities", communities.len());
        Ok(communities)
    }
}

/// Quality metrics for the overall community partitioning.
#[derive(Debug, Clone)]
pub struct CommunityMetrics {
    /// Overall modularity score for the partition (higher = better separation)
    pub modularity: f64,
    /// Per-community cohesion (internal density): community_label -> density [0,1]
    pub cohesion: HashMap<String, f64>,
    /// Inter-community coupling: (label_a, label_b) -> edge count
    pub coupling: Vec<CommunityCoupling>,
    /// Total communities
    pub community_count: usize,
}

/// Edge count between a pair of communities.
#[derive(Debug, Clone)]
pub struct CommunityCoupling {
    pub community_a: String,
    pub community_b: String,
    pub edge_count: u32,
}

/// Compute quality metrics for the community partitioning.
///
/// Runs Leiden to get partitions, then computes modularity, per-community
/// cohesion, and inter-community coupling from the structural edges.
pub fn compute_community_metrics(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
) -> Result<CommunityMetrics> {
    let structural_rels: Vec<(&Relationship, f64)> = relationships
        .iter()
        .filter_map(|r| match r.kind {
            RelationshipKind::Calls => Some((r, 3.0)),
            RelationshipKind::Imports => Some((r, 2.0)),
            RelationshipKind::ContainedBy => Some((r, 1.0)),
            _ => None,
        })
        .collect();

    if structural_rels.is_empty() || symbols.len() < 2 {
        return Ok(CommunityMetrics {
            modularity: 0.0,
            cohesion: HashMap::new(),
            coupling: vec![],
            community_count: 0,
        });
    }

    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let mut symbol_uids: HashSet<String> = HashSet::new();
    for (rel, _) in &structural_rels {
        symbol_uids.insert(rel.source_uid.clone());
        symbol_uids.insert(rel.target_uid.clone());
    }

    let nodes: Vec<Arc<Node<String, ()>>> = symbol_uids
        .iter()
        .map(|uid| Node::from_name(uid.clone()))
        .collect();

    let edges: Vec<Arc<Edge<String, ()>>> = structural_rels
        .iter()
        .filter(|(rel, _)| rel.source_uid != rel.target_uid)
        .map(|(rel, weight)| {
            Edge::with_weight(rel.source_uid.clone(), rel.target_uid.clone(), *weight)
        })
        .collect();

    let graph =
        Graph::<String, ()>::new_from_nodes_and_edges(nodes, edges, GraphSpecs::undirected());

    let graph = match graph {
        Ok(g) => g,
        Err(_) => {
            return Ok(CommunityMetrics {
                modularity: 0.0,
                cohesion: HashMap::new(),
                coupling: vec![],
                community_count: 0,
            })
        }
    };

    let partitions = leiden(
        &graph,
        true,
        QualityFunction::Modularity,
        Some(1.0),
        None,
        None,
    );

    let partitions = match partitions {
        Ok(p) => p,
        Err(_) => {
            return Ok(CommunityMetrics {
                modularity: 0.0,
                cohesion: HashMap::new(),
                coupling: vec![],
                community_count: 0,
            })
        }
    };

    let centrality = degree_centrality(&graph);

    // Build UID -> community index + label
    let mut uid_to_community: HashMap<&str, usize> = HashMap::new();
    let mut community_labels: Vec<String> = Vec::new();

    for (idx, partition) in partitions.iter().enumerate() {
        let label = partition
            .iter()
            .max_by(|a, b| {
                let ca = centrality.get(*a).copied().unwrap_or(0.0);
                let cb = centrality.get(*b).copied().unwrap_or(0.0);
                ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|uid| uid_to_symbol.get(uid.as_str()))
            .map(|sym| sym.name.clone())
            .unwrap_or_else(|| format!("community-{}", idx));

        community_labels.push(label);
        for uid in partition {
            uid_to_community.insert(uid.as_str(), idx);
        }
    }

    // Compute modularity: Q = (1/2m) * sum_ij [ A_ij - (k_i * k_j)/(2m) ] * delta(c_i, c_j)
    let total_weight: f64 = structural_rels
        .iter()
        .filter(|(rel, _)| rel.source_uid != rel.target_uid)
        .map(|(_, w)| w)
        .sum();
    let two_m = total_weight * 2.0; // undirected: each edge counted once, but formula uses 2m

    let mut modularity = 0.0;
    if two_m > 0.0 {
        // Strength of each node (weighted degree)
        let mut strength: HashMap<&str, f64> = HashMap::new();
        for (rel, weight) in &structural_rels {
            if rel.source_uid != rel.target_uid {
                *strength.entry(rel.source_uid.as_str()).or_insert(0.0) += weight;
                *strength.entry(rel.target_uid.as_str()).or_insert(0.0) += weight;
            }
        }

        for (rel, weight) in &structural_rels {
            if rel.source_uid == rel.target_uid {
                continue;
            }
            let ci = uid_to_community.get(rel.source_uid.as_str());
            let cj = uid_to_community.get(rel.target_uid.as_str());
            if ci == cj {
                let ki = strength
                    .get(rel.source_uid.as_str())
                    .copied()
                    .unwrap_or(0.0);
                let kj = strength
                    .get(rel.target_uid.as_str())
                    .copied()
                    .unwrap_or(0.0);
                modularity += weight - (ki * kj) / two_m;
            }
        }
        modularity /= two_m;
    }

    // Compute per-community cohesion (internal density)
    let mut cohesion: HashMap<String, f64> = HashMap::new();
    for (idx, partition) in partitions.iter().enumerate() {
        let n = partition.len();
        if n < 2 {
            cohesion.insert(community_labels[idx].clone(), 1.0);
            continue;
        }
        let max_edges = (n * (n - 1)) / 2;
        let internal_edges = structural_rels
            .iter()
            .filter(|(rel, _)| {
                rel.source_uid != rel.target_uid
                    && uid_to_community.get(rel.source_uid.as_str()) == Some(&idx)
                    && uid_to_community.get(rel.target_uid.as_str()) == Some(&idx)
            })
            .count();
        let density = if max_edges > 0 {
            internal_edges as f64 / max_edges as f64
        } else {
            0.0
        };
        cohesion.insert(community_labels[idx].clone(), density);
    }

    // Compute inter-community coupling
    let mut coupling_map: HashMap<(usize, usize), u32> = HashMap::new();
    for (rel, _) in &structural_rels {
        if rel.source_uid == rel.target_uid {
            continue;
        }
        let ci = uid_to_community.get(rel.source_uid.as_str()).copied();
        let cj = uid_to_community.get(rel.target_uid.as_str()).copied();
        if let (Some(ci), Some(cj)) = (ci, cj) {
            if ci != cj {
                let key = if ci < cj { (ci, cj) } else { (cj, ci) };
                *coupling_map.entry(key).or_insert(0) += 1;
            }
        }
    }

    let mut coupling: Vec<CommunityCoupling> = coupling_map
        .into_iter()
        .map(|((a, b), count)| CommunityCoupling {
            community_a: community_labels.get(a).cloned().unwrap_or_default(),
            community_b: community_labels.get(b).cloned().unwrap_or_default(),
            edge_count: count,
        })
        .collect();
    coupling.sort_by(|a, b| b.edge_count.cmp(&a.edge_count));

    Ok(CommunityMetrics {
        modularity,
        cohesion,
        coupling,
        community_count: partitions.len(),
    })
}

/// Returns a map from symbol UID to community label for use in cross-community analysis.
///
/// The label for each community is derived from the highest-degree symbol in that
/// partition — matching the labelling logic in [`CommunityDetector::detect`].
/// Returns an empty map when there are not enough CALLS edges for detection.
pub fn compute_uid_to_community_label(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
) -> anyhow::Result<HashMap<String, String>> {
    // Use same structural relationships as detect(): CALLS + IMPORTS + CONTAINED_BY
    let structural_rels: Vec<(&Relationship, f64)> = relationships
        .iter()
        .filter_map(|r| match r.kind {
            RelationshipKind::Calls => Some((r, 3.0)),
            RelationshipKind::Imports => Some((r, 2.0)),
            RelationshipKind::ContainedBy => Some((r, 1.0)),
            _ => None,
        })
        .collect();

    if structural_rels.is_empty() || symbols.len() < 2 {
        return Ok(HashMap::new());
    }

    let mut symbol_uids: HashSet<String> = HashSet::new();
    for (rel, _) in &structural_rels {
        symbol_uids.insert(rel.source_uid.clone());
        symbol_uids.insert(rel.target_uid.clone());
    }

    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    let nodes: Vec<Arc<Node<String, ()>>> = symbol_uids
        .iter()
        .map(|uid| Node::from_name(uid.clone()))
        .collect();

    let edges: Vec<Arc<Edge<String, ()>>> = structural_rels
        .iter()
        .filter(|(rel, _)| rel.source_uid != rel.target_uid) // Filter self-loops
        .map(|(rel, weight)| {
            Edge::with_weight(rel.source_uid.clone(), rel.target_uid.clone(), *weight)
        })
        .collect();

    let graph =
        Graph::<String, ()>::new_from_nodes_and_edges(nodes, edges, GraphSpecs::undirected());

    let graph = match graph {
        Ok(g) => g,
        Err(_) => return Ok(HashMap::new()),
    };

    let partitions = leiden(
        &graph,
        true,
        QualityFunction::Modularity,
        Some(1.0),
        None,
        None,
    );

    let partitions = match partitions {
        Ok(p) => p,
        Err(_) => {
            // Single-community fallback: all symbols get label "all"
            let membership = symbol_uids
                .into_iter()
                .map(|uid| (uid, "all".to_string()))
                .collect();
            return Ok(membership);
        }
    };

    let centrality = degree_centrality(&graph);

    let mut membership: HashMap<String, String> = HashMap::new();
    for (idx, partition) in partitions.iter().enumerate() {
        if partition.is_empty() {
            continue;
        }

        // Pick the highest-centrality member as the community label
        let label = partition
            .iter()
            .max_by(|a, b| {
                let ca = centrality.get(*a).copied().unwrap_or(0.0);
                let cb = centrality.get(*b).copied().unwrap_or(0.0);
                ca.partial_cmp(&cb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .and_then(|uid| uid_to_symbol.get(uid.as_str()))
            .map(|sym| sym.name.clone())
            .unwrap_or_else(|| format!("community-{}", idx));

        for uid in partition {
            membership.insert(uid.clone(), label.clone());
        }
    }

    Ok(membership)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    #[test]
    fn test_summary_generation() {
        let mut uid_to_symbol = HashMap::new();

        // Create mock symbols
        let sym1 = CodeSymbol {
            uid: "uid1".to_string(),
            name: "login".to_string(),
            qualified_name: "auth::login".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/auth/login.rs".to_string(),
            start_line: 10,
            end_line: 20,
            signature: "fn login(user: &str) -> bool".to_string(),
            content: "fn login...".to_string(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        };

        let sym2 = CodeSymbol {
            uid: "uid2".to_string(),
            name: "logout".to_string(),
            qualified_name: "auth::logout".to_string(),
            kind: SymbolKind::Function,
            file_path: "src/auth/logout.rs".to_string(),
            start_line: 30,
            end_line: 40,
            signature: "fn logout()".to_string(),
            content: "fn logout...".to_string(),
            repo_id: "test-repo".to_string(),
            metadata: None,
        };

        uid_to_symbol.insert("uid1", &sym1);
        uid_to_symbol.insert("uid2", &sym2);

        let top_members = vec![("uid1", &sym1), ("uid2", &sym2)];
        let partition_uids = vec!["uid1".to_string(), "uid2".to_string()];

        let summary =
            CommunityDetector::generate_summary(&top_members, &partition_uids, &uid_to_symbol);

        // Verify the summary contains key elements
        assert!(!summary.is_empty(), "Summary should not be empty");
        assert!(
            summary.contains("Function"),
            "Summary should mention Function kind"
        );
        assert!(
            summary.contains("login"),
            "Summary should contain top symbol"
        );
        assert!(
            summary.contains("logout"),
            "Summary should contain second symbol"
        );
        assert!(
            summary.contains("auth"),
            "Summary should mention auth domain"
        );
        println!("Generated summary: {}", summary);
    }

    #[test]
    fn test_empty_members_returns_empty_summary() {
        let uid_to_symbol: HashMap<&str, &CodeSymbol> = HashMap::new();
        let top_members: Vec<(&str, &CodeSymbol)> = vec![];
        let partition_uids = vec![];

        let summary =
            CommunityDetector::generate_summary(&top_members, &partition_uids, &uid_to_symbol);

        assert_eq!(summary, "", "Empty members should produce empty summary");
    }

    fn make_symbol(uid: &str, name: &str, file: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            signature: String::new(),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_rel(
        source: &str,
        target: &str,
        kind: myceliums_storage::RelationshipKind,
    ) -> myceliums_storage::Relationship {
        myceliums_storage::Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_compute_community_metrics_basic() {
        // Two clusters: (a,b,c) and (d,e,f) with one cross-community edge
        let symbols = vec![
            make_symbol("a", "alpha", "src/a.rs"),
            make_symbol("b", "beta", "src/a.rs"),
            make_symbol("c", "gamma", "src/a.rs"),
            make_symbol("d", "delta", "src/b.rs"),
            make_symbol("e", "epsilon", "src/b.rs"),
            make_symbol("f", "zeta", "src/b.rs"),
        ];
        let rels = vec![
            // Cluster 1 internal
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "c", RelationshipKind::Calls),
            make_rel("a", "c", RelationshipKind::Calls),
            // Cluster 2 internal
            make_rel("d", "e", RelationshipKind::Calls),
            make_rel("e", "f", RelationshipKind::Calls),
            make_rel("d", "f", RelationshipKind::Calls),
            // Cross-community edge
            make_rel("c", "d", RelationshipKind::Calls),
        ];

        let metrics = compute_community_metrics(&symbols, &rels).unwrap();
        assert!(
            metrics.community_count >= 1,
            "Should detect at least 1 community"
        );
        // Modularity should be positive for well-separated clusters
        // (exact value depends on Leiden's partitioning)
    }

    #[test]
    fn test_community_metrics_empty_graph() {
        let metrics = compute_community_metrics(&[], &[]).unwrap();
        assert_eq!(metrics.modularity, 0.0);
        assert_eq!(metrics.community_count, 0);
        assert!(metrics.cohesion.is_empty());
        assert!(metrics.coupling.is_empty());
    }

    #[test]
    fn test_community_metrics_single_edge() {
        let symbols = vec![
            make_symbol("a", "alpha", "src/a.rs"),
            make_symbol("b", "beta", "src/b.rs"),
        ];
        let rels = vec![make_rel("a", "b", RelationshipKind::Calls)];

        let metrics = compute_community_metrics(&symbols, &rels).unwrap();
        // With only 2 nodes and 1 edge, Leiden may put them in one community
        assert!(metrics.community_count >= 1);
    }

    #[test]
    fn test_community_coupling_structure() {
        // Verify coupling entries have valid community names
        let symbols = vec![
            make_symbol("a", "alpha", "src/a.rs"),
            make_symbol("b", "beta", "src/a.rs"),
            make_symbol("c", "gamma", "src/b.rs"),
            make_symbol("d", "delta", "src/b.rs"),
        ];
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("c", "d", RelationshipKind::Calls),
            make_rel("a", "c", RelationshipKind::Calls),
        ];

        let metrics = compute_community_metrics(&symbols, &rels).unwrap();
        for c in &metrics.coupling {
            assert!(
                !c.community_a.is_empty(),
                "Coupling community_a should not be empty"
            );
            assert!(
                !c.community_b.is_empty(),
                "Coupling community_b should not be empty"
            );
            assert!(c.edge_count > 0, "Coupling edge_count should be positive");
        }
    }

    #[test]
    fn test_community_cohesion_values() {
        // Cohesion values should be in [0, 1]
        let symbols = vec![
            make_symbol("a", "alpha", "src/a.rs"),
            make_symbol("b", "beta", "src/a.rs"),
            make_symbol("c", "gamma", "src/a.rs"),
        ];
        let rels = vec![
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "c", RelationshipKind::Calls),
            make_rel("a", "c", RelationshipKind::Calls),
        ];

        let metrics = compute_community_metrics(&symbols, &rels).unwrap();
        for (label, density) in &metrics.cohesion {
            assert!(
                *density >= 0.0 && *density <= 1.0,
                "Cohesion for {} should be in [0,1], got {}",
                label,
                density
            );
        }
    }
}
