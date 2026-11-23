//! Ranking test code below product code (#146).
//!
//! On a real repository much of what a search returns is test code: on hono,
//! two thirds of the exact semantic top 10 sit in test files, which no
//! labelled answer names. The index records which symbols are tests
//! ([`myceliums_storage::CodeSymbol::is_test`], set at index time by
//! [`crate::parser::classify`]), and every search mode reorders its ranking by
//! a [`TestPolicy`] before cutting it to the requested length.
//!
//! The demotion works on **rank positions**, not scores, so one weight means
//! the same in every mode — BM25 scores, cosine similarities, fusion scores
//! and cross-encoder logits live on unrelated scales. A demoted symbol at
//! position `p` (1-based) is placed as if it were at `p / weight`: with weight
//! 0.2 a test at position 2 is placed at 10: product symbols ranked 3 to 9
//! move ahead of it, the one ranked 10 and everything below stay behind.
//! Nothing is removed: a test that clearly beats the product code still
//! shows, only later, and `TestRanking::Include` turns the reordering off.
//! The weight was tuned on the development split of the real-repository set
//! and confirmed on its held-out split
//! (`crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`).

use myceliums_storage::{CodeSymbol, SymbolKind};
use serde::{Deserialize, Serialize};

/// Rank weight of a demoted symbol by default: a test at position `p` is
/// placed as if at `p / DEFAULT_TEST_WEIGHT` = `5p`, so it lands no lower
/// than position `5p` — a test ranked 1st at 5th at worst, 2nd at 10th, 3rd at
/// 15th.
///
/// Chosen on the real-repository set as the strongest weight that keeps the
/// test-seeking queries' hit@10 at 80% of no demotion while improving the
/// regular queries (`real-repo-eval`, #146; numbers in
/// `crates/myceliums-benchmarks/benchmarks/METHODOLOGY.md`).
pub const DEFAULT_TEST_WEIGHT: f64 = 0.2;

/// The furthest a ranking is fetched past the requested length to make room
/// for demoted symbols: `limit / weight`, at most this many times `limit`.
const MAX_DEPTH_FACTOR: usize = 50;

/// How a search ranks the symbols the index classified as test code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestRanking {
    /// Test code ranks below product code (the default).
    #[default]
    Demote,
    /// Test code ranks like product code: the ranking is not reordered.
    Include,
    /// Product code ranks below test code — for questions about the tests.
    Prefer,
}

impl TestRanking {
    /// The name used in parameters and output: `demote`, `include`, `prefer`.
    pub fn as_str(self) -> &'static str {
        match self {
            TestRanking::Demote => "demote",
            TestRanking::Include => "include",
            TestRanking::Prefer => "prefer",
        }
    }
}

impl std::str::FromStr for TestRanking {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim().to_ascii_lowercase().as_str() {
            "demote" => Ok(TestRanking::Demote),
            "include" => Ok(TestRanking::Include),
            "prefer" => Ok(TestRanking::Prefer),
            other => Err(format!(
                "unknown test ranking '{other}' (expected demote, include or prefer)"
            )),
        }
    }
}

/// A [`TestRanking`] and the rank weight it applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TestPolicy {
    /// Which symbols are moved down.
    pub ranking: TestRanking,
    /// Rank weight of a moved symbol, in `(0, 1]`: at position `p` it is
    /// placed as if at `p / weight`. `1` reorders nothing.
    pub weight: f64,
    /// Doc sections and documents are moved down too, by the same weight
    /// (#180): the MCP `search_code` sets it, so a doc page that repeats a
    /// query's words does not push code out of a code search. Off by
    /// default: the library, the CLI and the retrieval evaluations rank docs
    /// like code.
    pub docs: bool,
}

impl Default for TestPolicy {
    /// Demote test code by [`DEFAULT_TEST_WEIGHT`] — what search does unless
    /// told otherwise.
    fn default() -> Self {
        Self::new(TestRanking::Demote)
    }
}

impl From<TestRanking> for TestPolicy {
    fn from(ranking: TestRanking) -> Self {
        Self::new(ranking)
    }
}

impl TestPolicy {
    /// `ranking` with the shipped [`DEFAULT_TEST_WEIGHT`].
    pub fn new(ranking: TestRanking) -> Self {
        Self {
            ranking,
            weight: DEFAULT_TEST_WEIGHT,
            docs: false,
        }
    }

    /// No reordering: test code ranks like product code.
    pub fn include() -> Self {
        Self::new(TestRanking::Include)
    }

    /// Demote test code by `weight` — for measuring other weights through
    /// the production code path. Clamped to `(0, 1]`.
    pub fn demote(weight: f64) -> Self {
        Self {
            ranking: TestRanking::Demote,
            weight,
            docs: false,
        }
    }

    fn weight(&self) -> f64 {
        if self.weight.is_nan() {
            1.0
        } else {
            self.weight.clamp(f64::MIN_POSITIVE, 1.0)
        }
    }

    /// This policy with doc sections and documents moved down as well.
    pub fn demoting_docs(mut self, docs: bool) -> Self {
        self.docs = docs;
        self
    }

    /// True when this policy leaves every ranking as it is.
    pub fn is_neutral(&self) -> bool {
        (self.ranking == TestRanking::Include && !self.docs) || self.weight() >= 1.0
    }

    /// The class [`apply`](Self::apply) and [`top`](Self::top) move by, for
    /// `symbol`: test code; with [`docs`](Self::docs), doc sections and
    /// documents too (under `prefer`, which moves product code down, a doc
    /// counts as product code). Pass it as their `is_test` argument.
    pub fn demoted_class(&self, symbol: &CodeSymbol) -> bool {
        let doc = self.docs && matches!(symbol.kind, SymbolKind::Section | SymbolKind::Document);
        match self.ranking {
            TestRanking::Demote => symbol.is_test() || doc,
            TestRanking::Include => doc,
            TestRanking::Prefer => symbol.is_test() && !doc,
        }
    }

    /// How deep a ranking must be fetched so that its first `limit` entries
    /// after [`apply`](Self::apply) are exactly what reordering the whole
    /// ranking would give: `limit / weight` (a symbol further down is placed
    /// below position `limit` whatever it is). Bounded by
    /// `MAX_DEPTH_FACTOR × limit`.
    pub fn depth_for(&self, limit: usize) -> usize {
        if self.is_neutral() {
            return limit;
        }
        let depth = (limit as f64 / self.weight()).ceil() as usize;
        depth.clamp(limit, limit.saturating_mul(MAX_DEPTH_FACTOR))
    }

    /// Whether this policy moves a symbol down — a test under `demote`,
    /// product code under `prefer` — for explain output.
    pub fn demotes(&self, is_test: bool) -> bool {
        !self.is_neutral() && self.moves(is_test)
    }

    /// Whether this policy moves a symbol of the given class down.
    fn moves(&self, is_test: bool) -> bool {
        match self.ranking {
            TestRanking::Demote => is_test,
            // Only reached with `docs`: the class is then the docs alone.
            TestRanking::Include => is_test,
            TestRanking::Prefer => !is_test,
        }
    }

    /// Reorder `ranking` (best first): a moved symbol at position `p` is
    /// placed as if at `p / weight`. The order within each class is kept, and
    /// on an exact tie the symbol that ranked higher before stays ahead.
    pub fn apply<T>(&self, ranking: Vec<T>, is_test: impl Fn(&T) -> bool) -> Vec<T> {
        if self.is_neutral() {
            return ranking;
        }
        let weight = self.weight();
        let mut keyed: Vec<(f64, T)> = ranking
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let position = (i + 1) as f64;
                let key = if self.moves(is_test(&item)) {
                    position / weight
                } else {
                    position
                };
                (key, item)
            })
            .collect();
        // Stable: equal keys keep their original order.
        keyed.sort_by(|a, b| a.0.total_cmp(&b.0));
        keyed.into_iter().map(|(_, item)| item).collect()
    }

    /// The first `limit` entries of `ranking` after [`apply`](Self::apply),
    /// reading only the first [`depth_for`](Self::depth_for)`(limit)` of it.
    pub fn top<T>(
        &self,
        mut ranking: Vec<T>,
        limit: usize,
        is_test: impl Fn(&T) -> bool,
    ) -> Vec<T> {
        ranking.truncate(self.depth_for(limit));
        let mut ranked = self.apply(ranking, is_test);
        ranked.truncate(limit);
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `(name, is_test)` rankings.
    fn ranking(spec: &str) -> Vec<(char, bool)> {
        spec.chars().map(|c| (c, c.is_ascii_uppercase())).collect()
    }

    fn names(items: &[(char, bool)]) -> String {
        items.iter().map(|(c, _)| *c).collect()
    }

    #[test]
    fn demote_moves_tests_by_rank_weight() {
        // Upper case = test. Weight 0.5: A at 1 → 2, B at 2 → 4, C at 4 → 8.
        let items = ranking("ABdCef");
        let ranked = TestPolicy::demote(0.5).apply(items, |i| i.1);
        // keys: A 2, B 4, d 3, C 8, e 5, f 6 → A(2) d(3) B(4) e(5) f(6) C(8)
        assert_eq!(names(&ranked), "AdBefC");
    }

    #[test]
    fn ties_keep_the_original_order() {
        // Weight 0.5: A at 1 → 2 ties with b at 2; A ranked higher before.
        let ranked = TestPolicy::demote(0.5).apply(ranking("Ab"), |i| i.1);
        assert_eq!(names(&ranked), "Ab");
    }

    #[test]
    fn include_and_weight_one_reorder_nothing() {
        let items = ranking("ABcd");
        assert_eq!(
            names(&TestPolicy::include().apply(items.clone(), |i| i.1)),
            "ABcd"
        );
        assert_eq!(
            names(&TestPolicy::demote(1.0).apply(items.clone(), |i| i.1)),
            "ABcd"
        );
        assert!(TestPolicy::include().is_neutral());
        assert_eq!(TestPolicy::include().depth_for(20), 20);
    }

    #[test]
    fn prefer_moves_product_code_down() {
        let policy = TestPolicy {
            ranking: TestRanking::Prefer,
            weight: 0.25,
            docs: false,
        };
        let ranked = policy.apply(ranking("abCd"), |i| i.1);
        // keys: a 4, b 8, C 3, d 16
        assert_eq!(names(&ranked), "Cabd");
    }

    #[test]
    fn nothing_is_dropped() {
        let items = ranking("ABCDEfgh");
        let ranked = TestPolicy::default().apply(items.clone(), |i| i.1);
        assert_eq!(ranked.len(), items.len());
        let mut sorted = names(&ranked).chars().collect::<Vec<_>>();
        sorted.sort_unstable();
        assert_eq!(sorted.into_iter().collect::<String>(), "ABCDEfgh");
    }

    /// Cutting the ranking at `depth_for(limit)` gives the same top `limit`
    /// as reordering all of it, whatever the mix of tests.
    #[test]
    fn top_reads_only_as_deep_as_needed() {
        let specs = [
            "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghij",
            "aBcDeFgHiJkLmNoPqRsTuVwXyZ",
            "abcdefghijklmnopqrstuvwxyzABC",
            "ABCDEFGHIJKLMNOPQRSTUVWXYZABCDEFGHIJKLMNOPQRSTUVWXYZabc",
        ];
        for weight in [0.5, 0.33, 0.25, 0.1] {
            let policy = TestPolicy::demote(weight);
            for spec in specs {
                for limit in [1, 3, 5, 10] {
                    let full = policy.apply(ranking(spec), |i| i.1);
                    let top = policy.top(ranking(spec), limit, |i| i.1);
                    assert_eq!(
                        names(&top),
                        names(&full[..limit.min(full.len())]),
                        "weight {weight}, limit {limit}, {spec}"
                    );
                }
            }
        }
    }

    #[test]
    fn rankings_parse_from_their_names() {
        for ranking in [
            TestRanking::Demote,
            TestRanking::Include,
            TestRanking::Prefer,
        ] {
            assert_eq!(ranking.as_str().parse::<TestRanking>(), Ok(ranking));
        }
        assert!("exclude".parse::<TestRanking>().is_err());
        assert_eq!(TestRanking::default(), TestRanking::Demote);
    }

    fn symbol(kind: SymbolKind, test: bool) -> CodeSymbol {
        CodeSymbol {
            uid: format!("{kind}-{test}"),
            name: "x".to_string(),
            qualified_name: "x".to_string(),
            kind,
            file_path: "x".to_string(),
            start_line: 1,
            end_line: 1,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: test.then(|| r#"{"test":true}"#.to_string()),
        }
    }

    #[test]
    fn docs_move_down_only_when_asked() {
        let code = symbol(SymbolKind::Function, false);
        let test = symbol(SymbolKind::Function, true);
        let doc = symbol(SymbolKind::Section, false);
        let ranking = vec![&doc, &code, &test];
        let order = |policy: TestPolicy| -> Vec<String> {
            policy
                .apply(ranking.clone(), |s| policy.demoted_class(s))
                .iter()
                .map(|s| s.uid.clone())
                .collect()
        };
        // Default: docs rank like code (the library and the evaluations).
        assert_eq!(
            order(TestPolicy::default()),
            ["Section-false", "Function-false", "Function-true"]
        );
        // `search_code`: docs below code, with the tests.
        assert_eq!(
            order(TestPolicy::default().demoting_docs(true)),
            ["Function-false", "Section-false", "Function-true"]
        );
        // Tests included, docs still demoted.
        let included = TestPolicy::include().demoting_docs(true);
        assert!(!included.is_neutral());
        assert_eq!(
            order(included),
            ["Function-false", "Function-true", "Section-false"]
        );
        assert!(TestPolicy::include().is_neutral());
        // Prefer: product code and docs below tests.
        assert_eq!(
            order(TestPolicy::new(TestRanking::Prefer).demoting_docs(true))[0],
            "Function-true"
        );
    }
}
