//! How much recall does the IVF-PQ index cost against exact search?
//!
//! Once a repository reaches [`ANN_INDEX_THRESHOLD`] symbols, `store_embeddings`
//! builds an IVF-PQ index over the vector column and every semantic and hybrid
//! query goes through it instead of an exact flat scan. IVF-PQ is approximate
//! twice over — a query only visits some partitions, and distances are
//! computed on product-quantised codes — so it can return a different top 10
//! than exact search. This module measures by how much, on a synthetic table,
//! through the production [`Store`] API.
//!
//! The procedure for one table:
//!
//! 1. Store `rows` synthetic unit vectors in batches below the threshold, so
//!    no index is built yet and LanceDB answers with an exact flat scan.
//! 2. For each held-out query, compute the exact top-k by brute-force cosine
//!    and compare it with the flat scan. This must be 1.0; anything else means
//!    the harness, not the index, is wrong.
//! 3. Build the index exactly as production does
//!    ([`Store::create_ann_index`] with [`ANN_INDEX_THRESHOLD`]) and repeat the
//!    comparison. The mean overlap is ANN recall@k.
//!
//! Two synthetic distributions bracket real embeddings: `clustered` (a mixture
//! of tight topical clusters, closer to what an embedding model produces for
//! code) and `uniform` (isotropic noise on the sphere, the worst case for any
//! partition-based index because there is no structure to exploit).

use anyhow::{bail, Context, Result};
use myceliums_storage::schema::{l2_normalize, DEFAULT_EMBEDDING_DIM};
use myceliums_storage::store::ANN_INDEX_THRESHOLD;
use myceliums_storage::{CodeSymbol, Store, SymbolKind};
use serde::Serialize;
use std::collections::HashSet;

use crate::eval::bootstrap::{paired_bootstrap_default, DeltaEstimate};

/// Rows stored per write while loading the table — below the threshold, so
/// loading itself never triggers an index build.
const LOAD_BATCH: usize = 1_000;

/// Shape of the synthetic vectors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Distribution {
    /// Unit vectors scattered around `clusters` random centres.
    Clustered,
    /// Isotropic Gaussian noise normalised to the unit sphere.
    Uniform,
}

impl Distribution {
    /// Stable name for reports.
    pub fn label(&self) -> &'static str {
        match self {
            Distribution::Clustered => "clustered",
            Distribution::Uniform => "uniform",
        }
    }
}

/// One ANN-recall measurement.
#[derive(Debug, Clone)]
pub struct AnnConfig {
    /// Vectors in the table.
    pub rows: usize,
    /// Held-out queries.
    pub queries: usize,
    /// Neighbours compared per query.
    pub k: usize,
    /// Synthetic distribution.
    pub distribution: Distribution,
    /// Cluster count for [`Distribution::Clustered`].
    pub clusters: usize,
    /// Per-dimension noise around a cluster centre, before normalisation.
    pub cluster_spread: f32,
    /// Index threshold handed to `create_ann_index`; production uses
    /// [`ANN_INDEX_THRESHOLD`], tests may pass less to stay fast.
    pub threshold: usize,
    /// Seed of the synthetic data.
    pub seed: u64,
}

impl AnnConfig {
    /// The production-scale check: a table exactly at the threshold.
    pub fn at_threshold(distribution: Distribution) -> Self {
        Self {
            rows: ANN_INDEX_THRESHOLD,
            queries: 200,
            k: 10,
            distribution,
            clusters: 64,
            // With 384 dimensions this puts a member at cosine ≈ 0.7 to its
            // centre and ≈ 0.5 to its cluster mates — tight topical groups,
            // loosely like embeddings of related code.
            cluster_spread: 0.05,
            threshold: ANN_INDEX_THRESHOLD,
            seed: 92,
        }
    }
}

/// What one measurement found.
#[derive(Debug, Clone, Serialize)]
pub struct AnnRecall {
    pub distribution: Distribution,
    pub rows: usize,
    pub queries: usize,
    pub k: usize,
    /// Mean overlap of the flat scan with brute force — the harness check.
    pub flat_recall: f64,
    /// Mean overlap of the IVF-PQ search with brute force.
    pub ann_recall: f64,
    /// 95% bootstrap interval of `ann_recall` over the queries.
    pub ann_recall_ci: (f64, f64),
    /// Share of queries whose exact nearest neighbour the ANN search ranked first.
    pub ann_top1_agreement: f64,
    /// IVF partitions the index was built with.
    pub partitions: u32,
}

/// Run one measurement in a scratch store.
pub async fn measure(config: &AnnConfig) -> Result<AnnRecall> {
    if config.rows < config.threshold {
        bail!(
            "{} rows is below the index threshold {} — nothing approximate to measure",
            config.rows,
            config.threshold
        );
    }
    let dim = DEFAULT_EMBEDDING_DIM as usize;
    let mut rng = Rng::new(config.seed);
    let centres: Vec<Vec<f32>> = (0..config.clusters.max(1))
        .map(|_| rng.unit_vector(dim))
        .collect();
    let sample = |rng: &mut Rng| match config.distribution {
        Distribution::Uniform => rng.unit_vector(dim),
        Distribution::Clustered => {
            let centre = &centres[rng.below(centres.len())];
            let mut v: Vec<f32> = centre
                .iter()
                .map(|c| c + config.cluster_spread * rng.gaussian())
                .collect();
            l2_normalize(&mut v);
            v
        }
    };
    let table: Vec<Vec<f32>> = (0..config.rows).map(|_| sample(&mut rng)).collect();
    let queries: Vec<Vec<f32>> = (0..config.queries).map(|_| sample(&mut rng)).collect();

    let dir = tempfile::tempdir()?;
    let store = Store::open(dir.path(), "ann-recall").await?;
    store.set_embedding_dim(DEFAULT_EMBEDDING_DIM);
    let symbols: Vec<CodeSymbol> = (0..config.rows).map(synthetic_symbol).collect();
    store.store_symbols(&symbols).await?;
    for (offset, chunk) in table.chunks(LOAD_BATCH).enumerate() {
        let pairs = chunk
            .iter()
            .enumerate()
            .map(|(i, v)| (uid(offset * LOAD_BATCH + i), v.clone()))
            .collect();
        store.store_embeddings(pairs).await?;
    }

    let exact: Vec<Vec<String>> = queries
        .iter()
        .map(|q| exact_top_k(&table, q, config.k))
        .collect();

    let flat = overlap(&store, &queries, &exact, config.k).await?;
    let flat_recall = mean(&flat.recall);
    if flat_recall < 1.0 - 1e-9 {
        bail!(
            "the flat scan disagrees with brute force (recall {flat_recall:.4}) before any \
             index exists — the harness is measuring something other than the index"
        );
    }

    let build = store
        .create_ann_index(config.rows, config.threshold)
        .await
        .context("building the IVF-PQ index")?;
    let myceliums_storage::AnnIndexBuild::Built { partitions } = build else {
        bail!(
            "expected an IVF-PQ build at {} rows, got {build:?}",
            config.rows
        );
    };

    let ann = overlap(&store, &queries, &exact, config.k).await?;
    let zeros = vec![0.0; ann.recall.len()];
    let DeltaEstimate {
        delta,
        ci_low,
        ci_high,
    } = paired_bootstrap_default(&ann.recall, &zeros)?;
    Ok(AnnRecall {
        distribution: config.distribution,
        rows: config.rows,
        queries: config.queries,
        k: config.k,
        flat_recall,
        ann_recall: delta,
        ann_recall_ci: (ci_low, ci_high),
        ann_top1_agreement: mean(&ann.top1),
        partitions,
    })
}

struct Overlap {
    recall: Vec<f64>,
    top1: Vec<f64>,
}

/// Compare the store's top-k with the exact top-k (best first) per query.
async fn overlap(
    store: &Store,
    queries: &[Vec<f32>],
    exact: &[Vec<String>],
    k: usize,
) -> Result<Overlap> {
    let mut recall = Vec::with_capacity(queries.len());
    let mut top1 = Vec::with_capacity(queries.len());
    for (query, truth) in queries.iter().zip(exact) {
        let hits = store.vector_search(query, k).await?;
        let wanted: HashSet<&str> = truth.iter().map(String::as_str).collect();
        let found = hits
            .iter()
            .filter(|(symbol, _)| wanted.contains(symbol.uid.as_str()))
            .count();
        recall.push(found as f64 / k as f64);
        let first_agrees = hits.first().map(|(symbol, _)| &symbol.uid) == truth.first();
        top1.push(if first_agrees { 1.0 } else { 0.0 });
    }
    Ok(Overlap { recall, top1 })
}

/// Exact top-k by brute-force cosine, best first. Vectors are unit length, so
/// the dot product is the cosine. Ties break by row index.
fn exact_top_k(table: &[Vec<f32>], query: &[f32], k: usize) -> Vec<String> {
    let mut scored: Vec<(usize, f32)> = table
        .iter()
        .enumerate()
        .map(|(i, v)| (i, v.iter().zip(query).map(|(a, b)| a * b).sum()))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().take(k).map(|(i, _)| uid(i)).collect()
}

fn mean(values: &[f64]) -> f64 {
    crate::eval::metrics::mean(values)
}

fn uid(i: usize) -> String {
    format!("v{i:06}")
}

fn synthetic_symbol(i: usize) -> CodeSymbol {
    CodeSymbol {
        uid: uid(i),
        name: uid(i),
        qualified_name: uid(i),
        kind: SymbolKind::Function,
        file_path: "synthetic.rs".to_string(),
        start_line: 1,
        end_line: 1,
        signature: String::new(),
        content: String::new(),
        repo_id: "ann-recall".to_string(),
        metadata: None,
    }
}

/// SplitMix64 with Box–Muller Gaussians — deterministic synthetic data.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    fn gaussian(&mut self) -> f32 {
        let u1 = self.uniform().max(f64::MIN_POSITIVE);
        let u2 = self.uniform();
        ((-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos()) as f32
    }

    fn unit_vector(&mut self, dim: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|_| self.gaussian()).collect();
        l2_normalize(&mut v);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The harness itself, at a size a unit test can afford: the flat scan
    /// must agree with brute force exactly, the index must really be built,
    /// and recall must land in (0, 1]. The production-scale numbers come from
    /// `cargo run -p myceliums-benchmarks --release --bin ann-recall` and the
    /// ignored test below.
    #[tokio::test]
    async fn the_harness_checks_itself_on_a_small_table() {
        let config = AnnConfig {
            rows: 600,
            queries: 20,
            threshold: 500,
            ..AnnConfig::at_threshold(Distribution::Clustered)
        };
        let result = measure(&config).await.unwrap();
        assert_eq!(result.flat_recall, 1.0);
        assert!(result.partitions >= 4);
        assert!(
            result.ann_recall > 0.0 && result.ann_recall <= 1.0,
            "{result:?}"
        );
        assert!(result.ann_recall_ci.0 <= result.ann_recall);
        assert!(result.ann_recall <= result.ann_recall_ci.1);
    }

    #[tokio::test]
    async fn a_table_below_the_threshold_is_refused() {
        let config = AnnConfig {
            rows: 100,
            threshold: 500,
            ..AnnConfig::at_threshold(Distribution::Uniform)
        };
        assert!(measure(&config).await.is_err());
    }

    /// Production scale: 5 000 vectors, the production threshold. Slow in a
    /// debug build (IVF-PQ training), hence ignored by default:
    /// `cargo test -p myceliums-benchmarks --release -- --ignored ann_recall`.
    ///
    /// Recorded result (METHODOLOGY.md): recall@10 0.37 on the clustered
    /// table — far below what exact search returns. The floor sits under that
    /// so the test reproduces the measurement and catches further loss; raise
    /// it when the index is tuned (refine step, probe count).
    const CLUSTERED_RECALL_FLOOR: f64 = 0.3;

    #[tokio::test]
    #[ignore = "builds a 5 000-row IVF-PQ index; run with --ignored"]
    async fn ann_recall_at_production_threshold() {
        let result = measure(&AnnConfig::at_threshold(Distribution::Clustered))
            .await
            .unwrap();
        assert_eq!(result.rows, ANN_INDEX_THRESHOLD);
        assert_eq!(result.flat_recall, 1.0);
        assert!(result.ann_recall >= CLUSTERED_RECALL_FLOOR, "{result:?}");
    }
}
