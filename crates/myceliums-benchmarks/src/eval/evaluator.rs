//! Evaluating a search mode against the golden set.
//!
//! The evaluator is the one place that turns "a search engine" into "a number":
//! it runs every golden query through a [`SearchMode`], projects the results
//! onto stable symbol references, and scores them with the metrics module.

use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::corpus::{symbol_ref, Corpus};
use super::golden_set::{GoldenQuery, GoldenSet, QueryIntent, SymbolRef};
use super::metrics::{
    hit_at_k, mean, ndcg_at_k, r_precision, recall_at_k, reciprocal_rank, RelevantSet,
};
use super::models::ModelIndex;

/// The cutoffs recall and hit rate are reported at.
pub const RECALL_CUTOFFS: [usize; 3] = [1, 5, 10];

/// Reciprocal rank counts a first relevant hit only within this depth, so the
/// reported MRR is MRR@10 — what an agent reading one page of results sees.
pub const MRR_CUTOFF: usize = 10;

/// Depth of the nDCG cutoff.
pub const NDCG_CUTOFF: usize = 10;

/// How many results every mode is asked for.
///
/// Deep enough for the deepest cutoff above *and* for R-precision, which reads
/// the top `|relevant|` results — the golden set has a query with 13 labelled
/// answers. 20 is also the result count `myc search --hybrid` and the MCP
/// `hybrid_search` tool return by default. [`evaluate`] refuses a golden set
/// whose largest label set would not fit.
pub const RANKING_DEPTH: usize = 20;

/// One reported retrieval metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Metric {
    /// Fraction of labelled answers in the top 1.
    RecallAt1,
    /// Fraction of labelled answers in the top 5.
    RecallAt5,
    /// Fraction of labelled answers in the top 10.
    RecallAt10,
    /// Any labelled answer at rank 1.
    HitAt1,
    /// Any labelled answer in the top 5.
    HitAt5,
    /// Any labelled answer in the top 10.
    HitAt10,
    /// Reciprocal rank of the first answer within [`MRR_CUTOFF`].
    Mrr,
    /// nDCG at [`NDCG_CUTOFF`], binary relevance.
    NdcgAt10,
    /// Precision at rank `|relevant|`.
    RPrecision,
}

impl Metric {
    /// Every metric, in report order.
    pub const ALL: [Metric; 9] = [
        Metric::RecallAt1,
        Metric::RecallAt5,
        Metric::RecallAt10,
        Metric::HitAt1,
        Metric::HitAt5,
        Metric::HitAt10,
        Metric::Mrr,
        Metric::NdcgAt10,
        Metric::RPrecision,
    ];

    /// Stable key used in the report and baseline JSON.
    pub fn id(&self) -> &'static str {
        match self {
            Metric::RecallAt1 => "recall_at_1",
            Metric::RecallAt5 => "recall_at_5",
            Metric::RecallAt10 => "recall_at_10",
            Metric::HitAt1 => "hit_at_1",
            Metric::HitAt5 => "hit_at_5",
            Metric::HitAt10 => "hit_at_10",
            Metric::Mrr => "mrr",
            Metric::NdcgAt10 => "ndcg_at_10",
            Metric::RPrecision => "r_precision",
        }
    }

    /// Short human label for tables.
    pub fn label(&self) -> &'static str {
        match self {
            Metric::RecallAt1 => "recall@1",
            Metric::RecallAt5 => "recall@5",
            Metric::RecallAt10 => "recall@10",
            Metric::HitAt1 => "hit@1",
            Metric::HitAt5 => "hit@5",
            Metric::HitAt10 => "hit@10",
            Metric::Mrr => "MRR@10",
            Metric::NdcgAt10 => "nDCG@10",
            Metric::RPrecision => "R-prec",
        }
    }

    /// Score one ranking against one relevant set.
    pub fn score(&self, ranking: &[SymbolRef], relevant: &RelevantSet) -> f64 {
        match self {
            Metric::RecallAt1 => recall_at_k(ranking, relevant, 1),
            Metric::RecallAt5 => recall_at_k(ranking, relevant, 5),
            Metric::RecallAt10 => recall_at_k(ranking, relevant, 10),
            Metric::HitAt1 => hit_at_k(ranking, relevant, 1),
            Metric::HitAt5 => hit_at_k(ranking, relevant, 5),
            Metric::HitAt10 => hit_at_k(ranking, relevant, 10),
            Metric::Mrr => {
                let depth = ranking.len().min(MRR_CUTOFF);
                reciprocal_rank(&ranking[..depth], relevant)
            }
            Metric::NdcgAt10 => ndcg_at_k(ranking, relevant, NDCG_CUTOFF),
            Metric::RPrecision => r_precision(ranking, relevant),
        }
    }
}

/// One value per [`Metric`], serialised with the metric ids as keys.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct MetricValues {
    pub recall_at_1: f64,
    pub recall_at_5: f64,
    pub recall_at_10: f64,
    pub hit_at_1: f64,
    pub hit_at_5: f64,
    pub hit_at_10: f64,
    pub mrr: f64,
    pub ndcg_at_10: f64,
    pub r_precision: f64,
}

impl MetricValues {
    /// The value of one metric.
    pub fn get(&self, metric: Metric) -> f64 {
        match metric {
            Metric::RecallAt1 => self.recall_at_1,
            Metric::RecallAt5 => self.recall_at_5,
            Metric::RecallAt10 => self.recall_at_10,
            Metric::HitAt1 => self.hit_at_1,
            Metric::HitAt5 => self.hit_at_5,
            Metric::HitAt10 => self.hit_at_10,
            Metric::Mrr => self.mrr,
            Metric::NdcgAt10 => self.ndcg_at_10,
            Metric::RPrecision => self.r_precision,
        }
    }

    /// Build a value set by computing each metric.
    pub fn from_fn(mut value: impl FnMut(Metric) -> f64) -> Self {
        Self {
            recall_at_1: value(Metric::RecallAt1),
            recall_at_5: value(Metric::RecallAt5),
            recall_at_10: value(Metric::RecallAt10),
            hit_at_1: value(Metric::HitAt1),
            hit_at_5: value(Metric::HitAt5),
            hit_at_10: value(Metric::HitAt10),
            mrr: value(Metric::Mrr),
            ndcg_at_10: value(Metric::NdcgAt10),
            r_precision: value(Metric::RPrecision),
        }
    }

    /// Per-metric unweighted mean over `rows` — every query counts equally.
    pub fn mean_of<'a>(rows: impl IntoIterator<Item = &'a MetricValues> + Clone) -> Self {
        Self::from_fn(|metric| {
            let values: Vec<f64> = rows.clone().into_iter().map(|r| r.get(metric)).collect();
            mean(&values)
        })
    }

    /// Every value rounded with `round`.
    pub fn map(&self, round: impl Fn(f64) -> f64) -> Self {
        Self::from_fn(|metric| round(self.get(metric)))
    }
}

/// A retrieval strategy under measurement.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SearchMode {
    /// BM25 keyword search over symbol name, signature, content and metadata.
    Lexical,
    /// Vector similarity over persisted embeddings.
    Semantic,
    /// Reciprocal-rank fusion of the lexical and semantic rankings.
    Hybrid,
    /// Hybrid fusion followed by cross-encoder reranking.
    HybridRerank,
}

impl SearchMode {
    /// Every mode, in report order.
    pub fn all() -> [SearchMode; 4] {
        [
            SearchMode::Lexical,
            SearchMode::Semantic,
            SearchMode::Hybrid,
            SearchMode::HybridRerank,
        ]
    }

    /// Stable identifier used in reports and the baseline file.
    pub fn id(&self) -> &'static str {
        match self {
            SearchMode::Lexical => "lexical",
            SearchMode::Semantic => "semantic",
            SearchMode::Hybrid => "hybrid",
            SearchMode::HybridRerank => "hybrid+rerank",
        }
    }

    /// Inverse of [`id`](Self::id).
    pub fn from_id(id: &str) -> Option<SearchMode> {
        SearchMode::all().into_iter().find(|mode| mode.id() == id)
    }

    /// True when ranking needs the embedding model.
    pub fn needs_embeddings(&self) -> bool {
        !matches!(self, SearchMode::Lexical)
    }

    /// True when ranking needs the cross-encoder reranker.
    pub fn needs_reranker(&self) -> bool {
        matches!(self, SearchMode::HybridRerank)
    }
}

/// What the modes search: the parsed corpus, plus the embedding index when the
/// model-backed modes are available.
pub struct Retrieval<'a> {
    corpus: &'a Corpus,
    models: Option<&'a ModelIndex>,
}

impl<'a> Retrieval<'a> {
    /// Lexical search only — no model is loaded or downloaded.
    pub fn lexical_only(corpus: &'a Corpus) -> Self {
        Self {
            corpus,
            models: None,
        }
    }

    /// Every mode, with the model-backed ones served by `models`.
    pub fn with_models(corpus: &'a Corpus, models: &'a ModelIndex) -> Self {
        Self {
            corpus,
            models: Some(models),
        }
    }

    /// Rank corpus symbols for `query` under `mode`, best first.
    ///
    /// Errors for a model-backed mode when no embedding index is available,
    /// rather than returning an empty ranking. An empty ranking scores zero on
    /// every metric, which is indistinguishable from a search engine that found
    /// nothing — so the one thing this must never do is stay quiet.
    pub async fn rank(&self, mode: SearchMode, query: &str) -> Result<Vec<SymbolRef>> {
        let symbols = match mode {
            SearchMode::Lexical => myceliums_core::search_symbols(self.corpus.symbols(), query)
                .into_iter()
                .take(RANKING_DEPTH)
                .map(|hit| hit.symbol)
                .collect(),
            _ => {
                let Some(models) = self.models else {
                    bail!(
                        "mode '{}' was asked to rank '{query}' but no embedding index was \
                         built for this run — it needs model weights",
                        mode.id()
                    );
                };
                models
                    .rank(mode, self.corpus.symbols(), query, RANKING_DEPTH)
                    .await?
            }
        };
        Ok(symbols.iter().map(symbol_ref).collect())
    }
}

/// Scores for a single query under a single mode.
#[derive(Debug, Clone)]
pub struct QueryScore {
    /// Golden query identifier.
    pub id: String,
    /// What the query probes.
    pub intent: QueryIntent,
    /// Every metric for this query.
    pub values: MetricValues,
}

/// Mean scores over the queries of one intent.
#[derive(Debug, Clone)]
pub struct IntentScore {
    /// Queries of this intent.
    pub queries: usize,
    /// Mean of each metric over them.
    pub mean: MetricValues,
}

/// Aggregate quality of one search mode over the whole golden set.
#[derive(Debug, Clone)]
pub struct ModeScore {
    /// Which mode was measured.
    pub mode: SearchMode,
    /// Mean of each metric across all queries.
    pub mean: MetricValues,
    /// Means restricted to each intent present in the golden set.
    pub by_intent: BTreeMap<QueryIntent, IntentScore>,
    /// Per-query detail, in golden-set order.
    pub queries: Vec<QueryScore>,
}

impl ModeScore {
    /// Mean recall at `k`, or `0.0` if the cutoff is not reported.
    pub fn recall_at(&self, k: usize) -> f64 {
        match k {
            1 => self.mean.recall_at_1,
            5 => self.mean.recall_at_5,
            10 => self.mean.recall_at_10,
            _ => 0.0,
        }
    }

    /// Mean reciprocal rank restricted to one intent category.
    pub fn mrr_for_intent(&self, intent: QueryIntent) -> f64 {
        self.by_intent
            .get(&intent)
            .map(|score| score.mean.mrr)
            .unwrap_or(0.0)
    }

    /// Per-query values of `metric`, in golden-set order — the input the paired
    /// bootstrap pairs on.
    pub fn per_query(&self, metric: Metric) -> Vec<f64> {
        self.queries.iter().map(|q| q.values.get(metric)).collect()
    }
}

/// Score every golden query for one mode.
///
/// Deterministic for the lexical mode by construction, and for the
/// model-backed modes up to the floating-point reproducibility of the model
/// runtime on one machine (see `benchmarks/METHODOLOGY.md`).
pub async fn evaluate(
    mode: SearchMode,
    retrieval: &Retrieval<'_>,
    set: &GoldenSet,
) -> Result<ModeScore> {
    if let Some(deepest) = set.queries.iter().map(|q| q.relevant.len()).max() {
        if deepest > RANKING_DEPTH {
            bail!(
                "a golden query labels {deepest} answers but modes are only asked for \
                 {RANKING_DEPTH} results, so R-precision would be truncated — raise \
                 RANKING_DEPTH"
            );
        }
    }

    let mut queries = Vec::with_capacity(set.len());
    for query in &set.queries {
        queries.push(score_query(mode, retrieval, query).await?);
    }

    let mean = MetricValues::mean_of(queries.iter().map(|q| &q.values));
    let mut by_intent = BTreeMap::new();
    for intent in QueryIntent::ALL {
        let rows: Vec<&MetricValues> = queries
            .iter()
            .filter(|q| q.intent == intent)
            .map(|q| &q.values)
            .collect();
        if !rows.is_empty() {
            by_intent.insert(
                intent,
                IntentScore {
                    queries: rows.len(),
                    mean: MetricValues::mean_of(rows.iter().copied()),
                },
            );
        }
    }

    Ok(ModeScore {
        mode,
        mean,
        by_intent,
        queries,
    })
}

async fn score_query(
    mode: SearchMode,
    retrieval: &Retrieval<'_>,
    query: &GoldenQuery,
) -> Result<QueryScore> {
    let ranking = retrieval.rank(mode, &query.query).await?;
    let relevant: RelevantSet = query.relevant.iter().cloned().collect();
    Ok(QueryScore {
        id: query.id.clone(),
        intent: query.intent,
        values: MetricValues::from_fn(|metric| metric.score(&ranking, &relevant)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::corpus::fixtures_root;

    async fn scored() -> ModeScore {
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let set = GoldenSet::embedded().unwrap();
        evaluate(SearchMode::Lexical, &Retrieval::lexical_only(&corpus), &set)
            .await
            .unwrap()
    }

    #[test]
    fn only_lexical_runs_without_a_model() {
        assert!(!SearchMode::Lexical.needs_embeddings());
        for mode in [
            SearchMode::Semantic,
            SearchMode::Hybrid,
            SearchMode::HybridRerank,
        ] {
            assert!(mode.needs_embeddings(), "{}", mode.id());
        }
        assert!(SearchMode::HybridRerank.needs_reranker());
        assert!(!SearchMode::Hybrid.needs_reranker());
    }

    #[test]
    fn mode_ids_round_trip() {
        for mode in SearchMode::all() {
            assert_eq!(SearchMode::from_id(mode.id()), Some(mode));
        }
        assert_eq!(SearchMode::from_id("bm25"), None);
    }

    #[test]
    fn metric_ids_match_the_serialised_field_names() {
        // The gate looks metrics up by id in the baseline JSON; a field renamed
        // without its id would make every comparison read a missing value.
        let values = MetricValues::from_fn(|metric| {
            Metric::ALL.iter().position(|m| *m == metric).unwrap() as f64
        });
        let json = serde_json::to_value(values).unwrap();
        for metric in Metric::ALL {
            assert_eq!(
                json[metric.id()].as_f64(),
                Some(values.get(metric)),
                "{}",
                metric.id()
            );
        }
        assert_eq!(json.as_object().unwrap().len(), Metric::ALL.len());
    }

    #[test]
    fn mrr_ignores_hits_past_its_cutoff() {
        let filler: Vec<SymbolRef> = (0..MRR_CUTOFF)
            .map(|i| SymbolRef::new("f.py", format!("noise{i}")))
            .collect();
        let mut ranking = filler;
        ranking.push(SymbolRef::new("f.py", "answer"));
        let relevant: RelevantSet = [SymbolRef::new("f.py", "answer")].into_iter().collect();
        assert_eq!(Metric::Mrr.score(&ranking, &relevant), 0.0);
        assert!(Metric::RPrecision.score(&ranking, &relevant) == 0.0);
    }

    #[test]
    fn the_ranking_is_deep_enough_for_every_metric() {
        // R-precision reads |relevant| results; the deepest fixed cutoff is 10.
        let set = GoldenSet::embedded().unwrap();
        let deepest_labels = set.queries.iter().map(|q| q.relevant.len()).max().unwrap();
        assert!(RANKING_DEPTH >= deepest_labels);
        assert!(RANKING_DEPTH >= *RECALL_CUTOFFS.iter().max().unwrap());
        assert!(RANKING_DEPTH >= MRR_CUTOFF.max(NDCG_CUTOFF));
    }

    #[tokio::test]
    async fn model_backed_modes_refuse_to_rank_without_an_index() {
        // An empty ranking would report a plausible-looking 0.0 instead of
        // failing, which is the one outcome this harness must not produce.
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let retrieval = Retrieval::lexical_only(&corpus);
        let set = GoldenSet::embedded().unwrap();
        for mode in [
            SearchMode::Semantic,
            SearchMode::Hybrid,
            SearchMode::HybridRerank,
        ] {
            let error = retrieval
                .rank(mode, "anything")
                .await
                .expect_err("a model-backed mode must not rank without its index");
            assert!(error.to_string().contains(mode.id()), "{error}");
            assert!(evaluate(mode, &retrieval, &set).await.is_err());
        }
    }

    #[tokio::test]
    async fn evaluation_is_deterministic() {
        let first = scored().await;
        let second = scored().await;
        assert_eq!(first.mean, second.mean);
        for (a, b) in first.queries.iter().zip(&second.queries) {
            assert_eq!(a.values, b.values, "{}", a.id);
        }
    }

    #[tokio::test]
    async fn metrics_stay_within_bounds() {
        let score = scored().await;
        for query in &score.queries {
            for metric in Metric::ALL {
                let value = query.values.get(metric);
                assert!(
                    (0.0..=1.0).contains(&value),
                    "{} {}: {value}",
                    query.id,
                    metric.id()
                );
            }
        }
    }

    #[tokio::test]
    async fn cutoff_metrics_are_monotonic_in_k() {
        let score = scored().await;
        for query in &score.queries {
            let v = &query.values;
            assert!(v.recall_at_1 <= v.recall_at_5 && v.recall_at_5 <= v.recall_at_10);
            assert!(v.hit_at_1 <= v.hit_at_5 && v.hit_at_5 <= v.hit_at_10);
            // A hit is at least as easy as full recall, and MRR@10 is zero
            // exactly when nothing relevant is in the top 10.
            assert!(v.recall_at_10 <= v.hit_at_10);
            assert_eq!(v.mrr == 0.0, v.hit_at_10 == 0.0, "{}", query.id);
        }
    }

    #[tokio::test]
    async fn intent_breakdown_partitions_the_queries() {
        let score = scored().await;
        let counted: usize = score.by_intent.values().map(|s| s.queries).sum();
        assert_eq!(counted, score.queries.len());
        // Unweighted means: the overall mean is the query-weighted mean of the
        // per-intent means.
        let weighted: f64 = score
            .by_intent
            .values()
            .map(|s| s.mean.mrr * s.queries as f64)
            .sum::<f64>()
            / counted as f64;
        assert!((weighted - score.mean.mrr).abs() < 1e-12);
    }

    #[tokio::test]
    async fn bm25_finds_exactly_named_symbols() {
        let score = scored().await;
        // Exact-name lookup is the easiest case; if this regresses, tokenization
        // or corpus loading is broken rather than relevance being subtly worse.
        // The floor is set well below the recorded 0.93 so ordinary ranking
        // churn does not trip it: 0.5 means the average exact-name query still
        // puts a correct answer in the top two, which no working tokenizer
        // fails to do.
        const EXACT_NAME_MRR_FLOOR: f64 = 0.5;
        assert!(
            score.mrr_for_intent(QueryIntent::ExactName) > EXACT_NAME_MRR_FLOOR,
            "exact-name MRR unexpectedly low: {}",
            score.mrr_for_intent(QueryIntent::ExactName)
        );
    }

    #[tokio::test]
    async fn every_query_is_scored() {
        let set = GoldenSet::embedded().unwrap();
        assert_eq!(scored().await.queries.len(), set.len());
    }
}
