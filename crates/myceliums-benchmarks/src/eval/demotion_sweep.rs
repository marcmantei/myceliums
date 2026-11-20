//! Tuning the test-code demotion weight on the development split, confirming
//! on the held-out split (#146).
//!
//! Search ranks test code below product code by a rank weight
//! ([`myceliums_core::TestPolicy`]). The protocol, fixed before any number was
//! looked at:
//!
//! 1. Score every weight of [`WEIGHT_GRID`] — `1` is no demotion — in every
//!    available mode on the **dev** queries.
//! 2. Pick the weight with the highest [`OBJECTIVE`]: dev nDCG@10 averaged
//!    over the modes, each mode counting once, because one weight serves all
//!    of them ([`select`]). On a tie the milder weight (closer to `1`) wins.
//! 3. Score the pick and no demotion on the **held-out** queries through the
//!    production search paths and take the paired bootstrap difference of the
//!    objective, and of each mode's metrics.
//! 4. Keep demotion on by default only when the held-out objective interval
//!    lies wholly above zero ([`decide`]).
//!
//! Step 1 does not rerun every search per weight. Demotion reorders a ranking
//! that does not depend on the weight, so [`sweep`] computes that ranking once
//! per query — the BM25 ranking, the fused hybrid list (the production
//! [`myceliums_core::reciprocal_rank_fusion`] over the production candidate
//! pools) — and reorders it with the production [`TestPolicy::top`] for each
//! weight. Semantic search is cheap and runs per weight through the production
//! [`myceliums_core::semantic_search`]; the rerank pool depends on the weight,
//! and each distinct pool is reranked once by the production reranker
//! ([`ModelIndex::rerank`]). The
//! binary checks that the sweep reproduces the production scores at the
//! shipped weight and at `1` on every dev query and fails otherwise; step 3
//! does not rely on the shortcut.

use anyhow::Result;
use myceliums_core::{TestPolicy, DEFAULT_TEST_WEIGHT};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::bootstrap::{paired_bootstrap_default, DeltaEstimate};
use super::corpus::symbol_ref;
use super::evaluator::{
    mask_repeats, score_ranking, Metric, QueryScore, Retrieval, SearchMode, RANKING_DEPTH,
};
use super::golden_set::GoldenQuery;
use super::models::{ModelIndex, SHIPPED_HYBRID_LIMIT};
use super::report::rounded;

/// Weights swept. `1` reorders nothing; the smaller the weight, the further
/// test code moves down (a test at position `p` is placed at `p / weight`).
///
/// The grid stops at 0.1 by requirement, not by measurement: demotion must
/// be a down-weight, not a filter, and below 0.1 the best-ranked test falls
/// off the first page of ten results. The labels almost never name a test,
/// so this set cannot measure what a smaller weight costs an agent looking
/// for one — it would only ever favour pushing tests further down.
pub const WEIGHT_GRID: [f64; 6] = [1.0, 0.5, 0.33, 0.25, 0.2, 0.1];

/// The per-mode metric the objective averages.
pub const OBJECTIVE: Metric = Metric::NdcgAt10;

/// Metrics reported per mode for the held-out confirmation.
pub const REPORTED_METRICS: [Metric; 4] = [
    Metric::NdcgAt10,
    Metric::Mrr,
    Metric::RecallAt10,
    Metric::HitAt10,
];

/// The policy a weight stands for.
pub fn policy(weight: f64) -> TestPolicy {
    TestPolicy::demote(weight)
}

/// One repository's share of the sweep.
pub struct WeightSweepInput<'a> {
    /// The lexical retrieval over the repository's stored symbols.
    pub retrieval: &'a Retrieval<'a>,
    /// Its embedding index, when the model-backed modes run.
    pub index: Option<&'a ModelIndex>,
    /// The queries to score (the dev split).
    pub queries: Vec<GoldenQuery>,
}

/// Per weight (in `weights` order), per mode, the query scores in input order.
pub type WeightScores = Vec<BTreeMap<SearchMode, Vec<QueryScore>>>;

/// Score every weight in every mode of `modes` on `inputs`.
pub async fn sweep(
    inputs: &[WeightSweepInput<'_>],
    modes: &[SearchMode],
    weights: &[f64],
) -> Result<WeightScores> {
    let mut out: WeightScores = vec![BTreeMap::new(); weights.len()];
    let policies: Vec<TestPolicy> = weights.iter().map(|&w| policy(w)).collect();
    let deepest = |limit: usize| {
        policies
            .iter()
            .map(|p| p.depth_for(limit))
            .max()
            .unwrap_or(limit)
    };
    let score = |query: &GoldenQuery, symbols: &[myceliums_storage::CodeSymbol]| {
        score_ranking(
            query,
            &mask_repeats(symbols.iter().map(symbol_ref).collect()),
        )
    };
    for input in inputs {
        for query in &input.queries {
            // BM25 with test code in place, as deep as any weight reads.
            let bm25: Vec<_> =
                myceliums_core::search_symbols(input.retrieval.symbols(), &query.query)
                    .into_iter()
                    .take(deepest(RANKING_DEPTH))
                    .map(|hit| hit.symbol)
                    .collect();
            for (i, tests) in policies.iter().enumerate() {
                if modes.contains(&SearchMode::Lexical) {
                    let ranked = tests.top(bm25.clone(), RANKING_DEPTH, |s| s.is_test());
                    out[i]
                        .entry(SearchMode::Lexical)
                        .or_default()
                        .push(score(query, &ranked));
                }
            }
            let Some(index) = input.index else {
                continue;
            };
            let query_vector = index.embed_query(&query.query).await?;
            // The fused hybrid list before demotion, for the pool the
            // product's hybrid search draws from at its default limit.
            let fused = index
                .fusion_inputs(
                    &query.query,
                    SHIPPED_HYBRID_LIMIT,
                    &[myceliums_core::DEFAULT_VECTOR_CANDIDATES],
                )
                .await?
                .fuse_undemoted(myceliums_core::DEFAULT_RRF_K)?;
            for (i, tests) in policies.iter().enumerate() {
                if modes.contains(&SearchMode::Semantic) {
                    let hits = myceliums_core::semantic_search(
                        index.store(),
                        &query_vector,
                        RANKING_DEPTH,
                        tests,
                    )
                    .await?;
                    let ranked: Vec<_> = hits.into_iter().map(|(s, _)| s).collect();
                    out[i]
                        .entry(SearchMode::Semantic)
                        .or_default()
                        .push(score(query, &ranked));
                }
                let pool = tests.top(fused.clone(), SHIPPED_HYBRID_LIMIT, |r| r.symbol.is_test());
                if modes.contains(&SearchMode::Hybrid) {
                    let ranked: Vec<_> = pool
                        .iter()
                        .take(RANKING_DEPTH)
                        .map(|r| r.symbol.clone())
                        .collect();
                    out[i]
                        .entry(SearchMode::Hybrid)
                        .or_default()
                        .push(score(query, &ranked));
                }
                if modes.contains(&SearchMode::HybridRerank) {
                    let order = index.rerank(&query.query, pool).await?;
                    let ranked: Vec<_> = tests
                        .apply(order, |r| r.symbol.is_test())
                        .into_iter()
                        .take(RANKING_DEPTH)
                        .map(|r| r.symbol)
                        .collect();
                    out[i]
                        .entry(SearchMode::HybridRerank)
                        .or_default()
                        .push(score(query, &ranked));
                }
            }
        }
    }
    Ok(out)
}

/// Per query, the [`OBJECTIVE`] averaged over `modes` — the value the weight
/// is chosen on. Every mode must hold the same queries in the same order.
pub fn objective(
    by_mode: &BTreeMap<SearchMode, Vec<QueryScore>>,
    modes: &[SearchMode],
) -> Vec<f64> {
    let Some(first) = modes.first().and_then(|m| by_mode.get(m)) else {
        return Vec::new();
    };
    (0..first.len())
        .map(|q| {
            modes
                .iter()
                .map(|m| by_mode[m][q].values.get(OBJECTIVE))
                .sum::<f64>()
                / modes.len() as f64
        })
        .collect()
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}

/// The weight with the highest mean objective. A weight replaces a milder
/// one (closer to `1`, earlier in [`WEIGHT_GRID`]) only by scoring strictly
/// higher, so a tie keeps the milder demotion.
pub fn select(weights: &[f64], objectives: &[Vec<f64>]) -> f64 {
    let mut order: Vec<usize> = (0..weights.len()).collect();
    order.sort_by(|&a, &b| weights[b].total_cmp(&weights[a]));
    let mut best = order[0];
    for &i in &order[1..] {
        if mean(&objectives[i]) > mean(&objectives[best]) + 1e-12 {
            best = i;
        }
    }
    weights[best]
}

/// Paired bootstrap `treatment − control` on the reported metrics.
pub fn deltas(
    treatment: &[QueryScore],
    control: &[QueryScore],
) -> Result<BTreeMap<String, DeltaEstimate>> {
    let values = |scores: &[QueryScore], metric: Metric| -> Vec<f64> {
        scores.iter().map(|q| q.values.get(metric)).collect()
    };
    REPORTED_METRICS
        .iter()
        .map(|&metric| {
            let estimate =
                paired_bootstrap_default(&values(treatment, metric), &values(control, metric))?;
            Ok((metric.id().to_string(), rounded(estimate)))
        })
        .collect()
}

/// Paired bootstrap of the objective, `treatment − control`.
pub fn objective_delta(treatment: &[f64], control: &[f64]) -> Result<DeltaEstimate> {
    Ok(rounded(paired_bootstrap_default(treatment, control)?))
}

/// Whether the held-out split confirms demotion at the weight chosen on dev.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// The weight chosen on dev.
    pub chosen: f64,
    /// The weight that ships ([`DEFAULT_TEST_WEIGHT`]).
    pub shipped: f64,
    /// True when demotion should be on by default: the chosen weight demotes
    /// and its held-out objective interval lies wholly above zero.
    pub demote_by_default: bool,
    /// One sentence a reader can act on.
    pub reason: String,
}

/// Apply the confirmation rule to the held-out objective difference
/// (chosen − no demotion); `None` when the chosen weight is `1`.
pub fn decide(chosen: f64, held_out: Option<DeltaEstimate>) -> Decision {
    let shipped = DEFAULT_TEST_WEIGHT;
    let (demote_by_default, reason) = match held_out {
        None => (
            false,
            "no weight beat leaving test code in place on dev; demotion should be off by default"
                .to_string(),
        ),
        Some(estimate) if estimate.ci_low > 0.0 => (
            true,
            format!(
                "weight {chosen} raised the held-out objective by {:+.3} [{:+.3}, {:+.3}], an \
                 interval wholly above zero: demote test code by default{}",
                estimate.delta,
                estimate.ci_low,
                estimate.ci_high,
                if (chosen - shipped).abs() < 1e-9 {
                    String::new()
                } else {
                    format!(" — and ship weight {chosen}, not {shipped}")
                }
            ),
        ),
        Some(estimate) => (
            false,
            format!(
                "weight {chosen} moved the held-out objective by {:+.3} [{:+.3}, {:+.3}], an \
                 interval that does not exclude zero: demotion is not confirmed",
                estimate.delta, estimate.ci_low, estimate.ci_high
            ),
        ),
    };
    Decision {
        chosen,
        shipped,
        demote_by_default,
        reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_grid_holds_no_demotion_and_the_shipped_weight() {
        assert_eq!(WEIGHT_GRID[0], 1.0);
        assert!(policy(1.0).is_neutral());
        assert!(WEIGHT_GRID
            .iter()
            .any(|&w| (w - DEFAULT_TEST_WEIGHT).abs() < 1e-12));
        assert!(WEIGHT_GRID.iter().all(|&w| w > 0.0 && w <= 1.0));
    }

    #[test]
    fn select_prefers_the_milder_weight_on_a_tie() {
        let weights = [1.0, 0.5, 0.25];
        assert_eq!(select(&weights, &[vec![0.4], vec![0.5], vec![0.5]]), 0.5);
        assert_eq!(select(&weights, &[vec![0.4], vec![0.5], vec![0.6]]), 0.25);
        assert_eq!(select(&weights, &[vec![0.6], vec![0.5], vec![0.6]]), 1.0);
    }

    #[test]
    fn decide_needs_an_interval_above_zero() {
        let estimate = |low: f64| DeltaEstimate {
            delta: 0.1,
            ci_low: low,
            ci_high: 0.2,
        };
        assert!(decide(0.25, Some(estimate(0.01))).demote_by_default);
        assert!(!decide(0.25, Some(estimate(-0.01))).demote_by_default);
        assert!(!decide(1.0, None).demote_by_default);
    }
}
