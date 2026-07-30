//! Retrieval-relevance metrics: recall@k and mean reciprocal rank.
//!
//! These are the standard information-retrieval measures, defined here over a
//! *ranking* (an ordered list of retrieved symbols, best first) and a *relevant
//! set* (the symbols a human labelled as correct answers for the query).
//!
//! Both metrics are pure functions of their inputs — same ranking and same
//! relevant set always produce the same number, with no clock, no I/O and no
//! model involved.

use super::golden_set::SymbolRef;
use std::collections::BTreeSet;

/// The set of symbols a human labelled as correct answers for one query.
pub type RelevantSet = BTreeSet<SymbolRef>;

/// Fraction of the relevant symbols that appear in the top `k` results.
///
/// Returns `0.0` when the relevant set is empty — a query with no labelled
/// answer cannot be satisfied, and silently returning `1.0` would flatter the
/// engine. The golden-set validator rejects such queries before evaluation, so
/// this is a defensive floor rather than an expected path.
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
}
