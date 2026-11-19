//! Tuning hybrid search on the development split, confirming on the held-out
//! split (#95).
//!
//! The protocol is fixed before any number is looked at:
//!
//! 1. Score every configuration of [`grid`] on the **dev** queries of the
//!    real-repo set.
//! 2. Pick the one with the highest mean [`PRIMARY_METRIC`]; on a tie the
//!    shipped default wins ([`select`]).
//! 3. Score the pick and the shipped default on the **held-out** queries —
//!    through the production [`myceliums_core::hybrid_search_with`] — and take
//!    the paired bootstrap difference.
//! 4. Recommend changing the defaults only when that held-out interval lies
//!    wholly above zero ([`Decision`]). Otherwise the defaults stay, and the
//!    dev result is reported as what it is: the best of many tries on the
//!    queries it was chosen on.
//!
//! Step 1 scores 20 configurations per query, so it does not call
//! `hybrid_search_with` 20 times: [`super::models::ModelIndex::fusion_inputs`]
//! computes the BM25 ranking, the query embedding and the vector candidates
//! once, and each configuration fuses them with the production
//! [`myceliums_core::reciprocal_rank_fusion`]. The binary checks on
//! every dev query that this reproduces the production hybrid ranking at the
//! shipped defaults exactly; the decision-bearing step 3 does not rely on it.

use anyhow::Result;
use myceliums_core::{HybridParams, DEFAULT_RRF_K, DEFAULT_VECTOR_CANDIDATES};
use serde::{Deserialize, Serialize};

use super::bootstrap::{paired_bootstrap_default, DeltaEstimate};
use super::corpus::symbol_ref;
use super::evaluator::{
    mask_repeats, score_ranking, Metric, ModeScore, QueryScore, SearchMode, RANKING_DEPTH,
};
use super::golden_set::GoldenQuery;
use super::models::{ModelIndex, SHIPPED_HYBRID_LIMIT};
use super::report::rounded;

/// RRF constants swept.
pub const RRF_K_GRID: [f64; 5] = [10.0, 20.0, 40.0, 60.0, 100.0];

/// Minimum vector-candidate counts swept.
pub const VECTOR_CANDIDATE_GRID: [usize; 4] = [50, 100, 200, 400];

/// The metric a configuration is chosen on and confirmed with. nDCG@10 credits
/// every labelled answer by position, which suits the multi-answer conceptual
/// queries; MRR@10 and recall@10 are reported beside it.
pub const PRIMARY_METRIC: Metric = Metric::NdcgAt10;

/// Metrics every sweep row and the decision report a difference for.
pub const REPORTED_METRICS: [Metric; 4] = [
    Metric::NdcgAt10,
    Metric::Mrr,
    Metric::RecallAt10,
    Metric::HitAt10,
];

/// One point of the sweep — the serialisable form of [`HybridParams`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// RRF constant.
    pub rrf_k: f64,
    /// Minimum vector candidates.
    pub vector_candidates: usize,
}

impl Config {
    /// What ships today.
    pub fn shipped() -> Self {
        Self {
            rrf_k: DEFAULT_RRF_K,
            vector_candidates: DEFAULT_VECTOR_CANDIDATES,
        }
    }

    /// The parameters to hand to hybrid search.
    pub fn params(&self) -> HybridParams {
        HybridParams {
            rrf_k: self.rrf_k,
            vector_candidates: self.vector_candidates,
        }
    }

    /// True for the shipped default.
    pub fn is_shipped(&self) -> bool {
        *self == Self::shipped()
    }

    /// `k=60 cand=100` — for tables.
    pub fn label(&self) -> String {
        format!("k={} cand={}", self.rrf_k, self.vector_candidates)
    }
}

/// Every configuration swept, in a fixed order. Contains the shipped default.
pub fn grid() -> Vec<Config> {
    let mut configs = Vec::new();
    for &vector_candidates in &VECTOR_CANDIDATE_GRID {
        for &rrf_k in &RRF_K_GRID {
            configs.push(Config {
                rrf_k,
                vector_candidates,
            });
        }
    }
    configs
}

/// One repository's share of the sweep: its index and the dev queries over it.
pub struct SweepInput<'a> {
    /// The repository's embedding index.
    pub index: &'a ModelIndex,
    /// Its development queries.
    pub queries: Vec<GoldenQuery>,
}

/// Score every configuration of `configs` on `inputs`, one [`ModeScore`] per
/// configuration in the same order, queries in input order.
pub async fn sweep(inputs: &[SweepInput<'_>], configs: &[Config]) -> Result<Vec<ModeScore>> {
    let mut candidate_counts: Vec<usize> = configs.iter().map(|c| c.vector_candidates).collect();
    candidate_counts.sort_unstable();
    candidate_counts.dedup();
    let limit = RANKING_DEPTH.max(SHIPPED_HYBRID_LIMIT);

    let mut per_config: Vec<Vec<QueryScore>> = vec![Vec::new(); configs.len()];
    for input in inputs {
        for query in &input.queries {
            let fusion = input
                .index
                .fusion_inputs(&query.query, limit, &candidate_counts)
                .await?;
            for (config, scores) in configs.iter().zip(per_config.iter_mut()) {
                let ranking: Vec<_> = fusion
                    .fuse(&config.params(), limit)?
                    .iter()
                    .take(RANKING_DEPTH)
                    .map(symbol_ref)
                    .collect();
                scores.push(score_ranking(query, &mask_repeats(ranking)));
            }
        }
    }
    Ok(per_config
        .into_iter()
        .map(|scores| ModeScore::from_queries(SearchMode::Hybrid, scores))
        .collect())
}

/// The configuration with the highest mean [`PRIMARY_METRIC`].
///
/// The shipped default is the incumbent: another configuration replaces it
/// only by scoring strictly higher, so a tie never moves away from what ships.
/// Among challengers, the first in [`grid`] order wins a tie.
pub fn select(configs: &[Config], scores: &[ModeScore]) -> Config {
    let primary = |score: &ModeScore| score.mean.get(PRIMARY_METRIC);
    let mut best = Config::shipped();
    let mut best_score = configs
        .iter()
        .zip(scores)
        .find(|(config, _)| config.is_shipped())
        .map(|(_, score)| primary(score))
        .unwrap_or(f64::NEG_INFINITY);
    for (config, score) in configs.iter().zip(scores) {
        if primary(score) > best_score + 1e-12 {
            best = *config;
            best_score = primary(score);
        }
    }
    best
}

/// Paired bootstrap differences `treatment − control` for the reported
/// metrics. Both scores must cover the same queries in the same order.
pub fn deltas(treatment: &ModeScore, control: &ModeScore) -> Result<Vec<(Metric, DeltaEstimate)>> {
    REPORTED_METRICS
        .iter()
        .map(|&metric| {
            let estimate =
                paired_bootstrap_default(&treatment.per_query(metric), &control.per_query(metric))?;
            Ok((metric, rounded(estimate)))
        })
        .collect()
}

/// Whether the held-out split confirms the configuration chosen on dev.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    /// The configuration chosen on dev.
    pub chosen: Config,
    /// True only when `chosen` differs from the default and its held-out
    /// [`PRIMARY_METRIC`] interval lies wholly above zero.
    pub change_defaults: bool,
    /// One sentence a reader can act on.
    pub reason: String,
}

/// Apply the confirmation rule to the held-out difference on the primary
/// metric (`None` when the chosen configuration is the default and there is
/// nothing to confirm).
pub fn decide(chosen: Config, held_out_primary: Option<DeltaEstimate>) -> Decision {
    let metric = PRIMARY_METRIC.label();
    match held_out_primary {
        _ if chosen.is_shipped() => Decision {
            chosen,
            change_defaults: false,
            reason: format!(
                "no configuration beat the shipped default on the dev split's {metric}; \
                 the defaults stay"
            ),
        },
        Some(delta) if delta.significantly_positive() => Decision {
            chosen,
            change_defaults: true,
            reason: format!(
                "{} beat the default on held-out {metric} by {:+.4} with a 95% interval \
                 [{:+.4}, {:+.4}] above zero",
                chosen.label(),
                delta.delta,
                delta.ci_low,
                delta.ci_high
            ),
        },
        Some(delta) => Decision {
            chosen,
            change_defaults: false,
            reason: format!(
                "{} led on dev, but on held-out {metric} it moved by {:+.4} [{:+.4}, {:+.4}] \
                 — the interval does not exclude zero, so the defaults stay",
                chosen.label(),
                delta.delta,
                delta.ci_low,
                delta.ci_high
            ),
        },
        None => Decision {
            chosen,
            change_defaults: false,
            reason: format!(
                "{} led on dev but was not measured on held-out; the defaults stay",
                chosen.label()
            ),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eval::evaluator::{MetricValues, QueryScore};
    use crate::eval::golden_set::QueryIntent;

    fn score_with(primary: f64) -> ModeScore {
        let values = MetricValues::from_fn(|metric| {
            if metric == PRIMARY_METRIC {
                primary
            } else {
                0.0
            }
        });
        ModeScore::from_queries(
            SearchMode::Hybrid,
            vec![QueryScore {
                id: "q".into(),
                intent: QueryIntent::Paraphrase,
                values,
            }],
        )
    }

    #[test]
    fn the_grid_spans_the_requested_values_and_includes_the_default() {
        let configs = grid();
        assert_eq!(configs.len(), 5 * 4);
        assert!(configs.iter().any(Config::is_shipped));
        for k in [10.0, 20.0, 40.0, 60.0, 100.0] {
            assert!(configs.iter().any(|c| c.rrf_k == k));
        }
        for n in [50, 100, 200, 400] {
            assert!(configs.iter().any(|c| c.vector_candidates == n));
        }
    }

    #[test]
    fn the_shipped_config_is_the_core_default() {
        assert_eq!(Config::shipped().params(), HybridParams::default());
    }

    #[test]
    fn a_tie_keeps_the_default() {
        let configs = grid();
        let scores: Vec<ModeScore> = configs.iter().map(|_| score_with(0.5)).collect();
        assert!(select(&configs, &scores).is_shipped());
    }

    #[test]
    fn a_strictly_better_config_is_selected() {
        let configs = grid();
        let winner = configs.iter().position(|c| c.rrf_k == 20.0).unwrap();
        let scores: Vec<ModeScore> = configs
            .iter()
            .enumerate()
            .map(|(i, _)| score_with(if i == winner { 0.6 } else { 0.5 }))
            .collect();
        assert_eq!(select(&configs, &scores), configs[winner]);
    }

    #[test]
    fn defaults_change_only_when_the_held_out_interval_excludes_zero() {
        let challenger = Config {
            rrf_k: 20.0,
            ..Config::shipped()
        };
        let positive = DeltaEstimate {
            delta: 0.05,
            ci_low: 0.01,
            ci_high: 0.09,
        };
        let straddling = DeltaEstimate {
            delta: 0.05,
            ci_low: -0.01,
            ci_high: 0.11,
        };
        assert!(decide(challenger, Some(positive)).change_defaults);
        assert!(!decide(challenger, Some(straddling)).change_defaults);
        assert!(!decide(challenger, None).change_defaults);
        assert!(!decide(Config::shipped(), Some(positive)).change_defaults);
    }
}
