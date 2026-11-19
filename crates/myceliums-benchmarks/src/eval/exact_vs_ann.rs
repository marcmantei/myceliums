//! Exact against approximate vector search on a real repository (#141).
//!
//! The #95 real-repo run before #135 searched hono (10,420 symbols) through
//! the pre-#117 IVF-PQ index and scored semantic nDCG@10 0.314; the exact
//! scan that ships scored 0.254. The `exact-vs-ann` diagnostic runs both paths
//! over the same stored vectors and the same query vectors. This module holds
//! the parts of it that decide something and are tested here:
//!
//! - [`BruteForce`] — cosine against every stored vector, in `f64`: the
//!   oracle the exact path must reproduce;
//! - [`check_exact`] — are a search's results the `k` highest cosines of the
//!   table, in order (tie-tolerant)?
//! - [`label_ranks`] — where each labelled symbol sits in the exact ordering
//!   of the whole table, so a gain of the approximate path can be traced to the
//!   labels it promoted and from how deep.

use anyhow::{bail, Result};
use myceliums_storage::CodeSymbol;
use serde::Serialize;
use std::collections::HashMap;

use super::corpus::symbol_ref;
use super::golden_set::SymbolRef;

/// Largest difference, in cosine, between what a search returned and the
/// brute-force oracle that still counts as agreement. LanceDB computes squared
/// L2 in `f32` over 384 dimensions; on unit vectors that carries an error of a
/// few `1e-7` in cosine. Anything near `1e-5` is a different vector, not
/// rounding.
pub const EXACT_TOLERANCE: f64 = 1e-5;

/// Cosine similarity of two vectors, in `f64`. Does not assume unit length, so
/// a store holding unnormalised vectors shows up as a disagreement with the
/// `1 − d²/2` score the store exposes.
pub fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let (mut dot, mut na, mut nb) = (0.0f64, 0.0f64, 0.0f64);
    for (x, y) in a.iter().zip(b) {
        let (x, y) = (f64::from(*x), f64::from(*y));
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        0.0
    } else {
        dot / (na.sqrt() * nb.sqrt())
    }
}

/// Every stored symbol with its stored vector.
pub struct BruteForce {
    symbols: Vec<CodeSymbol>,
    vectors: Vec<Vec<f32>>,
    by_uid: HashMap<String, usize>,
}

impl BruteForce {
    /// From `Store::get_symbols_with_vectors`. Errors on a symbol without a
    /// vector: the exact path would never return it, and the oracle must
    /// search exactly what the store searches.
    pub fn new(rows: Vec<(CodeSymbol, Option<Vec<f32>>)>) -> Result<Self> {
        let mut symbols = Vec::with_capacity(rows.len());
        let mut vectors = Vec::with_capacity(rows.len());
        for (symbol, vector) in rows {
            let Some(vector) = vector else {
                bail!(
                    "symbol {} ({}) has no stored vector",
                    symbol.qualified_name,
                    symbol.file_path
                );
            };
            symbols.push(symbol);
            vectors.push(vector);
        }
        let by_uid = symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (s.uid.clone(), i))
            .collect();
        Ok(Self {
            symbols,
            vectors,
            by_uid,
        })
    }

    /// Rows in the table.
    pub fn len(&self) -> usize {
        self.symbols.len()
    }

    /// True for an empty table.
    pub fn is_empty(&self) -> bool {
        self.symbols.is_empty()
    }

    /// Cosine of `query` against every row, in row order.
    pub fn cosines(&self, query: &[f32]) -> Cosines<'_> {
        let values: Vec<f64> = self.vectors.iter().map(|v| cosine(query, v)).collect();
        let mut descending = values.clone();
        descending.sort_by(|a, b| b.total_cmp(a));
        Cosines {
            table: self,
            values,
            descending,
        }
    }
}

/// One query's cosines against the whole table.
pub struct Cosines<'a> {
    table: &'a BruteForce,
    values: Vec<f64>,
    descending: Vec<f64>,
}

impl Cosines<'_> {
    /// Cosine of the row with `uid`, if the table has it.
    pub fn of(&self, uid: &str) -> Option<f64> {
        self.table.by_uid.get(uid).map(|&i| self.values[i])
    }

    /// The `i`-th highest cosine in the table (0-based).
    pub fn nth_best(&self, i: usize) -> Option<f64> {
        self.descending.get(i).copied()
    }

    /// 1-based rank a row of cosine `value` has in the exact ordering: one
    /// plus the rows that score strictly higher, so exact ties share a rank.
    pub fn rank_of(&self, value: f64) -> usize {
        1 + self.descending.partition_point(|&c| c > value)
    }

    /// Rows at least as close as the `k`-th best, minus [`EXACT_TOLERANCE`]
    /// — the tie-tolerant "true top `k`" a returned row is counted against.
    pub fn in_top(&self, value: f64, k: usize) -> bool {
        match self.nth_best(
            k.saturating_sub(1)
                .min(self.descending.len().saturating_sub(1)),
        ) {
            Some(kth) => value >= kth - EXACT_TOLERANCE,
            None => false,
        }
    }

    /// The `k` closest rows among those `keep` accepts, best first, with their
    /// cosines; exact ties keep row order.
    pub fn top_where(
        &self,
        k: usize,
        keep: impl Fn(&CodeSymbol) -> bool,
    ) -> Vec<(&CodeSymbol, f64)> {
        let mut rows: Vec<(usize, f64)> = self
            .values
            .iter()
            .enumerate()
            .filter(|(i, _)| keep(&self.table.symbols[*i]))
            .map(|(i, &v)| (i, v))
            .collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        rows.into_iter()
            .take(k)
            .map(|(i, v)| (&self.table.symbols[i], v))
            .collect()
    }
}

/// Whether `file` (repository-relative, `/`-separated) holds tests,
/// benchmarks or runtime tests rather than the library: a `*.test.*`,
/// `*.spec.*`, `*_test.*` or `test_*` file, or anything under a `test`,
/// `tests`, `__tests__`, `runtime-tests`, `benchmarks` or `perf-measures`
/// directory. The real-repo labels never name such a symbol unless the query
/// asks about tests, so the share of them in a top 10 is a share of slots no
/// label can fill.
pub fn is_test_file(file: &str) -> bool {
    let mut parts: Vec<&str> = file.split('/').collect();
    let name = parts.pop().unwrap_or_default();
    let in_test_dir = parts.iter().any(|d| {
        matches!(
            *d,
            "test" | "tests" | "__tests__" | "runtime-tests" | "benchmarks" | "perf-measures"
        )
    });
    in_test_dir
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
}

/// How far a search's results are from the brute-force oracle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ExactCheck {
    /// Results compared.
    pub results: usize,
    /// Largest `|score the store exposed − cosine|`.
    pub max_score_error: f64,
    /// Largest `|cosine of the i-th result − i-th highest cosine|`: zero when
    /// the results are the top `k` in order, up to exact ties.
    pub max_rank_error: f64,
}

impl ExactCheck {
    /// Both errors within [`EXACT_TOLERANCE`].
    pub fn holds(&self) -> bool {
        self.max_score_error <= EXACT_TOLERANCE && self.max_rank_error <= EXACT_TOLERANCE
    }
}

/// Compare `results` — `(uid, exposed score)`, best first — with the oracle.
/// Errors on a uid the table does not hold.
pub fn check_exact(results: &[(String, f32)], cosines: &Cosines<'_>) -> Result<ExactCheck> {
    let mut check = ExactCheck {
        results: results.len(),
        max_score_error: 0.0,
        max_rank_error: 0.0,
    };
    for (i, (uid, score)) in results.iter().enumerate() {
        let Some(value) = cosines.of(uid) else {
            bail!("the search returned uid {uid}, which the table does not hold");
        };
        let Some(best) = cosines.nth_best(i) else {
            bail!("the search returned more rows than the table holds");
        };
        check.max_score_error = check.max_score_error.max((f64::from(*score) - value).abs());
        check.max_rank_error = check.max_rank_error.max((value - best).abs());
    }
    Ok(check)
}

/// Where one labelled symbol sits in the exact ordering.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LabelRank {
    /// The label.
    pub label: SymbolRef,
    /// Its cosine to the query; the best one when several rows share the
    /// reference (overloads).
    pub cosine: f64,
    /// 1-based exact rank in the whole table; tied rows share a rank.
    pub exact_rank: usize,
}

/// The exact rank of every label of a query. Errors on a label no row holds.
pub fn label_ranks(
    table: &BruteForce,
    cosines: &Cosines<'_>,
    labels: &[SymbolRef],
) -> Result<Vec<LabelRank>> {
    let mut best: HashMap<SymbolRef, f64> = HashMap::new();
    for (symbol, &value) in table.symbols.iter().zip(&cosines.values) {
        let reference = symbol_ref(symbol);
        if labels.contains(&reference) {
            let entry = best.entry(reference).or_insert(f64::NEG_INFINITY);
            *entry = entry.max(value);
        }
    }
    labels
        .iter()
        .map(|label| match best.get(label) {
            Some(&value) => Ok(LabelRank {
                label: label.clone(),
                cosine: value,
                exact_rank: cosines.rank_of(value),
            }),
            None => bail!("label {label} names no stored symbol"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::SymbolKind;

    fn symbol(uid: &str, file: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 2,
            signature: String::new(),
            content: String::new(),
            repo_id: "r".to_string(),
            metadata: None,
        }
    }

    /// Four rows: `a` and its exact copy `b`, a nearby `c`, a far `d`.
    fn table() -> BruteForce {
        BruteForce::new(vec![
            (symbol("a", "x.ts", "f"), Some(vec![1.0, 0.0, 0.0])),
            (symbol("b", "y.ts", "f"), Some(vec![2.0, 0.0, 0.0])),
            (symbol("c", "x.ts", "g"), Some(vec![0.8, 0.6, 0.0])),
            (symbol("d", "x.ts", "h"), Some(vec![0.0, 0.0, 1.0])),
        ])
        .unwrap()
    }

    #[test]
    fn test_files_are_recognised() {
        for file in [
            "src/hono.test.ts",
            "src/utils/url.spec.ts",
            "context_test.go",
            "tests/test_requests.py",
            "runtime-tests/deno/hono.test.ts",
            "benchmarks/handle-event/index.mjs",
            "perf-measures/type-check/client.ts",
            "src/__tests__/a.ts",
        ] {
            assert!(is_test_file(file), "{file}");
        }
        for file in [
            "src/hono.ts",
            "src/helper/testing/index.ts",
            "src/middleware/jwt/jwt.ts",
            "context.go",
            "src/requests/sessions.py",
            "contest.py",
        ] {
            assert!(!is_test_file(file), "{file}");
        }
    }

    #[test]
    fn top_where_filters_and_keeps_row_order_on_ties() {
        let table = table();
        let cosines = table.cosines(&[1.0, 0.0, 0.0]);
        let uids = |rows: Vec<(&CodeSymbol, f64)>| -> Vec<String> {
            rows.into_iter().map(|(s, _)| s.uid.clone()).collect()
        };
        assert_eq!(uids(cosines.top_where(3, |_| true)), ["a", "b", "c"]);
        assert_eq!(uids(cosines.top_where(2, |s| s.uid != "a")), ["b", "c"]);
    }

    #[test]
    fn cosine_ignores_length() {
        assert!((cosine(&[3.0, 4.0], &[0.6, 0.8]) - 1.0).abs() < 1e-12);
        assert!((cosine(&[1.0, 0.0], &[0.0, 5.0])).abs() < 1e-12);
        assert_eq!(cosine(&[0.0, 0.0], &[1.0, 0.0]), 0.0);
    }

    #[test]
    fn a_symbol_without_a_vector_is_refused() {
        let rows = vec![(symbol("a", "x.ts", "f"), None)];
        assert!(BruteForce::new(rows).is_err());
    }

    #[test]
    fn the_true_top_k_in_order_passes() {
        let table = table();
        let cosines = table.cosines(&[1.0, 0.0, 0.0]);
        // The copies tie at 1.0; either order is exact.
        for results in [
            vec![
                ("a".to_string(), 1.0f32),
                ("b".to_string(), 1.0),
                ("c".to_string(), 0.8),
            ],
            vec![
                ("b".to_string(), 1.0f32),
                ("a".to_string(), 1.0),
                ("c".to_string(), 0.8),
            ],
        ] {
            let check = check_exact(&results, &cosines).unwrap();
            assert!(check.holds(), "{check:?}");
        }
    }

    #[test]
    fn a_skipped_neighbour_or_a_wrong_order_fails() {
        let table = table();
        let cosines = table.cosines(&[1.0, 0.0, 0.0]);
        // `c` skipped for `d`: the third result is not the third best.
        let skipped = vec![
            ("a".to_string(), 1.0f32),
            ("b".to_string(), 1.0),
            ("d".to_string(), 0.0),
        ];
        assert!(!check_exact(&skipped, &cosines).unwrap().holds());
        // `c` ranked above the copies.
        let reordered = vec![("c".to_string(), 0.8f32), ("a".to_string(), 1.0)];
        assert!(!check_exact(&reordered, &cosines).unwrap().holds());
    }

    #[test]
    fn a_score_that_is_not_the_cosine_fails() {
        let table = table();
        let cosines = table.cosines(&[1.0, 0.0, 0.0]);
        // Right rows, right order, but scored on a distance that is not cosine
        // (what an unnormalised vector or a quantised distance would expose).
        let results = vec![("a".to_string(), 0.9f32), ("b".to_string(), 1.0)];
        let check = check_exact(&results, &cosines).unwrap();
        assert!((check.max_score_error - 0.1).abs() < 1e-6);
        assert!(!check.holds());
    }

    #[test]
    fn an_unknown_uid_is_an_error() {
        let table = table();
        let cosines = table.cosines(&[1.0, 0.0, 0.0]);
        assert!(check_exact(&[("zz".to_string(), 1.0)], &cosines).is_err());
    }

    #[test]
    fn ties_share_a_rank_and_labels_take_their_best_row() {
        let table = table();
        let cosines = table.cosines(&[1.0, 0.0, 0.0]);
        let labels = vec![SymbolRef::new("y.ts", "f"), SymbolRef::new("x.ts", "g")];
        let ranks = label_ranks(&table, &cosines, &labels).unwrap();
        assert_eq!(ranks[0].exact_rank, 1, "b ties a at the top");
        assert_eq!(ranks[1].exact_rank, 3);
        assert!((ranks[1].cosine - 0.8).abs() < 1e-6);
        assert!(cosines.in_top(0.8, 3));
        assert!(!cosines.in_top(0.0, 3));
        assert!(label_ranks(&table, &cosines, &[SymbolRef::new("x.ts", "nope")]).is_err());
    }

    /// #141 regression guard for the exact path itself: on a table large
    /// enough for the pre-#117 index, with exact duplicate vectors (ties) and
    /// that index left on the table, every production `vector_search` returns
    /// the top `k` cosines in order with cosine scores — at the semantic depth
    /// (20) and at hybrid's vector-candidate count (100). The same oracle must
    /// reject the index's own results, or it could not tell the paths apart.
    #[tokio::test]
    async fn production_search_returns_the_brute_force_top_k() {
        use crate::ann::{Distribution, Synthetic};
        use myceliums_storage::{AnnIndexKind, AnnQuery, AnnSettings, Store};

        let data = Synthetic {
            queries: 30,
            ..Synthetic::standard(Distribution::Clustered, 2_000)
        }
        .generate();
        // Every 50th row repeats the one before it under another name, as
        // hono's per-runtime adapter copies do.
        let rows: Vec<(CodeSymbol, Vec<f32>)> = data
            .table
            .iter()
            .enumerate()
            .map(|(i, v)| {
                let v = if i % 50 == 1 {
                    data.table[i - 1].clone()
                } else {
                    v.clone()
                };
                (
                    symbol(
                        &format!("s{i}"),
                        &format!("f{}.ts", i / 7),
                        &format!("n{i}"),
                    ),
                    v,
                )
            })
            .collect();

        let dir = tempfile::tempdir().unwrap();
        let legacy = Store::open(dir.path(), "r")
            .await
            .unwrap()
            .with_ann_settings(AnnSettings {
                threshold: Some(1_000),
                index: AnnIndexKind::IvfPq { sub_vectors: 16 },
                query: AnnQuery::default(),
            });
        let symbols: Vec<CodeSymbol> = rows.iter().map(|(s, _)| s.clone()).collect();
        legacy.store_symbols(&symbols).await.unwrap();
        let pairs: Vec<(String, Vec<f32>)> = rows
            .iter()
            .map(|(s, v)| (s.uid.clone(), v.clone()))
            .collect();
        legacy.store_embeddings(pairs).await.unwrap();

        let production = Store::open(dir.path(), "r").await.unwrap();
        let table = BruteForce::new(production.get_symbols_with_vectors().await.unwrap()).unwrap();
        assert_eq!(table.len(), rows.len());

        let mut index_misses = 0;
        for query in &data.queries {
            let cosines = table.cosines(query);
            for k in [20, 100] {
                let results = production.vector_search(query, k).await.unwrap();
                let scored: Vec<(String, f32)> = results
                    .iter()
                    .map(|(s, score)| (s.uid.clone(), *score))
                    .collect();
                assert_eq!(scored.len(), k);
                let check = check_exact(&scored, &cosines).unwrap();
                assert!(check.holds(), "k = {k}: {check:?}");
            }
            let through_index: Vec<(String, f32)> = legacy
                .vector_search(query, 20)
                .await
                .unwrap()
                .iter()
                .map(|(s, score)| (s.uid.clone(), *score))
                .collect();
            if !check_exact(&through_index, &cosines).unwrap().holds() {
                index_misses += 1;
            }
        }
        assert!(
            index_misses > data.queries.len() / 2,
            "the IVF-PQ index matched the oracle on {} of {} queries — the oracle \
             cannot tell an index search from an exact one",
            data.queries.len() - index_misses,
            data.queries.len()
        );
    }
}
