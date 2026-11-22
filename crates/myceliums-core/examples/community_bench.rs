//! Community detection on an analyzed index: the Leiden implementation of
//! #172 against the `graphrs` Leiden it replaced, and the structure of the
//! graph (hubs, test code).
//!
//! ```text
//! cargo run --release -p myceliums-core --example community_bench -- \
//!     <data dir> <repo id> [--graphrs-timeout <secs>] [--budget <secs>] \
//!     [--without-prefix tests/] [--graphrs-on-filtered] [--max-iterations <n>]
//! ```
//!
//! `<data dir>` is the `MYCELIUMS_DATA_DIR` the index was built in. The
//! graphrs run happens on a thread and is abandoned (the process exits) when
//! it exceeds `--graphrs-timeout` (default: not run). `--without-prefix`
//! repeats the new run on the graph without the symbols under that path
//! (`--graphrs-on-filtered`: the graphrs run too).

use myceliums_core::{
    community_membership, structural_modularity, CommunityBudget, CommunityOptions,
};
use myceliums_storage::{CodeSymbol, Relationship, RelationshipKind, RepoRegistry, Store};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Args {
    data_dir: PathBuf,
    repo_id: String,
    graphrs_timeout: Option<u64>,
    budget: Option<u64>,
    without_prefix: Option<String>,
    max_iterations: usize,
    graphrs_filtered: bool,
}

fn parse_args() -> Args {
    let mut it = std::env::args().skip(1);
    let data_dir = PathBuf::from(it.next().expect("<data dir>"));
    let repo_id = it.next().expect("<repo id>");
    let mut args = Args {
        data_dir,
        repo_id,
        graphrs_timeout: None,
        budget: None,
        without_prefix: None,
        max_iterations: myceliums_core::community::DEFAULT_MAX_ITERATIONS,
        graphrs_filtered: false,
    };
    while let Some(flag) = it.next() {
        if flag == "--graphrs-on-filtered" {
            args.graphrs_filtered = true;
            continue;
        }
        let value = it.next().expect("flag value");
        match flag.as_str() {
            "--graphrs-timeout" => args.graphrs_timeout = Some(value.parse().unwrap()),
            "--budget" => args.budget = Some(value.parse().unwrap()),
            "--without-prefix" => args.without_prefix = Some(value),
            "--max-iterations" => args.max_iterations = value.parse().unwrap(),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

fn structural(r: &Relationship) -> bool {
    matches!(
        r.kind,
        RelationshipKind::Calls | RelationshipKind::Imports | RelationshipKind::ContainedBy
    ) && r.source_uid != r.target_uid
}

fn describe_graph(symbols: &[CodeSymbol], relationships: &[Relationship], test_prefix: &str) {
    let by_uid: HashMap<&str, &CodeSymbol> = symbols.iter().map(|s| (s.uid.as_str(), s)).collect();
    let mut neighbours: HashMap<&str, HashSet<&str>> = HashMap::new();
    let mut kinds: HashMap<String, usize> = HashMap::new();
    for r in relationships.iter().filter(|r| structural(r)) {
        *kinds.entry(format!("{:?}", r.kind)).or_default() += 1;
        neighbours
            .entry(r.source_uid.as_str())
            .or_default()
            .insert(r.target_uid.as_str());
        neighbours
            .entry(r.target_uid.as_str())
            .or_default()
            .insert(r.source_uid.as_str());
    }
    let pairs: usize = neighbours.values().map(|n| n.len()).sum::<usize>() / 2;
    println!(
        "graph: {} nodes, {} distinct pairs, structural relationships by kind: {:?}",
        neighbours.len(),
        pairs,
        kinds
    );
    let in_tests = |uid: &str| {
        by_uid
            .get(uid)
            .is_some_and(|s| s.file_path.starts_with(test_prefix))
    };
    let test_nodes = neighbours.keys().filter(|u| in_tests(u)).count();
    let mut test_pairs = 0usize;
    for (u, ns) in &neighbours {
        for v in ns {
            if u < v && (in_tests(u) || in_tests(v)) {
                test_pairs += 1;
            }
        }
    }
    println!(
        "under {test_prefix}: {test_nodes} nodes ({:.0} %), {test_pairs} pairs touching them ({:.0} %)",
        100.0 * test_nodes as f64 / neighbours.len().max(1) as f64,
        100.0 * test_pairs as f64 / pairs.max(1) as f64
    );
    let mut degrees: Vec<(usize, &str)> = neighbours.iter().map(|(u, n)| (n.len(), *u)).collect();
    degrees.sort_unstable_by(|a, b| b.cmp(a));
    let mut sorted: Vec<usize> = degrees.iter().map(|d| d.0).collect();
    sorted.sort_unstable();
    let pct = |p: f64| sorted[((sorted.len() - 1) as f64 * p) as usize];
    println!(
        "degree: median {}, p99 {}, max {}",
        pct(0.5),
        pct(0.99),
        sorted.last().unwrap()
    );
    println!("highest-degree nodes:");
    for (d, uid) in degrees.iter().take(10) {
        match by_uid.get(uid) {
            Some(s) => println!(
                "  {d:>6}  {:?} {} ({})",
                s.kind, s.qualified_name, s.file_path
            ),
            None => println!("  {d:>6}  {uid}"),
        }
    }
}

fn run_new(
    label: &str,
    relationships: &[Relationship],
    budget: Option<u64>,
    max_iterations: usize,
) {
    let options = CommunityOptions {
        budget: CommunityBudget {
            time: budget.map(Duration::from_secs),
            max_iterations,
        },
        resolution: 1.0,
    };
    let started = Instant::now();
    let Some((membership, status)) = community_membership(relationships, &options) else {
        println!("{label}: no structural graph");
        return;
    };
    let elapsed = started.elapsed();
    let communities = membership.values().collect::<HashSet<_>>().len();
    println!(
        "{label}: {:.2} s, {} communities, modularity {:.4}, state {}, {} iteration(s), {} nodes, {} edges",
        elapsed.as_secs_f64(),
        communities,
        status.modularity.unwrap_or(0.0),
        status.state,
        status.iterations,
        status.nodes,
        status.edges
    );
}

fn run_graphrs(relationships: &[Relationship], timeout: u64) {
    use graphrs::algorithms::community::leiden::{leiden, QualityFunction};
    use graphrs::{Edge, Graph, GraphSpecs, Node};
    // The graph `CommunityDetector::detect` built before #172.
    let mut weights: HashMap<(String, String), f64> = HashMap::new();
    let mut uids: HashSet<String> = HashSet::new();
    for r in relationships {
        let w = match r.kind {
            RelationshipKind::Calls => 3.0,
            RelationshipKind::Imports => 2.0,
            RelationshipKind::ContainedBy => 1.0,
            _ => continue,
        };
        uids.insert(r.source_uid.clone());
        uids.insert(r.target_uid.clone());
        if r.source_uid == r.target_uid {
            continue;
        }
        let key = if r.source_uid <= r.target_uid {
            (r.source_uid.clone(), r.target_uid.clone())
        } else {
            (r.target_uid.clone(), r.source_uid.clone())
        };
        *weights.entry(key).or_insert(0.0) += w;
    }
    let nodes: Vec<Arc<Node<String, ()>>> = uids.into_iter().map(Node::from_name).collect();
    let edges: Vec<Arc<Edge<String, ()>>> = weights
        .into_iter()
        .map(|((a, b), w)| Edge::with_weight(a, b, w))
        .collect();
    let graph =
        Graph::<String, ()>::new_from_nodes_and_edges(nodes, edges, GraphSpecs::undirected())
            .expect("graph");
    let (tx, rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    std::thread::spawn(move || {
        let result = leiden(
            &graph,
            true,
            QualityFunction::Modularity,
            Some(1.0),
            None,
            None,
        );
        let _ = tx.send(result.map_err(|e| format!("{e:?}")));
    });
    match rx.recv_timeout(Duration::from_secs(timeout)) {
        Ok(Ok(partitions)) => {
            let elapsed = started.elapsed();
            let mut membership = HashMap::new();
            for (i, part) in partitions.iter().enumerate() {
                for uid in part {
                    membership.insert(uid.clone(), i);
                }
            }
            println!(
                "graphrs leiden: {:.2} s, {} communities, modularity {:.4}",
                elapsed.as_secs_f64(),
                partitions.len(),
                structural_modularity(relationships, &membership)
            );
        }
        Ok(Err(e)) => println!("graphrs leiden: failed: {e}"),
        Err(_) => {
            println!("graphrs leiden: not finished after {timeout} s (abandoned)");
            std::process::exit(0);
        }
    }
}

#[tokio::main]
async fn main() {
    let args = parse_args();
    let db = RepoRegistry::repo_db_path(&args.data_dir, &args.repo_id);
    let store = Store::open_existing(&db, &args.repo_id)
        .await
        .expect("open index");
    let symbols = store.get_symbols().await.expect("symbols");
    let relationships = store.get_relationships().await.expect("relationships");
    println!(
        "{}: {} symbols, {} relationships",
        args.repo_id,
        symbols.len(),
        relationships.len()
    );
    let prefix = args
        .without_prefix
        .clone()
        .unwrap_or_else(|| "tests/".into());
    describe_graph(&symbols, &relationships, &prefix);
    run_new(
        "new leiden",
        &relationships,
        args.budget,
        args.max_iterations,
    );
    let mut filtered: Option<Vec<Relationship>> = None;
    if let Some(prefix) = &args.without_prefix {
        // Drops every relationship that touches a symbol under the prefix;
        // file nodes (not symbols) stay, joined to the symbols they contain.
        let inside: HashSet<&str> = symbols
            .iter()
            .filter(|s| s.file_path.starts_with(prefix.as_str()))
            .map(|s| s.uid.as_str())
            .collect();
        let kept: Vec<Relationship> = relationships
            .iter()
            .filter(|r| {
                !inside.contains(r.source_uid.as_str()) && !inside.contains(r.target_uid.as_str())
            })
            .cloned()
            .collect();
        run_new(
            &format!("new leiden without {prefix}"),
            &kept,
            args.budget,
            args.max_iterations,
        );
        filtered = Some(kept);
    }
    if let Some(timeout) = args.graphrs_timeout {
        match filtered.as_deref().filter(|_| args.graphrs_filtered) {
            Some(kept) => run_graphrs(kept, timeout),
            None => run_graphrs(&relationships, timeout),
        }
    }
}
