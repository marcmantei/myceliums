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

/// How many results a mode is asked for: the deepest cutoff the report names.
///
/// Derived rather than written down, so adding a cutoff to [`RECALL_CUTOFFS`]
/// widens the request automatically. A result ranked past this point cannot
/// change any reported number, so fetching more would be wasted work.
const DEEPEST_REPORTED_CUTOFF: usize = deepest_cutoff();

/// Largest value in [`RECALL_CUTOFFS`], evaluated at compile time.
const fn deepest_cutoff() -> usize {
    let mut deepest = 0;
    let mut index = 0;
    while index < RECALL_CUTOFFS.len() {
        if RECALL_CUTOFFS[index] > deepest {
            deepest = RECALL_CUTOFFS[index];
        }
        index += 1;
    }
    deepest
}

/// Why a mode cannot be scored by this harness, and what would change that.
///
/// Every blocker recorded here is *environmental*: the mode's code works, but
/// the artefacts it needs are absent from an offline run. `lifted_by` names the
/// concrete change that makes the mode measurable, so `UNAVAILABLE` is never
/// read as "this mode is broken" or "this mode can never be scored".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfflineBlocker {
    /// What is missing, in one line.
    pub reason: &'static str,
    /// The change that would let this mode be measured.
    pub lifted_by: &'static str,
}

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

    /// Why a mode cannot be scored in this environment, or `None` when it can.
    pub fn offline_blocker(&self) -> Option<OfflineBlocker> {
        match self {
            SearchMode::Lexical => None,
            SearchMode::Semantic => Some(OfflineBlocker {
                reason: "requires fastembed model weights (network download)",
                lifted_by: "run where the fastembed weights are already cached",
            }),
            SearchMode::Hybrid => Some(OfflineBlocker {
                reason: "requires the semantic leg's model weights",
                lifted_by: "run where the fastembed weights are already cached",
            }),
            SearchMode::HybridRerank => Some(OfflineBlocker {
                reason: "requires the semantic leg plus cross-encoder reranker weights",
                lifted_by: "run where the fastembed and reranker weights are already cached",
            }),
        }
    }

    /// True when this mode is scored by the offline benchmark.
    pub fn is_measurable_offline(&self) -> bool {
        self.offline_blocker().is_none()
    }

    /// Rank corpus symbols for `query`, best first.
    ///
    /// Errors for a mode with an [`offline_blocker`](Self::offline_blocker)
    /// rather than returning an empty ranking. An empty ranking scores zero on
    /// every metric, which is indistinguishable from a search engine that found
    /// nothing — so the one thing this must never do is stay quiet.
    fn rank(&self, corpus: &Corpus, query: &str) -> Result<Vec<SymbolRef>> {
        match self {
            SearchMode::Lexical => Ok(myceliums_core::search_symbols(corpus.symbols(), query)
                .iter()
                .take(DEEPEST_REPORTED_CUTOFF)
                .map(|hit| symbol_ref(&hit.symbol))
                .collect()),
            SearchMode::Semantic | SearchMode::Hybrid | SearchMode::HybridRerank => {
                anyhow::bail!(
                    "mode '{}' was asked to rank '{}' but cannot run offline: {}",
                    self.id(),
                    query,
                    self.offline_blocker()
                        .map(|blocker| blocker.reason)
                        .unwrap_or("no blocker recorded"),
                )
            }
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
            "mode '{}' cannot be measured offline: {} ({})",
            mode.id(),
            blocker.reason,
            blocker.lifted_by
        );
    }

    let queries: Vec<QueryScore> = set
        .queries
        .iter()
        .map(|query| score_query(mode, corpus, query))
        .collect::<Result<_>>()?;

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

fn score_query(mode: SearchMode, corpus: &Corpus, query: &GoldenQuery) -> Result<QueryScore> {
    let ranking = mode.rank(corpus, &query.query)?;
    let relevant: RelevantSet = query.relevant.iter().cloned().collect();

    Ok(QueryScore {
        id: query.id.clone(),
        intent: query.intent,
        recall: RECALL_CUTOFFS
            .iter()
            .map(|&k| (k, recall_at_k(&ranking, &relevant, k)))
            .collect(),
        reciprocal_rank: reciprocal_rank(&ranking, &relevant),
    })
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
    fn every_blocker_names_what_would_lift_it() {
        for mode in SearchMode::all()
            .into_iter()
            .filter(|m| !m.is_measurable_offline())
        {
            let blocker = mode
                .offline_blocker()
                .expect("unmeasurable mode has a blocker");
            assert!(
                !blocker.lifted_by.trim().is_empty(),
                "{} reports UNAVAILABLE without saying what would fix it",
                mode.id()
            );
        }
    }

    #[test]
    fn results_are_requested_to_the_deepest_reported_cutoff() {
        // A result ranked past the deepest cutoff cannot move any reported
        // number, and one ranked before it must not be truncated away.
        assert_eq!(DEEPEST_REPORTED_CUTOFF, 10);
        assert_eq!(
            DEEPEST_REPORTED_CUTOFF,
            RECALL_CUTOFFS.into_iter().max().unwrap()
        );
    }

    #[test]
    fn unmeasurable_modes_refuse_to_produce_numbers() {
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let set = GoldenSet::embedded().unwrap();
        assert!(evaluate(SearchMode::Semantic, &corpus, &set).is_err());
    }

    #[test]
    fn ranking_an_unmeasurable_mode_errors_rather_than_scoring_zero() {
        // The guard in `evaluate` should make this unreachable — but if it is
        // ever bypassed, an empty ranking would report a plausible-looking 0.0
        // instead of failing, which is the one outcome this harness must not
        // produce.
        let corpus = Corpus::load(&fixtures_root().unwrap()).unwrap();
        let error = SearchMode::Hybrid
            .rank(&corpus, "anything")
            .expect_err("an unmeasurable mode must not return a ranking");
        assert!(error.to_string().contains("hybrid"));
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

    #[test]
    fn every_query_is_scored() {
        let set = GoldenSet::embedded().unwrap();
        assert_eq!(scored().queries.len(), set.len());
    }
}
