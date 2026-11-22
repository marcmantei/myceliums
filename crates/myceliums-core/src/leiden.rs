//! The Leiden algorithm (Traag, Waltman & van Eck 2019, "From Louvain to
//! Leiden: guaranteeing well-connected communities") for modularity, on a
//! compressed adjacency list, with a deadline (#172).
//!
//! It replaces `graphrs::algorithms::community::leiden`, which is quadratic
//! in the number of nodes: its partition keeps one `HashSet` per community
//! and every move clones the source community (`without`) to compute a
//! delta, removing an emptied community renumbers the whole node-to-community
//! table, the fast local move rebuilds its queue's membership set after every
//! move, and the refinement scans every community of the whole graph for
//! each node it merges. On Django 5.1.4 with its tests (43,236 nodes,
//! 85,676 symbol pairs) it had not finished after 20 minutes; this
//! implementation converges there in under half a second.
//!
//! Here one pass of the local move, the refinement and the aggregation each
//! cost O(edges); the queue is a ring with an in-queue flag. The run stops at
//! a deadline or an iteration cap and returns the partition it has: every
//! local move raises the quality and the refinement and aggregation leave the
//! partition unchanged, so the current partition is always the best so far.
//!
//! The random choices (node order, the refinement's randomized merge) come
//! from a fixed seed: the same graph gives the same partition.

use std::time::Instant;

/// An undirected weighted graph in compressed adjacency form. Both directions
/// of an edge are stored; self-loops are kept apart in `self_weight`.
#[derive(Debug, Clone)]
pub(crate) struct WeightedGraph {
    offsets: Vec<usize>,
    targets: Vec<usize>,
    weights: Vec<f64>,
    /// Sum of `A_ii` (both directions of every edge inside an aggregated
    /// node). Zero on the input graph.
    self_weight: Vec<f64>,
    /// Weighted degree, self-loops included.
    strength: Vec<f64>,
    /// Sum of all strengths: twice the total edge weight.
    two_m: f64,
}

impl WeightedGraph {
    /// Builds the graph from undirected edges `(a, b, weight)` over nodes
    /// `0..n`. Parallel edges are summed, self-loops dropped.
    pub(crate) fn from_edges(n: usize, edges: &[(usize, usize, f64)]) -> Self {
        let mut directed: Vec<(usize, usize, f64)> = Vec::with_capacity(edges.len() * 2);
        for &(a, b, w) in edges {
            if a == b || w <= 0.0 {
                continue;
            }
            directed.push((a, b, w));
            directed.push((b, a, w));
        }
        Self::from_directed(n, directed, vec![0.0; n])
    }

    /// Builds the CSR from both directions of every edge, merging duplicates.
    fn from_directed(
        n: usize,
        mut directed: Vec<(usize, usize, f64)>,
        self_weight: Vec<f64>,
    ) -> Self {
        directed.sort_unstable_by_key(|&(a, b, _)| (a, b));
        let mut offsets = vec![0usize; n + 1];
        let mut targets = Vec::with_capacity(directed.len());
        let mut weights = Vec::with_capacity(directed.len());
        let mut strength = self_weight.clone();
        let mut i = 0;
        while i < directed.len() {
            let (a, b, mut w) = directed[i];
            i += 1;
            while i < directed.len() && directed[i].0 == a && directed[i].1 == b {
                w += directed[i].2;
                i += 1;
            }
            targets.push(b);
            weights.push(w);
            offsets[a + 1] += 1;
            strength[a] += w;
        }
        for v in 0..n {
            offsets[v + 1] += offsets[v];
        }
        let two_m = strength.iter().sum();
        Self {
            offsets,
            targets,
            weights,
            self_weight,
            strength,
            two_m,
        }
    }

    pub(crate) fn node_count(&self) -> usize {
        self.strength.len()
    }

    /// Distinct undirected edges (self-loops excluded).
    pub(crate) fn edge_count(&self) -> usize {
        self.targets.len() / 2
    }

    /// Number of distinct neighbours of `v`.
    pub(crate) fn degree(&self, v: usize) -> usize {
        self.offsets[v + 1] - self.offsets[v]
    }

    pub(crate) fn neighbours(&self, v: usize) -> impl Iterator<Item = (usize, f64)> + '_ {
        let range = self.offsets[v]..self.offsets[v + 1];
        self.targets[range.clone()]
            .iter()
            .copied()
            .zip(self.weights[range].iter().copied())
    }
}

/// Newman–Girvan modularity of `membership` (community id per node) at
/// `resolution`: `Q = Σ_c [ in_c / 2m − γ (tot_c / 2m)² ]`.
pub(crate) fn modularity(graph: &WeightedGraph, membership: &[usize], resolution: f64) -> f64 {
    if graph.two_m <= 0.0 {
        return 0.0;
    }
    let communities = membership.iter().copied().max().map_or(0, |m| m + 1);
    let mut inside = vec![0.0f64; communities];
    let mut total = vec![0.0f64; communities];
    for v in 0..graph.node_count() {
        let c = membership[v];
        total[c] += graph.strength[v];
        inside[c] += graph.self_weight[v];
        for (u, w) in graph.neighbours(v) {
            if membership[u] == c {
                inside[c] += w;
            }
        }
    }
    let two_m = graph.two_m;
    inside
        .iter()
        .zip(&total)
        .map(|(i, t)| i / two_m - resolution * (t / two_m) * (t / two_m))
        .sum()
}

/// Limits of one run.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Stop when this instant passes.
    pub deadline: Option<Instant>,
    /// Most Leiden iterations (each one a full pass of local moves,
    /// refinements and aggregations until the partition is stable).
    pub max_iterations: usize,
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stop {
    /// An iteration changed nothing.
    Converged,
    /// The deadline passed.
    Deadline,
    /// `max_iterations` iterations ran and the last one still moved nodes.
    IterationCap,
}

/// A run's partition and how it ended.
#[derive(Debug, Clone)]
pub(crate) struct Outcome {
    /// Community per node, numbered `0..k` by first appearance.
    pub membership: Vec<usize>,
    pub stop: Stop,
    /// Iterations started (the interrupted one counts).
    pub iterations: usize,
}

/// Leiden's θ: the randomness of the refinement's merge (the paper's value).
const THETA: f64 = 0.01;
/// Most aggregation levels in one iteration; each level shrinks the graph,
/// so this is never reached on a real graph.
const MAX_LEVELS: usize = 64;
/// Nodes processed between two clock reads.
const CLOCK_EVERY: usize = 256;

/// SplitMix64: small, fast and good enough for shuffles.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, 1)`.
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    fn shuffled(&mut self, n: usize) -> Vec<usize> {
        let mut order: Vec<usize> = (0..n).collect();
        for i in (1..n).rev() {
            order.swap(i, self.below(i + 1));
        }
        order
    }
}

/// Reads the clock every [`CLOCK_EVERY`] calls.
struct Clock {
    deadline: Option<Instant>,
    ticks: usize,
    expired: bool,
}

impl Clock {
    fn new(deadline: Option<Instant>) -> Self {
        Self {
            deadline,
            ticks: 0,
            expired: false,
        }
    }

    fn expired(&mut self) -> bool {
        if self.expired {
            return true;
        }
        let Some(deadline) = self.deadline else {
            return false;
        };
        self.ticks += 1;
        if self.ticks % CLOCK_EVERY == 1 && Instant::now() >= deadline {
            self.expired = true;
        }
        self.expired
    }

    fn check_now(&mut self) -> bool {
        if let Some(deadline) = self.deadline {
            if Instant::now() >= deadline {
                self.expired = true;
            }
        }
        self.expired
    }
}

/// Per-community weight accumulator over a node's neighbours.
struct Neighbourhood {
    weight: Vec<f64>,
    seen: Vec<bool>,
    touched: Vec<usize>,
}

impl Neighbourhood {
    fn new(n: usize) -> Self {
        Self {
            weight: vec![0.0; n],
            seen: vec![false; n],
            touched: Vec::new(),
        }
    }

    fn add(&mut self, c: usize, w: f64) {
        if !self.seen[c] {
            self.seen[c] = true;
            self.touched.push(c);
        }
        self.weight[c] += w;
    }

    fn clear(&mut self) {
        for &c in &self.touched {
            self.weight[c] = 0.0;
            self.seen[c] = false;
        }
        self.touched.clear();
    }
}

/// Runs Leiden for modularity at `resolution` on `graph` and returns the
/// best partition found within `limits`.
pub(crate) fn leiden(graph: &WeightedGraph, resolution: f64, limits: Limits) -> Outcome {
    let n = graph.node_count();
    let mut rng = Rng(0x6D79_6365_6C69_756D); // "myceliums"
    let mut clock = Clock::new(limits.deadline);
    let mut membership: Vec<usize> = (0..n).collect();
    if n == 0 || graph.two_m <= 0.0 {
        return Outcome {
            membership,
            stop: Stop::Converged,
            iterations: 0,
        };
    }
    if clock.check_now() {
        return Outcome {
            membership,
            stop: Stop::Deadline,
            iterations: 0,
        };
    }
    let mut iterations = 0;
    let stop = loop {
        if iterations >= limits.max_iterations.max(1) {
            break Stop::IterationCap;
        }
        iterations += 1;
        let (next, changed, interrupted) =
            iterate(graph, &membership, resolution, &mut rng, &mut clock);
        membership = next;
        if interrupted {
            break Stop::Deadline;
        }
        if !changed {
            break Stop::Converged;
        }
    };
    Outcome {
        membership: renumber(&membership),
        stop,
        iterations,
    }
}

/// One Leiden iteration from `start` on the input graph. Returns the
/// partition, whether any node moved, and whether the deadline cut it short.
fn iterate(
    input: &WeightedGraph,
    start: &[usize],
    resolution: f64,
    rng: &mut Rng,
    clock: &mut Clock,
) -> (Vec<usize>, bool, bool) {
    // Input node -> node of the current level's graph.
    let mut level_of: Vec<usize> = (0..input.node_count()).collect();
    let mut owned: Option<WeightedGraph> = None;
    let mut community: Vec<usize> = renumber(start);
    let mut changed = false;
    for _ in 0..MAX_LEVELS {
        let graph = owned.as_ref().unwrap_or(input);
        let moved = move_nodes_fast(graph, &mut community, resolution, rng, clock);
        changed |= moved;
        let flatten = |community: &[usize]| -> Vec<usize> {
            level_of.iter().map(|&v| community[v]).collect()
        };
        if clock.expired() {
            return (flatten(&community), changed, true);
        }
        let communities = count_communities(&community);
        if communities == graph.node_count() {
            return (flatten(&community), changed, false);
        }
        let Some(refined) = refine(graph, &community, resolution, rng, clock) else {
            return (flatten(&community), changed, true);
        };
        let (refined, refined_count) = renumbered_with_count(&refined);
        // Stop when aggregation would not shrink the graph (the refinement
        // merged nothing) or the graph is already one node per community.
        if refined_count >= graph.node_count() || graph.node_count() <= communities {
            return (flatten(&community), changed, false);
        }
        let aggregate = aggregate(graph, &refined, refined_count);
        let mut next_community = vec![0usize; refined_count];
        for v in 0..graph.node_count() {
            next_community[refined[v]] = community[v];
        }
        for v in level_of.iter_mut() {
            *v = refined[*v];
        }
        // Community ids index per-node arrays of the smaller graph.
        community = renumber(&next_community);
        owned = Some(aggregate);
    }
    let flat = level_of.iter().map(|&v| community[v]).collect();
    (flat, changed, false)
}

/// Leiden's fast local move: visit nodes from a queue, move each to the
/// neighbouring (or an empty) community with the largest modularity gain,
/// and re-queue the neighbours a move leaves outside the new community.
/// Returns whether any node moved. Stops early at the deadline.
fn move_nodes_fast(
    graph: &WeightedGraph,
    community: &mut [usize],
    resolution: f64,
    rng: &mut Rng,
    clock: &mut Clock,
) -> bool {
    let n = graph.node_count();
    let scale = resolution / graph.two_m;
    let mut total = vec![0.0f64; n];
    let mut size = vec![0usize; n];
    for v in 0..n {
        total[community[v]] += graph.strength[v];
        size[community[v]] += 1;
    }
    let mut empty: Vec<usize> = (0..n).filter(|&c| size[c] == 0).collect();
    let mut queue: std::collections::VecDeque<usize> = rng.shuffled(n).into();
    let mut queued = vec![true; n];
    let mut around = Neighbourhood::new(n);
    let mut moved = false;
    while let Some(v) = queue.pop_front() {
        if clock.expired() {
            break;
        }
        queued[v] = false;
        let own = community[v];
        let k = graph.strength[v];
        for (u, w) in graph.neighbours(v) {
            around.add(community[u], w);
        }
        total[own] -= k;
        size[own] -= 1;
        let own_gain = around.weight[own] - scale * k * total[own];
        let mut best = own;
        let mut best_gain = own_gain;
        for &c in &around.touched {
            if c == own {
                continue;
            }
            let gain = around.weight[c] - scale * k * total[c];
            if gain > best_gain + 1e-12 {
                best = c;
                best_gain = gain;
            }
        }
        // An empty community gains 0; worth it only if v is not alone.
        if best == own && 0.0 > best_gain + 1e-12 && size[own] > 0 {
            if let Some(c) = empty.pop() {
                best = c;
            }
        }
        around.clear();
        total[best] += k;
        size[best] += 1;
        if size[own] == 0 && best != own {
            empty.push(own);
        }
        if best != own {
            community[v] = best;
            moved = true;
            for (u, _) in graph.neighbours(v) {
                if !queued[u] && community[u] != best {
                    queued[u] = true;
                    queue.push_back(u);
                }
            }
        }
    }
    moved
}

/// Leiden's refinement: inside every community of `community`, start from
/// singletons and merge well-connected nodes into well-connected refined
/// communities, choosing among non-negative gains at random with weight
/// `exp(gain / θ)`. Returns the refined community per node, or `None` at
/// the deadline.
fn refine(
    graph: &WeightedGraph,
    community: &[usize],
    resolution: f64,
    rng: &mut Rng,
    clock: &mut Clock,
) -> Option<Vec<usize>> {
    let n = graph.node_count();
    let scale = resolution / graph.two_m;
    let communities = community.iter().copied().max().map_or(0, |m| m + 1);
    let mut members: Vec<Vec<usize>> = vec![Vec::new(); communities];
    for v in rng.shuffled(n) {
        members[community[v]].push(v);
    }
    let mut community_total = vec![0.0f64; communities];
    for v in 0..n {
        community_total[community[v]] += graph.strength[v];
    }
    let mut refined: Vec<usize> = (0..n).collect();
    let mut refined_total: Vec<f64> = graph.strength.clone();
    let mut refined_size = vec![1usize; n];
    // Weight between a refined community and the rest of its community.
    let mut refined_external = vec![0.0f64; n];
    let mut node_external = vec![0.0f64; n];
    for v in 0..n {
        let ext: f64 = graph
            .neighbours(v)
            .filter(|&(u, _)| community[u] == community[v])
            .map(|(_, w)| w)
            .sum();
        node_external[v] = ext;
        refined_external[v] = ext;
    }
    let mut around = Neighbourhood::new(n);
    let mut candidates: Vec<(usize, f64)> = Vec::new();
    for (c, nodes) in members.iter().enumerate() {
        let s_total = community_total[c];
        for &v in nodes {
            if clock.expired() {
                return None;
            }
            let k = graph.strength[v];
            // Only well-connected nodes still alone are merged.
            if refined_size[refined[v]] != 1 || node_external[v] < scale * k * (s_total - k) - 1e-12
            {
                continue;
            }
            for (u, w) in graph.neighbours(v) {
                if community[u] == c {
                    around.add(refined[u], w);
                }
            }
            let own = refined[v];
            candidates.clear();
            candidates.push((own, 0.0));
            for &r in &around.touched {
                if r == own {
                    continue;
                }
                let r_total = refined_total[r];
                let well_connected =
                    refined_external[r] >= scale * r_total * (s_total - r_total) - 1e-12;
                if !well_connected {
                    continue;
                }
                let gain = around.weight[r] - scale * k * r_total;
                if gain >= 0.0 {
                    candidates.push((r, gain));
                }
            }
            let chosen = choose(&candidates, rng);
            if chosen != own {
                let link = around.weight[chosen];
                refined_external[chosen] += node_external[v] - 2.0 * link;
                refined_total[chosen] += k;
                refined_size[chosen] += 1;
                refined_total[own] = 0.0;
                refined_size[own] = 0;
                refined_external[own] = 0.0;
                refined[v] = chosen;
            }
            around.clear();
        }
    }
    Some(refined)
}

/// Picks a candidate with probability proportional to `exp(gain / θ)`.
fn choose(candidates: &[(usize, f64)], rng: &mut Rng) -> usize {
    let max = candidates
        .iter()
        .map(|&(_, g)| g)
        .fold(f64::NEG_INFINITY, f64::max);
    let weights: Vec<f64> = candidates
        .iter()
        .map(|&(_, g)| ((g - max) / THETA).exp())
        .collect();
    let sum: f64 = weights.iter().sum();
    let mut pick = rng.next_f64() * sum;
    for (i, w) in weights.iter().enumerate() {
        if pick < *w {
            return candidates[i].0;
        }
        pick -= w;
    }
    candidates[candidates.len() - 1].0
}

/// The graph with one node per refined community (numbered `0..count`).
fn aggregate(graph: &WeightedGraph, refined: &[usize], count: usize) -> WeightedGraph {
    let mut self_weight = vec![0.0f64; count];
    let mut directed = Vec::new();
    for v in 0..graph.node_count() {
        let a = refined[v];
        self_weight[a] += graph.self_weight[v];
        for (u, w) in graph.neighbours(v) {
            let b = refined[u];
            if a == b {
                self_weight[a] += w;
            } else {
                directed.push((a, b, w));
            }
        }
    }
    WeightedGraph::from_directed(count, directed, self_weight)
}

fn count_communities(community: &[usize]) -> usize {
    let mut seen = vec![false; community.len().max(1)];
    let mut count = 0;
    for &c in community {
        if c >= seen.len() {
            seen.resize(c + 1, false);
        }
        if !seen[c] {
            seen[c] = true;
            count += 1;
        }
    }
    count
}

/// Renumbers community ids to `0..k` in order of first appearance.
pub(crate) fn renumber(membership: &[usize]) -> Vec<usize> {
    renumbered_with_count(membership).0
}

fn renumbered_with_count(membership: &[usize]) -> (Vec<usize>, usize) {
    let mut map: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
    let out = membership
        .iter()
        .map(|&c| {
            let next = map.len();
            *map.entry(c).or_insert(next)
        })
        .collect();
    (out, map.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn unlimited() -> Limits {
        Limits {
            deadline: None,
            max_iterations: 50,
        }
    }

    /// `k` cliques of `size` nodes joined in a ring by one edge each.
    fn ring_of_cliques(k: usize, size: usize) -> WeightedGraph {
        let mut edges = Vec::new();
        for c in 0..k {
            let base = c * size;
            for i in 0..size {
                for j in (i + 1)..size {
                    edges.push((base + i, base + j, 1.0));
                }
            }
            edges.push((base, ((c + 1) % k) * size + 1, 1.0));
        }
        WeightedGraph::from_edges(k * size, &edges)
    }

    #[test]
    fn finds_the_cliques_of_a_ring_of_cliques() {
        let graph = ring_of_cliques(8, 6);
        let outcome = leiden(&graph, 1.0, unlimited());
        assert_eq!(outcome.stop, Stop::Converged);
        assert_eq!(count_communities(&outcome.membership), 8);
        for c in 0..8 {
            let first = outcome.membership[c * 6];
            for i in 0..6 {
                assert_eq!(outcome.membership[c * 6 + i], first, "clique {c} split");
            }
        }
        let q = modularity(&graph, &outcome.membership, 1.0);
        assert!(q > 0.7, "modularity {q}");
    }

    #[test]
    fn is_deterministic() {
        let graph = ring_of_cliques(12, 5);
        let a = leiden(&graph, 1.0, unlimited());
        let b = leiden(&graph, 1.0, unlimited());
        assert_eq!(a.membership, b.membership);
    }

    #[test]
    fn modularity_of_known_partitions() {
        // Two triangles joined by one edge, m = 7.
        let edges = [
            (0, 1, 1.0),
            (1, 2, 1.0),
            (0, 2, 1.0),
            (3, 4, 1.0),
            (4, 5, 1.0),
            (3, 5, 1.0),
            (2, 3, 1.0),
        ];
        let graph = WeightedGraph::from_edges(6, &edges);
        // in = 6 per triangle (3 edges, both directions), tot = 7 each.
        let expected = 2.0 * (6.0 / 14.0 - (7.0 / 14.0f64).powi(2));
        let q = modularity(&graph, &[0, 0, 0, 1, 1, 1], 1.0);
        assert!((q - expected).abs() < 1e-12, "{q} vs {expected}");
        assert!(modularity(&graph, &[0; 6], 1.0).abs() < 1e-12);
    }

    #[test]
    fn aggregation_preserves_modularity() {
        let graph = ring_of_cliques(4, 4);
        let refined: Vec<usize> = (0..16).map(|v| v / 2).collect();
        let agg = aggregate(&graph, &refined, 8);
        assert!((agg.two_m - graph.two_m).abs() < 1e-9);
        // Partition by clique on both levels.
        let fine: Vec<usize> = (0..16).map(|v| v / 4).collect();
        let coarse: Vec<usize> = (0..8).map(|v| v / 2).collect();
        let q1 = modularity(&graph, &fine, 1.0);
        let q2 = modularity(&agg, &coarse, 1.0);
        assert!((q1 - q2).abs() < 1e-12, "{q1} vs {q2}");
    }

    #[test]
    fn a_passed_deadline_returns_the_starting_partition_as_partial() {
        let graph = ring_of_cliques(10, 5);
        let outcome = leiden(
            &graph,
            1.0,
            Limits {
                deadline: Some(Instant::now() - Duration::from_millis(1)),
                max_iterations: 10,
            },
        );
        assert_eq!(outcome.stop, Stop::Deadline);
        assert_eq!(outcome.membership.len(), 50);
    }

    #[test]
    fn the_iteration_cap_stops_the_run() {
        let graph = ring_of_cliques(10, 5);
        let outcome = leiden(
            &graph,
            1.0,
            Limits {
                deadline: None,
                max_iterations: 1,
            },
        );
        // One iteration moves nodes, so it cannot have converged yet.
        assert_eq!(outcome.stop, Stop::IterationCap);
        assert_eq!(outcome.iterations, 1);
    }

    /// Panics unless every community of `membership` is connected in `graph`
    /// (Leiden's guarantee) and the ids are `0..k`.
    fn assert_connected_communities(graph: &WeightedGraph, membership: &[usize], what: &str) {
        let k = count_communities(membership);
        assert_eq!(membership.iter().copied().max().map_or(0, |m| m + 1), k);
        for c in 0..k {
            let nodes: Vec<usize> = (0..graph.node_count())
                .filter(|&v| membership[v] == c)
                .collect();
            let mut seen = vec![false; graph.node_count()];
            let mut stack = vec![nodes[0]];
            seen[nodes[0]] = true;
            let mut reached = 1;
            while let Some(v) = stack.pop() {
                for (u, _) in graph.neighbours(v) {
                    if membership[u] == c && !seen[u] {
                        seen[u] = true;
                        reached += 1;
                        stack.push(u);
                    }
                }
            }
            assert_eq!(
                reached,
                nodes.len(),
                "{what}: community {c} is disconnected"
            );
        }
    }

    #[test]
    fn every_community_is_connected() {
        let graph = ring_of_cliques(15, 4);
        let outcome = leiden(&graph, 1.0, unlimited());
        assert_connected_communities(&graph, &outcome.membership, "ring of cliques");
    }

    /// Zachary's karate club (34 members, 78 ties, 1-indexed as published).
    fn karate_club() -> WeightedGraph {
        const TIES: [(usize, usize); 78] = [
            (1, 2),
            (1, 3),
            (1, 4),
            (1, 5),
            (1, 6),
            (1, 7),
            (1, 8),
            (1, 9),
            (1, 11),
            (1, 12),
            (1, 13),
            (1, 14),
            (1, 18),
            (1, 20),
            (1, 22),
            (1, 32),
            (2, 3),
            (2, 4),
            (2, 8),
            (2, 14),
            (2, 18),
            (2, 20),
            (2, 22),
            (2, 31),
            (3, 4),
            (3, 8),
            (3, 9),
            (3, 10),
            (3, 14),
            (3, 28),
            (3, 29),
            (3, 33),
            (4, 8),
            (4, 13),
            (4, 14),
            (5, 7),
            (5, 11),
            (6, 7),
            (6, 11),
            (6, 17),
            (7, 17),
            (9, 31),
            (9, 33),
            (9, 34),
            (10, 34),
            (14, 34),
            (15, 33),
            (15, 34),
            (16, 33),
            (16, 34),
            (19, 33),
            (19, 34),
            (20, 34),
            (21, 33),
            (21, 34),
            (23, 33),
            (23, 34),
            (24, 26),
            (24, 28),
            (24, 30),
            (24, 33),
            (24, 34),
            (25, 26),
            (25, 28),
            (25, 32),
            (26, 32),
            (27, 30),
            (27, 34),
            (28, 34),
            (29, 32),
            (29, 34),
            (30, 33),
            (30, 34),
            (31, 33),
            (31, 34),
            (32, 33),
            (32, 34),
            (33, 34),
        ];
        let edges: Vec<(usize, usize, f64)> =
            TIES.iter().map(|&(a, b)| (a - 1, b - 1, 1.0)).collect();
        WeightedGraph::from_edges(34, &edges)
    }

    /// The karate club's best known modularity is 0.4198 (four communities).
    #[test]
    fn reaches_the_known_optimum_on_the_karate_club() {
        let graph = karate_club();
        assert_eq!(graph.edge_count(), 78);
        let outcome = leiden(&graph, 1.0, unlimited());
        assert_eq!(outcome.stop, Stop::Converged);
        let q = modularity(&graph, &outcome.membership, 1.0);
        assert!((q - 0.4198).abs() < 0.005, "modularity {q}");
        assert_connected_communities(&graph, &outcome.membership, "karate club");
    }

    /// Property: on seeded random graphs (planted groups plus noise, varied
    /// size and density, isolated nodes included) every community is
    /// connected, the run converges, and a second run gives the same result.
    #[test]
    fn random_graphs_give_connected_deterministic_communities() {
        for seed in 0..40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9) + 1);
            let n = 20 + rng.below(180);
            let groups = 1 + rng.below(8);
            let p_in = 0.05 + rng.next_f64() * 0.4;
            let p_out = rng.next_f64() * 0.03;
            let mut edges = Vec::new();
            for a in 0..n {
                for b in (a + 1)..n {
                    let p = if a % groups == b % groups {
                        p_in
                    } else {
                        p_out
                    };
                    if rng.next_f64() < p {
                        edges.push((a, b, 1.0 + rng.below(3) as f64));
                    }
                }
            }
            let graph = WeightedGraph::from_edges(n, &edges);
            let first = leiden(&graph, 1.0, unlimited());
            let second = leiden(&graph, 1.0, unlimited());
            assert_eq!(first.stop, Stop::Converged, "seed {seed}");
            assert_eq!(first.membership, second.membership, "seed {seed}");
            assert_connected_communities(&graph, &first.membership, &format!("seed {seed}"));
            assert!(
                modularity(&graph, &first.membership, 1.0) >= -1e-12,
                "seed {seed}"
            );
        }
    }

    #[test]
    fn an_empty_graph_is_trivially_converged() {
        let graph = WeightedGraph::from_edges(3, &[]);
        let outcome = leiden(&graph, 1.0, unlimited());
        assert_eq!(outcome.stop, Stop::Converged);
        assert_eq!(outcome.membership, vec![0, 1, 2]);
    }
}
