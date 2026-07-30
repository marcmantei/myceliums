//! Evaluating a search mode against the golden set.
//!
//! The evaluator is the one place that turns "a search engine" into "a number":
//! it runs every golden query through a [`SearchMode`], projects the results
//! onto stable symbol references, and scores them with the metrics module.

use anyhow::Result;
use std::collections::BTreeMap;

use super::corpus::{symbol_ref, Corpus};
use super::golden_set::{GoldenQuery, GoldenSet, QueryIntent, SymbolRef};
use super::metrics::{mean, recall_at_k, reciprocal_rank, RelevantSet};

/// The cutoffs recall is reported at, the cutoffs issue #34 asks for.
pub const RECALL_CUTOFFS: [usize; 3] = [1, 5, 10];

/// How many results a mode is asked for. Ten is the largest reported cutoff.
const RESULT_LIMIT: usize = 10;

/// A retrieval strategy under measurement.
///
/// Only [`SearchMode::Lexical`] can be scored offline: the semantic leg needs
/// the fastembed model weights, which are downloaded at first use and would
/// make the benchmark neither offline nor deterministic. The remaining modes
/// are named here — rather than omitted — so the report can state plainly that
/// they are *unmeasured*, in the same spirit as the `is_verified` flag in
/// `metrics.rs`. Fabricating numbers for them would repeat the mistake #22 fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
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
    /// Every mode named by issue #34, measured or not.
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

    /// Why a mode cannot be scored offline, or `None` when it can.
    pub fn offline_blocker(&self) -> Option<&'static str> {
        match self {
            SearchMode::Lexical => None,
            SearchMode::Semantic => Some("requires fastembed model weights (network download)"),
            SearchMode::Hybrid => Some("requires the semantic leg's model weights"),
            SearchMode::HybridRerank => {
                Some("requires the semantic leg plus cross-encoder reranker weights")
            }
        }
    }

    /// True when this mode is scored by the offline benchmark.
    pub fn is_measurable_offline(&self) -> bool {
        self.offline_blocker().is_none()
    }

    /// Rank corpus symbols for `query`, best first.
    fn rank(&self, corpus: &Corpus, query: &str) -> Vec<SymbolRef> {
        match self {
            SearchMode::Lexical => myceliums_core::search_symbols(corpus.symbols(), query)
                .iter()
                .take(RESULT_LIMIT)
                .map(|hit| symbol_ref(&hit.symbol))
                .collect(),
            // Unmeasurable offline; never invoked by `evaluate`.
            SearchMode::Semantic | SearchMode::Hybrid | SearchMode::HybridRerank => Vec::new(),
        }
    }
}

/// Scores for a single query under a single mode.
#[derive(Debug, Clone)]
pub struct QueryScore {
    /// Golden query identifier.
    pub id: String,
    /// What the query probes.
    pub intent: QueryIntent,
    /// recall@k, keyed by cutoff.
    pub recall: BTreeMap<usize, f64>,
    /// Reciprocal rank of the first relevant hit.
    pub reciprocal_rank: f64,
}

/// Aggregate quality of one search mode over the whole golden set.
#[derive(Debug, Clone)]
pub struct ModeScore {
    /// Which mode was measured.
    pub mode: SearchMode,
    /// Mean recall@k across queries, keyed by cutoff.
    pub recall: BTreeMap<usize, f64>,
    /// Mean reciprocal rank across queries.
    pub mrr: f64,
    /// Per-query detail, in golden-set order.
    pub queries: Vec<QueryScore>,
}

impl ModeScore {
    /// Mean recall at `k`, or `0.0` if the cutoff was not measured.
    pub fn recall_at(&self, k: usize) -> f64 {
        self.recall.get(&k).copied().unwrap_or(0.0)
    }

    /// Mean reciprocal rank restricted to one intent category.
    pub fn mrr_for_intent(&self, intent: QueryIntent) -> f64 {
        let values: Vec<f64> = self
            .queries
            .iter()
            .filter(|q| q.intent == intent)
            .map(|q| q.reciprocal_rank)
            .collect();
        mean(&values)
    }
}

/// Score every golden query for one mode.
///
/// Deterministic: BM25 scoring, corpus order and golden-set order are all
/// fixed, so repeated runs return identical numbers.
pub fn evaluate(mode: SearchMode, corpus: &Corpus, set: &GoldenSet) -> Result<ModeScore> {
    if let Some(blocker) = mode.offline_blocker() {
        anyhow::bail!(
            "mode '{}' cannot be measured offline: {}",
            mode.id(),
            blocker
        );
    }

    let queries: Vec<QueryScore> = set
        .queries
        .iter()
        .map(|query| score_query(mode, corpus, query))
        .collect();

    let recall = RECALL_CUTOFFS
        .iter()
        .map(|&k| {
            let per_query: Vec<f64> = queries.iter().map(|q| q.recall[&k]).collect();
            (k, mean(&per_query))
        })
        .collect();

    let reciprocal_ranks: Vec<f64> = queries.iter().map(|q| q.reciprocal_rank).collect();

    Ok(ModeScore {
        mode,
        recall,
        mrr: mean(&reciprocal_ranks),
        queries,
    })
}

fn score_query(mode: SearchMode, corpus: &Corpus, query: &GoldenQuery) -> QueryScore {
    let ranking = mode.rank(corpus, &query.query);
    let relevant: RelevantSet = query.relevant.iter().cloned().collect();

    QueryScore {
        id: query.id.clone(),
        intent: query.intent,
        recall: RECALL_CUTOFFS
            .iter()
            .map(|&k| (k, recall_at_k(&ranking, &relevant, k)))
            .collect(),
        reciprocal_rank: reciprocal_rank(&ranking, &relevant),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::corpus::fixtures_root;

    fn scored() -> ModeScore {
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let set = GoldenSet::embedded().unwrap();
        evaluate(SearchMode::Lexical, &corpus, &set).unwrap()
    }

    #[test]
    fn lexical_mode_is_measurable_offline() {
        assert!(SearchMode::Lexical.is_measurable_offline());
        assert!(!SearchMode::Semantic.is_measurable_offline());
        assert!(!SearchMode::Hybrid.is_measurable_offline());
        assert!(!SearchMode::HybridRerank.is_measurable_offline());
    }

    #[test]
    fn unmeasurable_modes_refuse_to_produce_numbers() {
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let set = GoldenSet::embedded().unwrap();
        assert!(evaluate(SearchMode::Semantic, &corpus, &set).is_err());
    }

    #[test]
    fn evaluation_is_deterministic() {
        let first = scored();
        let second = scored();
        assert_eq!(first.mrr, second.mrr);
        for k in RECALL_CUTOFFS {
            assert_eq!(first.recall_at(k), second.recall_at(k));
        }
    }

    #[test]
    fn metrics_stay_within_bounds() {
        let score = scored();
        assert!((0.0..=1.0).contains(&score.mrr));
        for k in RECALL_CUTOFFS {
            assert!((0.0..=1.0).contains(&score.recall_at(k)));
        }
    }

    #[test]
    fn recall_is_monotonic_in_k() {
        let score = scored();
        assert!(score.recall_at(1) <= score.recall_at(5));
        assert!(score.recall_at(5) <= score.recall_at(10));
    }

    #[test]
    fn bm25_finds_exactly_named_symbols() {
        let score = scored();
        // Exact-name lookup is the easiest case; if this regresses, tokenization
        // or corpus loading is broken rather than relevance being subtly worse.
        assert!(
            score.mrr_for_intent(QueryIntent::ExactName) > 0.5,
            "exact-name MRR unexpectedly low: {}",
            score.mrr_for_intent(QueryIntent::ExactName)
        );
    }

    #[test]
    fn every_query_is_scored() {
        let set = GoldenSet::embedded().unwrap();
        assert_eq!(scored().queries.len(), set.len());
    }
}
