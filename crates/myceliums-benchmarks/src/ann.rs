//! How much recall does the ANN vector index cost against exact search?
//!
//! Once a repository reaches [`ANN_INDEX_THRESHOLD`] symbols, `store_embeddings`
//! builds an ANN index over the vector column and every semantic and hybrid
//! query goes through it instead of an exact flat scan. An approximate index
//! can return a different top 10 than exact search. This module measures by
//! how much, through the production [`Store`] API.
//!
//! The procedure for one table:
//!
//! 1. Store the table's unit vectors with indexing disabled, so LanceDB
//!    answers with an exact flat scan.
//! 2. For each held-out query, compute the exact top-k by brute-force cosine
//!    and compare it with the flat scan. This must be 1.0; anything else means
//!    the harness, not the index, is wrong. The flat scan's latency is the
//!    cost of not having an index at this size.
//! 3. For each [`AnnSettings`] under test, build the index the way production
//!    does ([`Store::create_ann_index`]), search through
//!    [`Store::vector_search`] with that setting's query parameters, and
//!    compare. The mean overlap is ANN recall@k.
//!
//! Recall is tie-tolerant: a returned row counts as a true neighbour when its
//! exact cosine is at least the k-th best exact cosine (less a float epsilon).
//! Real code has duplicate symbols with identical vectors; without tolerance
//! an index that returns the other copy of a tied pair would be marked wrong.
//!
//! Two synthetic distributions bracket real embeddings: `clustered` (a mixture
//! of tight topical clusters, closer to what an embedding model produces for
//! code) and `uniform` (isotropic noise on the sphere, the worst case for any
//! partition-based index because there is no structure to exploit). Real
//! embeddings of a real repository come from [`crate::ann_real`].

use anyhow::{bail, Context, Result};
use myceliums_storage::schema::{l2_normalize, DEFAULT_EMBEDDING_DIM};
use myceliums_storage::store::ANN_INDEX_THRESHOLD;
use myceliums_storage::{AnnIndexBuild, AnnSettings, CodeSymbol, Probes, Store, SymbolKind};
use serde::Serialize;
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::eval::bootstrap::{paired_bootstrap_default, DeltaEstimate};

/// Repository id of the scratch store.
const REPO_ID: &str = "ann-recall";

/// Cosine slack for tie-tolerant recall — float noise between brute force
/// and LanceDB's distance kernels, far below any real neighbour gap.
const TIE_EPSILON: f32 = 1e-5;

/// Queries run before timing starts, so table open and caches are warm.
const WARMUP_QUERIES: usize = 3;

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

/// A table and the queries run against it. Every vector is unit length.
#[derive(Debug, Clone)]
pub struct Vectors {
    /// Name for reports (`clustered`, `uniform`, `real:<repo>`).
    pub label: String,
    /// The stored vectors.
    pub table: Vec<Vec<f32>>,
    /// The query vectors.
    pub queries: Vec<Vec<f32>>,
}

/// Parameters of a synthetic table.
#[derive(Debug, Clone)]
pub struct Synthetic {
    /// Vectors in the table.
    pub rows: usize,
    /// Held-out queries.
    pub queries: usize,
    /// Synthetic distribution.
    pub distribution: Distribution,
    /// Cluster count for [`Distribution::Clustered`].
    pub clusters: usize,
    /// Per-dimension noise around a cluster centre, before normalisation.
    pub cluster_spread: f32,
    /// Seed of the synthetic data.
    pub seed: u64,
}

impl Synthetic {
    /// The shape the recorded measurements use, at `rows` vectors.
    pub fn standard(distribution: Distribution, rows: usize) -> Self {
        Self {
            rows,
            queries: 200,
            distribution,
            clusters: 64,
            // With 384 dimensions this puts a member at cosine ≈ 0.7 to its
            // centre and ≈ 0.5 to its cluster mates — tight topical groups,
            // loosely like embeddings of related code.
            cluster_spread: 0.05,
            seed: 92,
        }
    }

    /// Generate the table and queries. Deterministic in `seed`.
    pub fn generate(&self) -> Vectors {
        let dim = DEFAULT_EMBEDDING_DIM as usize;
        let mut rng = Rng::new(self.seed);
        let centres: Vec<Vec<f32>> = (0..self.clusters.max(1))
            .map(|_| rng.unit_vector(dim))
            .collect();
        let sample = |rng: &mut Rng| match self.distribution {
            Distribution::Uniform => rng.unit_vector(dim),
            Distribution::Clustered => {
                let centre = &centres[rng.below(centres.len())];
                let mut v: Vec<f32> = centre
                    .iter()
                    .map(|c| c + self.cluster_spread * rng.gaussian())
                    .collect();
                l2_normalize(&mut v);
                v
            }
        };
        let table = (0..self.rows).map(|_| sample(&mut rng)).collect();
        let queries = (0..self.queries).map(|_| sample(&mut rng)).collect();
        Vectors {
            label: self.distribution.label().to_string(),
            table,
            queries,
        }
    }
}

/// One measurement of the production setting on a synthetic table — what the
/// CI check and `ann-recall` without arguments run.
#[derive(Debug, Clone)]
pub struct AnnConfig {
    /// The synthetic table.
    pub data: Synthetic,
    /// Neighbours compared per query.
    pub k: usize,
    /// Index policy under test; its `threshold` must not exceed `data.rows`.
    pub settings: AnnSettings,
}

impl AnnConfig {
    /// The production setting on a table exactly at the production threshold.
    pub fn at_threshold(distribution: Distribution) -> Self {
        Self {
            data: Synthetic::standard(distribution, ANN_INDEX_THRESHOLD),
            k: 10,
            settings: AnnSettings::PRODUCTION,
        }
    }
}

/// Lowest ANN recall@10 the production setting may score on the clustered
/// table of the CI check ([`ci_check_config`]); below it `cargo test` fails
/// (issue #117). Measured: 1.0 (release build).
pub const CI_RECALL_FLOOR: f64 = 0.9;

/// The same for the uniform table — the structureless worst case, where
/// recall depends almost entirely on the share of partitions probed.
/// Measured: 0.917 with 75% probes; 50% probes scored 0.726 and the pre-#117
/// IVF-PQ setting 0.09, so this floor catches a probe or quantisation
/// regression while leaving room for untrained-seed variation.
pub const CI_UNIFORM_RECALL_FLOOR: f64 = 0.85;

/// The CI check: the production index and query setting on a synthetic table
/// (fixed seed) at [`CI_CHECK_ROWS`] rows — below the production threshold,
/// so a debug `cargo test` can afford it.
pub fn ci_check_config(distribution: Distribution) -> AnnConfig {
    AnnConfig {
        data: Synthetic {
            queries: 100,
            ..Synthetic::standard(distribution, CI_CHECK_ROWS)
        },
        k: 10,
        settings: AnnSettings {
            threshold: CI_CHECK_ROWS,
            ..AnnSettings::PRODUCTION
        },
    }
}

/// Table size of the CI check.
pub const CI_CHECK_ROWS: usize = 5_000;

/// What one measurement found.
#[derive(Debug, Clone, Serialize)]
pub struct AnnRecall {
    /// Data the table holds.
    pub data: String,
    /// Index and query parameters, human-readable.
    pub setting: String,
    pub rows: usize,
    pub queries: usize,
    pub k: usize,
    /// Mean overlap of the flat scan with brute force — the harness check.
    pub flat_recall: f64,
    /// Mean overlap of the indexed search with brute force.
    pub ann_recall: f64,
    /// 95% bootstrap interval of `ann_recall` over the queries.
    pub ann_recall_ci: (f64, f64),
    /// Share of queries whose exact nearest neighbour the ANN search ranked first.
    pub ann_top1_agreement: f64,
    /// IVF partitions the index was built with.
    pub partitions: u32,
    /// Wall time of the index build.
    pub build_ms: f64,
    /// Flat-scan latency per query (median, 95th percentile), milliseconds.
    pub flat_latency_ms: (f64, f64),
    /// Indexed-search latency per query (median, 95th percentile), milliseconds.
    pub ann_latency_ms: (f64, f64),
}

/// Run one measurement in a scratch store.
pub async fn measure(config: &AnnConfig) -> Result<AnnRecall> {
    let bench = Bench::load(config.data.generate(), config.k).await?;
    bench.measure(config.settings).await
}

/// A loaded scratch table, its exact answers and its flat-scan baseline,
/// ready to have any number of index settings measured against it.
pub struct Bench {
    _dir: tempfile::TempDir,
    path: PathBuf,
    data: Vectors,
    k: usize,
    /// Exact top-k row indices per query, best first.
    exact: Vec<Vec<usize>>,
    flat_recall: f64,
    flat_latency_ms: (f64, f64),
}

impl Bench {
    /// Store `data` with indexing disabled, compute the exact answers, and
    /// check the flat scan reproduces them.
    pub async fn load(data: Vectors, k: usize) -> Result<Self> {
        if data.table.is_empty() || data.queries.is_empty() {
            bail!("an ANN measurement needs a non-empty table and query set");
        }
        let dim = data.table[0].len();
        let dir = tempfile::tempdir()?;
        let path = dir.path().to_path_buf();
        let store = Store::open(&path, REPO_ID)
            .await?
            .with_ann_settings(AnnSettings {
                threshold: usize::MAX,
                ..AnnSettings::PRODUCTION
            });
        store.set_embedding_dim(dim as i32);
        let symbols: Vec<CodeSymbol> = (0..data.table.len()).map(synthetic_symbol).collect();
        store.store_symbols(&symbols).await?;
        let pairs = data
            .table
            .iter()
            .enumerate()
            .map(|(i, v)| (uid(i), v.clone()))
            .collect();
        store.store_embeddings(pairs).await?;

        let exact: Vec<Vec<usize>> = data
            .queries
            .iter()
            .map(|q| exact_top_k(&data.table, q, k))
            .collect();
        let mut bench = Self {
            _dir: dir,
            path,
            data,
            k,
            exact,
            flat_recall: 0.0,
            flat_latency_ms: (0.0, 0.0),
        };
        let flat = bench.run_queries(&store).await?;
        bench.flat_recall = mean(&flat.recall);
        bench.flat_latency_ms = percentiles(&flat.latency_ms);
        if bench.flat_recall < 1.0 - 1e-9 {
            bail!(
                "the flat scan disagrees with brute force (recall {:.4}) before any \
                 index exists — the harness is measuring something other than the index",
                bench.flat_recall
            );
        }
        Ok(bench)
    }

    /// Rows in the table.
    pub fn rows(&self) -> usize {
        self.data.table.len()
    }

    /// Flat-scan latency per query (median, 95th percentile), milliseconds.
    pub fn flat_latency_ms(&self) -> (f64, f64) {
        self.flat_latency_ms
    }

    /// Build the index `settings` describes over the loaded table (replacing
    /// whatever index a previous measurement left) and measure it.
    pub async fn measure(&self, settings: AnnSettings) -> Result<AnnRecall> {
        let rows = self.rows();
        if rows < settings.threshold {
            bail!(
                "{rows} rows is below the index threshold {} — nothing approximate to measure",
                settings.threshold
            );
        }
        let store = Store::open(&self.path, REPO_ID)
            .await?
            .with_ann_settings(settings);
        store.set_embedding_dim(self.data.table[0].len() as i32);
        let started = Instant::now();
        let build = store
            .create_ann_index(rows, settings.threshold)
            .await
            .with_context(|| format!("building the {} index", settings.index.label()))?;
        let build_ms = started.elapsed().as_secs_f64() * 1e3;
        let AnnIndexBuild::Built { partitions } = build else {
            bail!("expected an index build at {rows} rows, got {build:?}");
        };

        let ann = self.run_queries(&store).await?;
        let zeros = vec![0.0; ann.recall.len()];
        let DeltaEstimate {
            delta,
            ci_low,
            ci_high,
        } = paired_bootstrap_default(&ann.recall, &zeros)?;
        Ok(AnnRecall {
            data: self.data.label.clone(),
            setting: describe(&settings),
            rows,
            queries: self.data.queries.len(),
            k: self.k,
            flat_recall: self.flat_recall,
            ann_recall: delta,
            ann_recall_ci: (ci_low, ci_high),
            ann_top1_agreement: mean(&ann.top1),
            partitions,
            build_ms,
            flat_latency_ms: self.flat_latency_ms,
            ann_latency_ms: percentiles(&ann.latency_ms),
        })
    }

    /// Run every query through `store` and score it against the exact answers.
    async fn run_queries(&self, store: &Store) -> Result<Probe> {
        for query in self.data.queries.iter().take(WARMUP_QUERIES) {
            store.vector_search(query, self.k).await?;
        }
        let n = self.data.queries.len();
        let mut probe = Probe {
            recall: Vec::with_capacity(n),
            top1: Vec::with_capacity(n),
            latency_ms: Vec::with_capacity(n),
        };
        for (query, truth) in self.data.queries.iter().zip(&self.exact) {
            let started = Instant::now();
            let hits = store.vector_search(query, self.k).await?;
            probe.latency_ms.push(started.elapsed().as_secs_f64() * 1e3);
            let rows: Vec<usize> = hits
                .iter()
                .map(|(symbol, _)| row_of(&symbol.uid))
                .collect::<Result<_>>()?;
            let kth = truth
                .last()
                .map(|&i| dot(&self.data.table[i], query))
                .unwrap_or(f32::NEG_INFINITY);
            let mut counted = HashSet::new();
            let found = rows
                .iter()
                .filter(|&&row| counted.insert(row))
                .filter(|&&row| dot(&self.data.table[row], query) >= kth - TIE_EPSILON)
                .count()
                .min(self.k);
            probe.recall.push(found as f64 / self.k as f64);
            let best = truth
                .first()
                .map(|&i| dot(&self.data.table[i], query))
                .unwrap_or(f32::NEG_INFINITY);
            let first_agrees = rows
                .first()
                .is_some_and(|&row| dot(&self.data.table[row], query) >= best - TIE_EPSILON);
            probe.top1.push(if first_agrees { 1.0 } else { 0.0 });
        }
        Ok(probe)
    }
}

struct Probe {
    recall: Vec<f64>,
    top1: Vec<f64>,
    latency_ms: Vec<f64>,
}

/// Human-readable description of an index setting, e.g.
/// `IVF_PQ/16 nprobes=20 refine=10`. Unset query knobs are omitted.
pub fn describe(settings: &AnnSettings) -> String {
    let mut text = settings.index.label();
    match settings.query.nprobes {
        Some(Probes::Fixed(n)) => text.push_str(&format!(" nprobes={n}")),
        Some(Probes::Percent(p)) => text.push_str(&format!(" nprobes={p}%")),
        None => {}
    }
    if let Some(r) = settings.query.refine_factor {
        text.push_str(&format!(" refine={r}"));
    }
    if let Some(ef) = settings.query.ef {
        text.push_str(&format!(" ef={ef}"));
    }
    text
}

/// The settings the `--sweep` run compares (issue #117): the old production
/// setting, then one knob at a time — refine factor, probes, PQ sub-vectors,
/// other index types — and the production setting. Thresholds are zero: the
/// sweep decides the size, not the policy.
pub fn sweep_settings() -> Vec<AnnSettings> {
    use myceliums_storage::{AnnIndexKind, AnnQuery};
    let pq = |sub_vectors| AnnIndexKind::IvfPq { sub_vectors };
    let query = |nprobes: Option<usize>, refine_factor: Option<u32>, ef: Option<usize>| AnnQuery {
        nprobes: nprobes.map(Probes::Fixed),
        refine_factor,
        ef,
    };
    let share = |percent: u32, refine_factor: Option<u32>, ef: Option<usize>| AnnQuery {
        nprobes: Some(Probes::Percent(percent)),
        refine_factor,
        ef,
    };
    let mut settings = vec![
        // Before #117: IVF_PQ, 16 sub-vectors, LanceDB defaults.
        (pq(16), query(None, None, None)),
        (pq(16), query(None, Some(5), None)),
        (pq(16), query(None, Some(10), None)),
        (pq(16), query(None, Some(20), None)),
        (pq(16), query(Some(10), None, None)),
        (pq(16), query(Some(50), None, None)),
        (pq(16), query(Some(50), Some(10), None)),
        (pq(32), query(None, None, None)),
        (pq(48), query(None, None, None)),
        (pq(96), query(None, None, None)),
        (pq(48), query(None, Some(10), None)),
        (pq(96), query(None, Some(10), None)),
        (AnnIndexKind::IvfFlat, query(None, None, None)),
        (AnnIndexKind::IvfFlat, query(Some(50), None, None)),
        (AnnIndexKind::IvfHnswSq, query(None, None, None)),
        (AnnIndexKind::IvfHnswSq, query(None, None, Some(100))),
        (AnnIndexKind::IvfHnswSq, query(None, None, Some(200))),
        (AnnIndexKind::IvfHnswSq, query(Some(50), None, Some(100))),
        // Probing a share of the partitions rather than a fixed count, added
        // after real embeddings showed partition coverage, not quantisation,
        // limits recall.
        (pq(16), share(50, Some(20), None)),
        (AnnIndexKind::IvfFlat, share(50, None, None)),
        (AnnIndexKind::IvfFlat, share(75, None, None)),
        (AnnIndexKind::IvfHnswSq, share(50, None, Some(100))),
        (AnnIndexKind::IvfHnswSq, share(75, None, Some(100))),
        (AnnIndexKind::IvfHnswSq, share(75, None, Some(200))),
    ];
    let production = (AnnSettings::PRODUCTION.index, AnnSettings::PRODUCTION.query);
    if !settings.contains(&production) {
        settings.push(production);
    }
    settings
        .into_iter()
        .map(|(index, query)| AnnSettings {
            threshold: 0,
            index,
            query,
        })
        .collect()
}

/// Median and 95th percentile (nearest rank) of `values`.
pub fn percentiles(values: &[f64]) -> (f64, f64) {
    if values.is_empty() {
        return (0.0, 0.0);
    }
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let rank = |p: f64| {
        let index = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len()) - 1;
        sorted[index]
    };
    (rank(0.50), rank(0.95))
}

/// Exact top-k row indices by brute-force cosine, best first. Vectors are unit
/// length, so the dot product is the cosine. Ties break by row index.
fn exact_top_k(table: &[Vec<f32>], query: &[f32], k: usize) -> Vec<usize> {
    let mut scored: Vec<(usize, f32)> = table
        .iter()
        .enumerate()
        .map(|(i, v)| (i, dot(v, query)))
        .collect();
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    scored.into_iter().take(k).map(|(i, _)| i).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn mean(values: &[f64]) -> f64 {
    crate::eval::metrics::mean(values)
}

fn uid(i: usize) -> String {
    format!("v{i:07}")
}

fn row_of(uid: &str) -> Result<usize> {
    uid.strip_prefix('v')
        .and_then(|digits| digits.parse().ok())
        .with_context(|| format!("the store returned a row the harness never wrote: {uid}"))
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
        repo_id: REPO_ID.to_string(),
        metadata: None,
    }
}

/// SplitMix64 with Box–Muller Gaussians — deterministic synthetic data.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
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

    pub(crate) fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// Fisher–Yates shuffle.
    pub(crate) fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            items.swap(i, self.below(i + 1));
        }
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
    /// and recall must land in (0, 1].
    #[tokio::test]
    async fn the_harness_checks_itself_on_a_small_table() {
        let config = AnnConfig {
            data: Synthetic {
                rows: 600,
                queries: 20,
                ..Synthetic::standard(Distribution::Clustered, 600)
            },
            k: 10,
            settings: AnnSettings {
                threshold: 500,
                ..AnnSettings::PRODUCTION
            },
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
        assert!(result.ann_latency_ms.0 <= result.ann_latency_ms.1);
    }

    /// Issue #117 CI check: the production index setting must keep ANN
    /// recall@10 against exact search at or above the floors on the
    /// deterministic synthetic tables. Runs on every `cargo test`.
    #[tokio::test]
    async fn the_production_index_keeps_recall_above_the_floor() {
        for (distribution, floor) in [
            (Distribution::Clustered, CI_RECALL_FLOOR),
            (Distribution::Uniform, CI_UNIFORM_RECALL_FLOOR),
        ] {
            let result = measure(&ci_check_config(distribution)).await.unwrap();
            assert_eq!(result.flat_recall, 1.0);
            assert!(
                result.ann_recall >= floor,
                "{} table: ANN recall@10 {:.4} (95% CI {:.4}–{:.4}) is below the CI floor \
                 {floor} for {} — see benchmarks/METHODOLOGY.md, ANN recall",
                distribution.label(),
                result.ann_recall,
                result.ann_recall_ci.0,
                result.ann_recall_ci.1,
                result.setting
            );
        }
    }

    #[tokio::test]
    async fn a_table_below_the_threshold_is_refused() {
        let config = AnnConfig {
            data: Synthetic::standard(Distribution::Uniform, 100),
            k: 10,
            settings: AnnSettings {
                threshold: 500,
                ..AnnSettings::PRODUCTION
            },
        };
        assert!(measure(&config).await.is_err());
    }

    #[test]
    fn percentiles_use_nearest_rank() {
        let values: Vec<f64> = (1..=100).map(f64::from).collect();
        assert_eq!(percentiles(&values), (50.0, 95.0));
        assert_eq!(percentiles(&[7.0]), (7.0, 7.0));
        assert_eq!(percentiles(&[]), (0.0, 0.0));
    }

    #[test]
    fn synthetic_data_is_deterministic_and_unit_length() {
        let a = Synthetic::standard(Distribution::Clustered, 50).generate();
        let b = Synthetic::standard(Distribution::Clustered, 50).generate();
        assert_eq!(a.table, b.table);
        assert_eq!(a.queries, b.queries);
        for v in a.table.iter().chain(&a.queries) {
            assert!((dot(v, v) - 1.0).abs() < 1e-4);
        }
    }
}
