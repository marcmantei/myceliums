//! Retrieval-relevance metrics: recall@k, hit@k, reciprocal rank, nDCG@k and
//! R-precision.
//!
//! These are the standard information-retrieval measures, defined here over a
//! *ranking* (an ordered list of retrieved symbols, best first) and a *relevant
//! set* (the symbols a human labelled as correct answers for the query).
//! Relevance is binary: a symbol is either a labelled answer or it is not.
//!
//! Every metric is a pure function of its inputs — same ranking and same
//! relevant set always produce the same number, with no clock, no I/O and no
//! model involved.

use super::golden_set::SymbolRef;
use std::collections::BTreeSet;

/// The set of symbols a human labelled as correct answers for one query.
pub type RelevantSet = BTreeSet<SymbolRef>;

/// Fraction of the relevant symbols that appear in the top `k` results.
///
/// Recall divides by `|relevant|`, so it is undefined when nothing is labelled.
/// This returns `0.0` for that case: a data-quality problem should show up as a
/// visibly bad score, whereas `1.0` would read as "found everything" and let an
/// unlabelled query flatter every aggregate it appears in. The golden-set
/// validator rejects unlabelled queries, so evaluation never reaches this path
/// — it exists to make the function total and to fail conspicuously if the
/// validator is ever bypassed.
pub fn recall_at_k(ranking: &[SymbolRef], relevant: &RelevantSet, k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let found = ranking
        .iter()
        .take(k)
        .filter(|hit| relevant.contains(*hit))
        .count();
    found as f64 / relevant.len() as f64
}

/// Reciprocal of the 1-based rank of the first relevant symbol, or `0.0` if
/// none of the results are relevant.
///
/// A correct answer in position 1 scores `1.0`, position 2 scores `0.5`,
/// position 4 scores `0.25` — the measure rewards putting the right symbol
/// first, which is what an agent with a small context window actually needs.
pub fn reciprocal_rank(ranking: &[SymbolRef], relevant: &RelevantSet) -> f64 {
    ranking
        .iter()
        .position(|hit| relevant.contains(hit))
        .map(|zero_based| 1.0 / (zero_based + 1) as f64)
        .unwrap_or(0.0)
}

/// `1.0` when at least one relevant symbol appears in the top `k`, else `0.0`.
///
/// The "did the agent get *anything* useful in its first k results" measure.
/// Unlike recall it does not penalise a query for carrying many labels, so it is
/// the fairer headline for queries whose answer set is larger than `k`.
pub fn hit_at_k(ranking: &[SymbolRef], relevant: &RelevantSet, k: usize) -> f64 {
    let hit = ranking.iter().take(k).any(|hit| relevant.contains(hit));
    if hit {
        1.0
    } else {
        0.0
    }
}

/// Normalised discounted cumulative gain over the top `k`, binary relevance.
///
/// ```text
/// DCG@k  = Σ_{i=1..k, ranking[i] relevant} 1 / log2(i + 1)
/// IDCG@k = Σ_{i=1..min(k, |relevant|)}     1 / log2(i + 1)
/// nDCG@k = DCG@k / IDCG@k
/// ```
///
/// Rewards every relevant symbol, not just the first, with a logarithmic
/// discount by rank — so it moves when a second or third answer climbs, which
/// MRR cannot see. The ideal ranking puts all labelled answers first, so a
/// query with more labels than `k` can still reach `1.0`. Returns `0.0` without
/// labels for the same reason [`recall_at_k`] does.
pub fn ndcg_at_k(ranking: &[SymbolRef], relevant: &RelevantSet, k: usize) -> f64 {
    if relevant.is_empty() || k == 0 {
        return 0.0;
    }
    let discount = |zero_based: usize| 1.0 / ((zero_based + 2) as f64).log2();
    let dcg: f64 = ranking
        .iter()
        .take(k)
        .enumerate()
        .filter(|(_, hit)| relevant.contains(*hit))
        .map(|(position, _)| discount(position))
        .sum();
    let ideal: f64 = (0..relevant.len().min(k)).map(discount).sum();
    dcg / ideal
}

/// Precision at rank `R`, where `R = |relevant|`.
///
/// The fraction of the first `R` results that are relevant — equivalently
/// recall at `R`, which is why it needs no separate cutoff. A query with one
/// labelled answer scores it as hit@1; a query with five scores how many of the
/// first five results are answers. Returns `0.0` without labels.
///
/// The ranking must be at least `R` deep for this to be exact; a shorter
/// ranking is scored as if every missing position were irrelevant, so callers
/// fetch to the deepest labelled set (see the evaluator's `RANKING_DEPTH`).
pub fn r_precision(ranking: &[SymbolRef], relevant: &RelevantSet) -> f64 {
    recall_at_k(ranking, relevant, relevant.len())
}

/// Arithmetic mean of `values`, or `0.0` for an empty slice.
///
/// Every query counts equally, so a mode cannot inflate its score by doing well
/// on the queries that happen to have many labelled answers.
pub fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    values.iter().sum::<f64>() / values.len() as f64
}

/// Mean reciprocal rank over a set of rankings — one ranking per query.
///
/// This is the aggregate reported in the baseline: [`reciprocal_rank`] scores a
/// single query, `mrr` averages those scores across the whole golden set.
pub fn mrr(rankings: &[(Vec<SymbolRef>, RelevantSet)]) -> f64 {
    let per_query: Vec<f64> = rankings
        .iter()
        .map(|(ranking, relevant)| reciprocal_rank(ranking, relevant))
        .collect();
    mean(&per_query)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str) -> SymbolRef {
        SymbolRef::new("fixture.py", name)
    }

    fn relevant(names: &[&str]) -> RelevantSet {
        names.iter().map(|n| sym(n)).collect()
    }

    #[test]
    fn recall_counts_only_the_top_k() {
        let ranking = vec![sym("a"), sym("b"), sym("c")];
        let want = relevant(&["c"]);
        assert_eq!(recall_at_k(&ranking, &want, 1), 0.0);
        assert_eq!(recall_at_k(&ranking, &want, 3), 1.0);
    }

    #[test]
    fn recall_is_a_fraction_of_all_relevant_symbols() {
        let ranking = vec![sym("a"), sym("b")];
        let want = relevant(&["a", "b", "missing"]);
        // Two of three labelled answers retrieved.
        assert!((recall_at_k(&ranking, &want, 10) - 2.0 / 3.0).abs() < 1e-12);
    }

    #[test]
    fn recall_is_zero_without_labels() {
        assert_eq!(recall_at_k(&[sym("a")], &RelevantSet::new(), 5), 0.0);
    }

    #[test]
    fn reciprocal_rank_rewards_the_first_hit() {
        let ranking = vec![sym("a"), sym("b"), sym("c")];
        assert_eq!(reciprocal_rank(&ranking, &relevant(&["a"])), 1.0);
        assert_eq!(reciprocal_rank(&ranking, &relevant(&["b"])), 0.5);
        assert_eq!(reciprocal_rank(&ranking, &relevant(&["c"])), 1.0 / 3.0);
    }

    #[test]
    fn reciprocal_rank_uses_the_best_placed_relevant_symbol() {
        let ranking = vec![sym("a"), sym("b")];
        assert_eq!(reciprocal_rank(&ranking, &relevant(&["b", "a"])), 1.0);
    }

    #[test]
    fn reciprocal_rank_is_zero_on_a_miss() {
        assert_eq!(reciprocal_rank(&[sym("a")], &relevant(&["z"])), 0.0);
    }

    #[test]
    fn mean_of_nothing_is_zero() {
        assert_eq!(mean(&[]), 0.0);
        assert_eq!(mean(&[1.0, 0.0]), 0.5);
    }

    #[test]
    fn mrr_averages_reciprocal_ranks_across_queries() {
        // First query: hit at rank 1 (1.0). Second: hit at rank 2 (0.5).
        let rankings = vec![
            (vec![sym("a"), sym("b")], relevant(&["a"])),
            (vec![sym("a"), sym("b")], relevant(&["b"])),
        ];
        assert_eq!(mrr(&rankings), 0.75);
    }

    #[test]
    fn mrr_of_nothing_is_zero() {
        assert_eq!(mrr(&[]), 0.0);
    }

    #[test]
    fn hit_at_k_needs_only_one_answer_in_the_cutoff() {
        let ranking = vec![sym("a"), sym("b"), sym("c")];
        let want = relevant(&["c", "missing"]);
        assert_eq!(hit_at_k(&ranking, &want, 2), 0.0);
        assert_eq!(hit_at_k(&ranking, &want, 3), 1.0);
        assert_eq!(hit_at_k(&ranking, &RelevantSet::new(), 3), 0.0);
    }

    #[test]
    fn ndcg_is_one_for_an_ideal_ranking() {
        let ranking = vec![sym("a"), sym("b"), sym("x")];
        assert!((ndcg_at_k(&ranking, &relevant(&["a", "b"]), 10) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn ndcg_discounts_by_log_rank() {
        // One relevant symbol at rank 2: DCG = 1/log2(3), IDCG = 1/log2(2) = 1.
        let ranking = vec![sym("x"), sym("a")];
        let expected = 1.0 / 3f64.log2();
        assert!((ndcg_at_k(&ranking, &relevant(&["a"]), 10) - expected).abs() < 1e-12);
    }

    #[test]
    fn ndcg_sees_the_second_answer_where_reciprocal_rank_does_not() {
        let want = relevant(&["a", "b"]);
        let second_high = vec![sym("a"), sym("b"), sym("x")];
        let second_low = vec![sym("a"), sym("x"), sym("b")];
        assert_eq!(
            reciprocal_rank(&second_high, &want),
            reciprocal_rank(&second_low, &want)
        );
        assert!(ndcg_at_k(&second_high, &want, 10) > ndcg_at_k(&second_low, &want, 10));
    }

    #[test]
    fn ndcg_ignores_results_past_the_cutoff_and_caps_the_ideal_at_k() {
        let ranking = vec![sym("x"), sym("a")];
        assert_eq!(ndcg_at_k(&ranking, &relevant(&["a"]), 1), 0.0);
        // Three labels but k = 1: one answer at rank 1 is already ideal.
        let first = vec![sym("a")];
        assert_eq!(ndcg_at_k(&first, &relevant(&["a", "b", "c"]), 1), 1.0);
        assert_eq!(ndcg_at_k(&first, &RelevantSet::new(), 10), 0.0);
    }

    #[test]
    fn r_precision_cuts_at_the_number_of_labels() {
        let ranking = vec![sym("a"), sym("x"), sym("b"), sym("c")];
        // R = 3: top-3 holds a and b.
        assert!((r_precision(&ranking, &relevant(&["a", "b", "c"])) - 2.0 / 3.0).abs() < 1e-12);
        // R = 1: the first result is the answer.
        assert_eq!(r_precision(&ranking, &relevant(&["a"])), 1.0);
        assert_eq!(r_precision(&ranking, &relevant(&["b"])), 0.0);
    }
}
