//! Community detection: the Leiden algorithm on the structural graph
//! (CALLS, IMPORTS, CONTAINED_BY), with a time and iteration budget (#172).

use crate::config::CommunitySection;
use crate::error::Result;
use crate::leiden::{self, Limits, Stop, WeightedGraph};
use myceliums_storage::{CodeSymbol, Community, Relationship, RelationshipKind, Store};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tracing::{info, warn};
use uuid::Uuid;

/// Default time budget of one community detection run.
pub const DEFAULT_TIME_BUDGET: Duration = Duration::from_secs(60);
/// Default cap on Leiden iterations (each one runs local moves, refinement
/// and aggregation until the partition is stable at every level).
pub const DEFAULT_MAX_ITERATIONS: usize = 100;

/// How long and how many iterations community detection may run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommunityBudget {
    /// Wall-clock limit; `None` runs to convergence (or the iteration cap).
    pub time: Option<Duration>,
    /// Most Leiden iterations.
    pub max_iterations: usize,
}

impl Default for CommunityBudget {
    fn default() -> Self {
        Self {
            time: Some(DEFAULT_TIME_BUDGET),
            max_iterations: DEFAULT_MAX_ITERATIONS,
        }
    }
}

/// Parameters of a community detection run.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CommunityOptions {
    /// Time and iteration limits.
    pub budget: CommunityBudget,
    /// Modularity resolution (higher = more, smaller communities).
    pub resolution: f64,
}

impl Default for CommunityOptions {
    fn default() -> Self {
        Self {
            budget: CommunityBudget::default(),
            resolution: 1.0,
        }
    }
}

impl CommunityOptions {
    /// The options a project's `[community]` section asks for:
    /// `time_budget_secs = 0` means no limit, `max_iterations = 0` counts as
    /// 1, a resolution ≤ 0 as 1.0. Only `analyze` uses them; the on-demand
    /// recomputations use [`CommunityOptions::default`].
    pub fn from_config(section: &CommunitySection) -> Self {
        Self {
            budget: CommunityBudget {
                time: (section.time_budget_secs > 0)
                    .then(|| Duration::from_secs(section.time_budget_secs)),
                max_iterations: section.max_iterations.max(1),
            },
            resolution: if section.resolution > 0.0 {
                section.resolution
            } else {
                1.0
            },
        }
    }
}

/// Whether an index's communities are complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommunityState {
    /// Leiden converged.
    Complete,
    /// The budget ran out: the communities are the best partition found by
    /// then.
    Partial,
    /// Community detection did not run.
    Skipped,
}

impl std::fmt::Display for CommunityState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Complete => "complete",
            Self::Partial => "partial",
            Self::Skipped => "skipped",
        })
    }
}

/// How a community detection run ended; stored in the index metadata
/// (`index_meta` key `communities`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommunityStatus {
    /// Complete, partial or skipped.
    pub state: CommunityState,
    /// Why the communities are partial or skipped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Wall time of the run, graph construction included.
    #[serde(default)]
    pub elapsed_ms: u64,
    /// Leiden iterations started.
    #[serde(default)]
    pub iterations: usize,
    /// Symbols in the structural graph.
    #[serde(default)]
    pub nodes: usize,
    /// Distinct symbol pairs joined by a structural relationship.
    #[serde(default)]
    pub edges: usize,
    /// Modularity (resolution 1) of the partition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modularity: Option<f64>,
}

impl CommunityStatus {
    /// Key under which the status is stored in the `index_meta` table.
    pub const META_KEY: &'static str = "communities";

    /// Detection did not run, for `reason`.
    pub fn skipped(reason: impl Into<String>) -> Self {
        Self {
            state: CommunityState::Skipped,
            reason: Some(reason.into()),
            elapsed_ms: 0,
            iterations: 0,
            nodes: 0,
            edges: 0,
            modularity: None,
        }
    }

    fn empty(elapsed: Duration) -> Self {
        Self {
            state: CommunityState::Complete,
            reason: None,
            elapsed_ms: elapsed.as_millis() as u64,
            iterations: 0,
            nodes: 0,
            edges: 0,
            modularity: None,
        }
    }

    /// True unless detection converged.
    pub fn is_degraded(&self) -> bool {
        self.state != CommunityState::Complete
    }

    /// One sentence for a reader of community results when they are partial
    /// or skipped; `None` when complete.
    pub fn notice(&self) -> Option<String> {
        let reason = self.reason.as_deref().unwrap_or("no reason recorded");
        match self.state {
            CommunityState::Complete => None,
            CommunityState::Partial => Some(format!(
                "Communities are partial: {reason}. They are the best partition found by then; \
                 a larger budget (`[community] time_budget_secs`, `max_iterations` in \
                 .myceliums.toml) lets a re-analysis converge."
            )),
            CommunityState::Skipped => Some(format!(
                "Communities were not detected ({reason}); community tools have nothing to \
                 list. Re-analyze without it to detect them."
            )),
        }
    }

    /// Persists the status inside the index.
    pub async fn record(&self, store: &Store) -> Result<()> {
        let json = serde_json::to_string(self)?;
        store.set_index_meta(Self::META_KEY, &json).await?;
        Ok(())
    }

    /// Reads the status from an index, or `None` for an index analyzed
    /// before it was recorded.
    pub async fn load(store: &Store) -> Result<Option<Self>> {
        match store.get_index_meta(Self::META_KEY).await? {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }
}

/// The communities of one run and how it ended.
#[derive(Debug, Clone)]
pub struct CommunityDetection {
    /// Detected communities, largest first.
    pub communities: Vec<Community>,
    /// How the run ended.
    pub status: CommunityStatus,
}

/// Whether an analysis runs community detection, and with which options.
#[derive(Debug, Clone, PartialEq)]
pub enum CommunityPlan {
    /// Detect communities with these options.
    Run(CommunityOptions),
    /// Do not detect communities, for this reason.
    Skip(String),
}

impl CommunityPlan {
    /// The plan for an analysis: skipped when the caller asks for it (`flag`
    /// names the switch, e.g. `` `--skip-communities` ``) or the project
    /// turns detection off, else run with the project's budget.
    pub fn new(section: Option<&CommunitySection>, skip_requested: bool, flag: &str) -> Self {
        let default_section = CommunitySection::default();
        let section = section.unwrap_or(&default_section);
        if skip_requested {
            Self::Skip(flag.to_string())
        } else if !section.enabled {
            Self::Skip("`[community] enabled = false` in .myceliums.toml".to_string())
        } else {
            Self::Run(CommunityOptions::from_config(section))
        }
    }

    /// Runs the plan: detects (or skips) communities, stores them and the
    /// status in `store`, and returns them.
    pub async fn execute(
        &self,
        store: &Store,
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
    ) -> Result<CommunityDetection> {
        let detection = match self {
            Self::Skip(reason) => {
                info!("Community detection skipped: {reason}");
                CommunityDetection {
                    communities: vec![],
                    status: CommunityStatus::skipped(reason.clone()),
                }
            }
            Self::Run(options) => {
                CommunityDetector::detect_with(symbols, relationships, repo_id, options)?
            }
        };
        store.store_communities(&detection.communities).await?;
        detection.status.record(store).await?;
        Ok(detection)
    }
}

/// The structural graph: symbols joined by CALLS (weight 3), IMPORTS (2)
/// and CONTAINED_BY (1), parallel relationships summed, self-loops dropped.
struct StructuralGraph<'a> {
    /// Symbol UID per node, sorted (node order is deterministic).
    uids: Vec<&'a str>,
    graph: WeightedGraph,
}

impl<'a> StructuralGraph<'a> {
    fn build(relationships: &'a [Relationship]) -> Option<Self> {
        let structural: Vec<(&Relationship, f64)> = relationships
            .iter()
            .filter_map(|r| match r.kind {
                RelationshipKind::Calls => Some((r, 3.0)),
                RelationshipKind::Imports => Some((r, 2.0)),
                RelationshipKind::ContainedBy => Some((r, 1.0)),
                _ => None,
            })
            .collect();
        if structural.is_empty() {
            return None;
        }
        let mut uids: Vec<&str> = structural
            .iter()
            .flat_map(|(r, _)| [r.source_uid.as_str(), r.target_uid.as_str()])
            .collect();
        uids.sort_unstable();
        uids.dedup();
        let index: HashMap<&str, usize> = uids.iter().enumerate().map(|(i, u)| (*u, i)).collect();
        let edges: Vec<(usize, usize, f64)> = structural
            .iter()
            .map(|(r, w)| {
                (
                    index[r.source_uid.as_str()],
                    index[r.target_uid.as_str()],
                    *w,
                )
            })
            .collect();
        let graph = WeightedGraph::from_edges(uids.len(), &edges);
        Some(Self { uids, graph })
    }
}

/// A partition of a [`StructuralGraph`] and how the run ended.
struct Partition {
    /// Member nodes per community, largest community first; members in node
    /// (UID) order.
    groups: Vec<Vec<usize>>,
    status: CommunityStatus,
}

fn partition(
    structural: &StructuralGraph<'_>,
    options: &CommunityOptions,
    started: Instant,
) -> Partition {
    let graph = &structural.graph;
    let limits = Limits {
        deadline: options.budget.time.map(|t| started + t),
        max_iterations: options.budget.max_iterations.max(1),
    };
    let outcome = leiden::leiden(graph, options.resolution, limits);
    let count = outcome
        .membership
        .iter()
        .copied()
        .max()
        .map_or(0, |m| m + 1);
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (v, &c) in outcome.membership.iter().enumerate() {
        groups[c].push(v);
    }
    groups.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a[0].cmp(&b[0])));
    let elapsed = started.elapsed();
    let (state, reason) = match outcome.stop {
        Stop::Converged => (CommunityState::Complete, None),
        Stop::Deadline => (
            CommunityState::Partial,
            Some(format!(
                "the time budget of {} ran out during iteration {} of community detection",
                format_budget(options.budget.time),
                outcome.iterations.max(1)
            )),
        ),
        Stop::IterationCap => (
            CommunityState::Partial,
            Some(format!(
                "community detection stopped after {} iteration(s) (`[community] \
                 max_iterations`) before the partition was stable",
                outcome.iterations
            )),
        ),
    };
    let status = CommunityStatus {
        state,
        reason,
        elapsed_ms: elapsed.as_millis() as u64,
        iterations: outcome.iterations,
        nodes: graph.node_count(),
        edges: graph.edge_count(),
        modularity: Some(leiden::modularity(graph, &outcome.membership, 1.0)),
    };
    if status.is_degraded() {
        warn!(
            "Community detection is partial: {}",
            status.reason.as_deref().unwrap_or_default()
        );
    }
    Partition { groups, status }
}

fn format_budget(time: Option<Duration>) -> String {
    match time {
        Some(t) if t.as_secs() >= 1 => format!("{} s", t.as_secs()),
        Some(t) => format!("{} ms", t.as_millis()),
        None => "unlimited".to_string(),
    }
}

/// The member with the most distinct neighbours names a community.
fn by_degree(structural: &StructuralGraph<'_>, group: &[usize]) -> Vec<usize> {
    let mut members = group.to_vec();
    // Stable: equal degrees keep UID order.
    members.sort_by_key(|&v| std::cmp::Reverse(structural.graph.degree(v)));
    members
}

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
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
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
            .max_by(|a, b| a.1.cmp(b.1).then_with(|| b.0.cmp(a.0)))
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

    /// Detects communities with the default options (60 s budget).
    pub fn detect(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
    ) -> Result<Vec<Community>> {
        Ok(Self::detect_with(
            symbols,
            relationships,
            repo_id,
            &CommunityOptions::default(),
        )?
        .communities)
    }

    /// Detects communities in the symbol graph with the Leiden algorithm
    /// within `options.budget`, labels each one from its members, and says
    /// whether the partition converged.
    pub fn detect_with(
        symbols: &[CodeSymbol],
        relationships: &[Relationship],
        repo_id: &str,
        options: &CommunityOptions,
    ) -> Result<CommunityDetection> {
        let started = Instant::now();
        let structural = match StructuralGraph::build(relationships) {
            Some(s) if symbols.len() >= 2 => s,
            _ => {
                info!("Not enough data for community detection");
                return Ok(CommunityDetection {
                    communities: vec![],
                    status: CommunityStatus::empty(started.elapsed()),
                });
            }
        };
        let Partition { groups, status } = partition(&structural, options, started);

        let uid_to_symbol: HashMap<&str, &CodeSymbol> =
            symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
        let mut communities = Vec::with_capacity(groups.len());
        for (idx, group) in groups.iter().enumerate() {
            let ranked = by_degree(&structural, group);
            let top_symbols_with_info: Vec<(&str, &CodeSymbol)> = ranked
                .iter()
                .map(|&v| structural.uids[v])
                .filter_map(|uid| uid_to_symbol.get(uid).map(|s| (uid, *s)))
                .take(5)
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

            let partition_vec: Vec<String> = group
                .iter()
                .map(|&v| structural.uids[v].to_string())
                .collect();
            let summary =
                Self::generate_summary(&top_symbols_with_info, &partition_vec, &uid_to_symbol);

            communities.push(Community {
                uid: Uuid::new_v4().to_string(),
                label,
                repo_id: repo_id.to_string(),
                member_count: group.len() as u32,
                top_symbols: top_names.join(", "),
                summary,
            });
        }

        info!(
            "Detected {} communities ({}, {} ms, {} iteration(s))",
            communities.len(),
            status.state,
            status.elapsed_ms,
            status.iterations
        );
        Ok(CommunityDetection {
            communities,
            status,
        })
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
    /// How the detection behind these metrics ended (partial when its
    /// budget ran out).
    pub status: CommunityStatus,
}

/// Edge count between a pair of communities.
#[derive(Debug, Clone)]
pub struct CommunityCoupling {
    /// Label of the first community.
    pub community_a: String,
    /// Label of the second community.
    pub community_b: String,
    /// Relationships between members of the two communities.
    pub edge_count: u32,
}

/// Labels per community: the name of its highest-degree member.
fn community_labels(
    structural: &StructuralGraph<'_>,
    groups: &[Vec<usize>],
    uid_to_symbol: &HashMap<&str, &CodeSymbol>,
) -> Vec<String> {
    groups
        .iter()
        .enumerate()
        .map(|(idx, group)| {
            by_degree(structural, group)
                .first()
                .and_then(|&v| uid_to_symbol.get(structural.uids[v]))
                .map(|sym| sym.name.clone())
                .unwrap_or_else(|| format!("community-{}", idx))
        })
        .collect()
}

/// Compute quality metrics for the community partitioning.
///
/// Runs Leiden (default budget) to get partitions, then computes modularity,
/// per-community cohesion, and inter-community coupling from the structural
/// edges.
pub fn compute_community_metrics(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
) -> Result<CommunityMetrics> {
    let started = Instant::now();
    let structural = match StructuralGraph::build(relationships) {
        Some(s) if symbols.len() >= 2 => s,
        _ => {
            return Ok(CommunityMetrics {
                modularity: 0.0,
                cohesion: HashMap::new(),
                coupling: vec![],
                community_count: 0,
                status: CommunityStatus::empty(started.elapsed()),
            })
        }
    };
    let Partition { groups, status } =
        partition(&structural, &CommunityOptions::default(), started);

    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let community_labels = community_labels(&structural, &groups, &uid_to_symbol);
    let mut community_of = vec![0usize; structural.uids.len()];
    for (idx, group) in groups.iter().enumerate() {
        for &v in group {
            community_of[v] = idx;
        }
    }
    let graph = &structural.graph;

    // Cohesion: distinct internal symbol pairs over all possible pairs.
    let mut internal_pairs = vec![0usize; groups.len()];
    // Coupling: relationships whose endpoints sit in two communities.
    let mut coupling_map: HashMap<(usize, usize), u32> = HashMap::new();
    let index: HashMap<&str, usize> = structural
        .uids
        .iter()
        .enumerate()
        .map(|(i, u)| (*u, i))
        .collect();
    for v in 0..graph.node_count() {
        for (u, _) in graph.neighbours(v) {
            if v < u && community_of[u] == community_of[v] {
                internal_pairs[community_of[v]] += 1;
            }
        }
    }
    for rel in relationships.iter().filter(|r| {
        matches!(
            r.kind,
            RelationshipKind::Calls | RelationshipKind::Imports | RelationshipKind::ContainedBy
        ) && r.source_uid != r.target_uid
    }) {
        let (Some(&a), Some(&b)) = (
            index.get(rel.source_uid.as_str()),
            index.get(rel.target_uid.as_str()),
        ) else {
            continue;
        };
        let (ci, cj) = (community_of[a], community_of[b]);
        if ci != cj {
            let key = if ci < cj { (ci, cj) } else { (cj, ci) };
            *coupling_map.entry(key).or_insert(0) += 1;
        }
    }

    let mut cohesion: HashMap<String, f64> = HashMap::new();
    for (idx, group) in groups.iter().enumerate() {
        let n = group.len();
        let density = if n < 2 {
            1.0
        } else {
            internal_pairs[idx] as f64 / ((n * (n - 1)) / 2) as f64
        };
        cohesion.insert(community_labels[idx].clone(), density);
    }

    let mut coupling: Vec<CommunityCoupling> = coupling_map
        .into_iter()
        .map(|((a, b), count)| CommunityCoupling {
            community_a: community_labels.get(a).cloned().unwrap_or_default(),
            community_b: community_labels.get(b).cloned().unwrap_or_default(),
            edge_count: count,
        })
        .collect();
    coupling.sort_by_key(|c| std::cmp::Reverse(c.edge_count));

    Ok(CommunityMetrics {
        modularity: status.modularity.unwrap_or(0.0),
        cohesion,
        coupling,
        community_count: groups.len(),
        status,
    })
}

/// Community id per symbol UID (`0..k`, largest community first) and how
/// the run ended, for evaluations; `None` without structural relationships.
pub fn community_membership(
    relationships: &[Relationship],
    options: &CommunityOptions,
) -> Option<(HashMap<String, usize>, CommunityStatus)> {
    let started = Instant::now();
    let structural = StructuralGraph::build(relationships)?;
    let Partition { groups, status } = partition(&structural, options, started);
    let mut membership = HashMap::with_capacity(structural.uids.len());
    for (idx, group) in groups.iter().enumerate() {
        for &v in group {
            membership.insert(structural.uids[v].to_string(), idx);
        }
    }
    Some((membership, status))
}

/// Modularity (resolution 1) of a partition of the structural graph of
/// `relationships`, for comparing partitions from different algorithms.
/// Symbols missing from `membership` count as singletons.
pub fn structural_modularity(
    relationships: &[Relationship],
    membership: &HashMap<String, usize>,
) -> f64 {
    let Some(structural) = StructuralGraph::build(relationships) else {
        return 0.0;
    };
    let mut ids: HashMap<usize, usize> = HashMap::new();
    let mut next = 0usize;
    let mut dense = Vec::with_capacity(structural.uids.len());
    for uid in &structural.uids {
        let id = match membership.get(*uid) {
            Some(&c) => *ids.entry(c).or_insert_with(|| {
                next += 1;
                next - 1
            }),
            None => {
                next += 1;
                next - 1
            }
        };
        dense.push(id);
    }
    leiden::modularity(&structural.graph, &dense, 1.0)
}

/// Returns a map from symbol UID to community label for use in cross-community analysis.
///
/// The label for each community is derived from the highest-degree symbol in that
/// partition — matching the labelling logic in [`CommunityDetector::detect`].
/// Returns an empty map when there are not enough structural edges for detection.
pub fn compute_uid_to_community_label(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
) -> crate::error::Result<HashMap<String, String>> {
    let started = Instant::now();
    let structural = match StructuralGraph::build(relationships) {
        Some(s) if symbols.len() >= 2 => s,
        _ => return Ok(HashMap::new()),
    };
    let Partition { groups, .. } = partition(&structural, &CommunityOptions::default(), started);
    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let labels = community_labels(&structural, &groups, &uid_to_symbol);
    let mut membership: HashMap<String, String> = HashMap::new();
    for (group, label) in groups.iter().zip(labels) {
        for &v in group {
            membership.insert(structural.uids[v].to_string(), label.clone());
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
        assert_eq!(metrics.community_count, 2, "the two triangles");
        // Two triangles joined by one edge: Q = 2 (6/14 - (7/14)^2) = 0.357.
        assert!(
            (metrics.modularity - 0.357142857).abs() < 1e-6,
            "modularity {}",
            metrics.modularity
        );
        for density in metrics.cohesion.values() {
            assert_eq!(*density, 1.0, "each triangle is complete");
        }
        assert_eq!(metrics.coupling.len(), 1);
        assert_eq!(metrics.coupling[0].edge_count, 1);
        assert_eq!(metrics.status.state, CommunityState::Complete);
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

    /// Regression test: duplicate/parallel edges (both directions of a call, or
    /// several relationships between the same pair) must not abort detection.
    /// Previously graphrs returned `DuplicateEdge` and `detect` silently yielded
    /// zero communities — reproducible on Myceliums' own repository.
    #[test]
    fn test_detect_with_duplicate_edges() {
        let symbols = vec![
            make_symbol("a", "alpha", "src/a.rs"),
            make_symbol("b", "beta", "src/a.rs"),
            make_symbol("c", "gamma", "src/a.rs"),
        ];
        let rels = vec![
            // Same unordered pair in both directions + a repeat → 3 parallel edges.
            make_rel("a", "b", RelationshipKind::Calls),
            make_rel("b", "a", RelationshipKind::Calls),
            make_rel("a", "b", RelationshipKind::Imports),
            make_rel("b", "c", RelationshipKind::Calls),
        ];

        let communities = CommunityDetector::detect(&symbols, &rels, "test").unwrap();
        assert!(
            !communities.is_empty(),
            "duplicate edges must not collapse detection to zero communities"
        );
    }

    /// Two dense clusters of `size` functions, each cluster in its own file,
    /// joined by one call.
    fn two_clusters(size: usize) -> (Vec<CodeSymbol>, Vec<Relationship>) {
        let mut symbols = Vec::new();
        let mut rels = Vec::new();
        for c in 0..2 {
            for i in 0..size {
                let uid = format!("c{c}_{i}");
                symbols.push(make_symbol(&uid, &uid, &format!("src/m{c}.rs")));
                for j in 0..i {
                    rels.push(make_rel(
                        &uid,
                        &format!("c{c}_{j}"),
                        RelationshipKind::Calls,
                    ));
                }
            }
        }
        rels.push(make_rel("c0_0", "c1_0", RelationshipKind::Calls));
        (symbols, rels)
    }

    #[test]
    fn detection_converges_and_records_a_complete_status() {
        let (symbols, rels) = two_clusters(6);
        let detection =
            CommunityDetector::detect_with(&symbols, &rels, "test", &CommunityOptions::default())
                .unwrap();
        assert_eq!(detection.communities.len(), 2);
        assert_eq!(detection.status.state, CommunityState::Complete);
        assert_eq!(detection.status.reason, None);
        assert_eq!(detection.status.nodes, 12);
        assert!(detection.status.modularity.unwrap() > 0.4);
        assert!(detection.status.notice().is_none());
    }

    /// #172: an exhausted budget keeps the partition found so far and says
    /// the communities are partial, and why.
    #[test]
    fn an_exhausted_time_budget_returns_partial_communities() {
        let (symbols, rels) = two_clusters(6);
        let options = CommunityOptions {
            budget: CommunityBudget {
                time: Some(Duration::ZERO),
                max_iterations: 10,
            },
            resolution: 1.0,
        };
        let detection = CommunityDetector::detect_with(&symbols, &rels, "test", &options).unwrap();
        assert_eq!(detection.status.state, CommunityState::Partial);
        let reason = detection.status.reason.clone().unwrap();
        assert!(reason.contains("time budget of 0 ms"), "{reason}");
        // Every symbol of the graph is still in exactly one community.
        let members: u32 = detection.communities.iter().map(|c| c.member_count).sum();
        assert_eq!(members, 12);
        let notice = detection.status.notice().unwrap();
        assert!(notice.starts_with("Communities are partial"), "{notice}");
        assert!(notice.contains("time_budget_secs"), "{notice}");
    }

    #[test]
    fn the_iteration_cap_marks_communities_partial() {
        let (symbols, rels) = two_clusters(6);
        let options = CommunityOptions {
            budget: CommunityBudget {
                time: None,
                max_iterations: 1,
            },
            resolution: 1.0,
        };
        let detection = CommunityDetector::detect_with(&symbols, &rels, "test", &options).unwrap();
        assert_eq!(detection.status.state, CommunityState::Partial);
        assert!(detection
            .status
            .reason
            .as_deref()
            .unwrap()
            .contains("max_iterations"));
        // One iteration already separates the two clusters.
        assert_eq!(detection.communities.len(), 2);
    }

    #[test]
    fn a_skipped_status_says_so_and_round_trips() {
        let status = CommunityStatus::skipped("`--skip-communities`");
        assert_eq!(status.state, CommunityState::Skipped);
        let notice = status.notice().unwrap();
        assert!(notice.contains("not detected"), "{notice}");
        assert!(notice.contains("--skip-communities"), "{notice}");
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains("\"state\":\"skipped\""), "{json}");
        let back: CommunityStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, status);
    }

    #[test]
    fn detection_is_deterministic() {
        let (symbols, rels) = two_clusters(8);
        let a = CommunityDetector::detect(&symbols, &rels, "t").unwrap();
        let b = CommunityDetector::detect(&symbols, &rels, "t").unwrap();
        let shape = |cs: &[Community]| {
            cs.iter()
                .map(|c| (c.label.clone(), c.member_count, c.top_symbols.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(shape(&a), shape(&b));
    }

    #[test]
    fn uid_labels_cover_every_structural_symbol() {
        let (symbols, rels) = two_clusters(5);
        let labels = compute_uid_to_community_label(&symbols, &rels).unwrap();
        assert_eq!(labels.len(), 10);
        assert_ne!(labels["c0_1"], labels["c1_1"]);
        assert_eq!(labels["c0_1"], labels["c0_2"]);
    }
}
