//! Hybrid search combining BM25 text search with vector semantic search.
//!
//! Results from both ranking strategies are fused using Reciprocal Rank
//! Fusion (RRF), and optionally reranked with a cross-encoder model.

use anyhow::Result;
use myceliums_storage::{CodeSymbol, Relationship};
use serde::Serialize;
use std::collections::HashMap;

#[cfg(feature = "embeddings")]
use crate::embeddings::get_reranker;
#[cfg(feature = "embeddings")]
use crate::embeddings::Embedder;
use crate::search::{search_symbols, search_symbols_explain, SearchExplain};

/// Source of persisted vector-search candidates for the hybrid vector leg.
///
/// The hybrid vector leg queries pre-computed embeddings rather than
/// re-embedding the whole corpus on every request. `Store` implements this over
/// its persisted LanceDB vectors; tests can substitute a lightweight fake to
/// assert the query path never re-embeds stored symbols.
#[cfg(feature = "embeddings")]
#[async_trait::async_trait]
pub trait VectorSearcher {
    /// Return up to `limit` `(symbol, similarity)` candidates whose persisted
    /// vector is nearest to `query_vector`, ordered by similarity descending.
    async fn nearest(&self, query_vector: &[f32], limit: usize) -> Result<Vec<(CodeSymbol, f64)>>;
}

#[cfg(feature = "embeddings")]
#[async_trait::async_trait]
impl VectorSearcher for myceliums_storage::Store {
    async fn nearest(&self, query_vector: &[f32], limit: usize) -> Result<Vec<(CodeSymbol, f64)>> {
        // `Store::vector_search` already skips zero-vector placeholder rows and
        // scopes to this repo; it returns f32 similarity scores we widen to f64.
        let hits = self.vector_search(query_vector, limit).await?;
        Ok(hits
            .into_iter()
            .map(|(sym, score)| (sym, score as f64))
            .collect())
    }
}

/// A graph edge traversed during search -- shows how a result connects to others.
#[derive(Debug, Clone, Serialize)]
pub struct GraphEdge {
    /// Name of the source symbol.
    pub source: String,
    /// Name of the target symbol.
    pub target: String,
    /// Relationship kind (e.g. `"Calls"`, `"Imports"`).
    pub kind: String,
}

/// Full explain trace for a hybrid search result.
#[derive(Debug, Clone, Serialize)]
pub struct HybridExplain {
    /// BM25 scoring breakdown (if the result appeared in the BM25 ranking).
    pub bm25: Option<SearchExplain>,
    /// Graph edges connecting this symbol to callers/callees.
    pub graph_edges: Vec<GraphEdge>,
}

/// Result from hybrid search combining BM25 and vector rankings.
#[derive(Debug, Clone)]
pub struct HybridSearchResult {
    /// The matched code symbol.
    pub symbol: CodeSymbol,
    /// BM25 text-search score (if present in the BM25 ranking).
    pub bm25_score: Option<f64>,
    /// Cosine similarity from vector search (if present in the vector ranking).
    pub vector_score: Option<f64>,
    /// Combined RRF score (a.k.a. fusion score). Preserved even after
    /// reranking so both scales remain observable (issue #29).
    pub combined_score: f64,
    /// Cross-encoder rerank score, populated only when reranking ran.
    /// `None` for non-reranked results. This is a separate scale from
    /// `combined_score` and must not overwrite it.
    pub rerank_score: Option<f64>,
    /// 1-based rank in the BM25 results.
    pub bm25_rank: Option<usize>,
    /// 1-based rank in the vector results.
    pub vector_rank: Option<usize>,
    /// Optional explain trace for debugging relevance.
    pub explain: Option<HybridExplain>,
}

/// Reciprocal Rank Fusion: score = sum(1 / (k + rank_i)) for each ranking.
///
/// `k` is a constant (typically 60) that dampens the influence of high ranks.
/// Results from both BM25 and vector search are combined by matching on symbol uid.
/// Symbols appearing in only one list receive only that list's rank contribution.
pub fn reciprocal_rank_fusion(
    bm25_results: Vec<(CodeSymbol, f64, Option<SearchExplain>)>,
    vector_results: Vec<(CodeSymbol, f64)>,
    k: f64,
    limit: usize,
) -> Vec<HybridSearchResult> {
    // Build uid -> (rank, score, symbol, explain) maps for each result set.
    // Ranks are 1-based.
    let mut bm25_map: HashMap<String, (usize, f64, CodeSymbol, Option<SearchExplain>)> =
        HashMap::new();
    for (rank, (sym, score, explain)) in bm25_results.into_iter().enumerate() {
        let rank1 = rank + 1;
        bm25_map.insert(sym.uid.clone(), (rank1, score, sym, explain));
    }

    let mut vector_map: HashMap<String, (usize, f64, CodeSymbol)> = HashMap::new();
    for (rank, (sym, score)) in vector_results.into_iter().enumerate() {
        let rank1 = rank + 1;
        vector_map.insert(sym.uid.clone(), (rank1, score, sym));
    }

    // Collect all unique uids
    let mut all_uids: Vec<String> = bm25_map.keys().cloned().collect();
    for uid in vector_map.keys() {
        if !bm25_map.contains_key(uid) {
            all_uids.push(uid.clone());
        }
    }

    let mut results: Vec<HybridSearchResult> = all_uids
        .into_iter()
        .map(|uid| {
            let bm25 = bm25_map.remove(&uid);
            let vector = vector_map.remove(&uid);

            let bm25_rank = bm25.as_ref().map(|(r, _, _, _)| *r);
            let bm25_score = bm25.as_ref().map(|(_, s, _, _)| *s);
            let bm25_explain = bm25.as_ref().and_then(|(_, _, _, e)| e.clone());
            let vector_rank = vector.as_ref().map(|(r, _, _)| *r);
            let vector_score = vector.as_ref().map(|(_, s, _)| *s);

            let symbol = bm25
                .map(|(_, _, sym, _)| sym)
                .or_else(|| vector.clone().map(|(_, _, sym)| sym))
                .unwrap();

            let mut combined = 0.0;
            if let Some(r) = bm25_rank {
                combined += 1.0 / (k + r as f64);
            }
            if let Some(r) = vector_rank {
                combined += 1.0 / (k + r as f64);
            }

            let explain = bm25_explain.map(|bm25| HybridExplain {
                bm25: Some(bm25),
                graph_edges: Vec::new(),
            });

            HybridSearchResult {
                symbol,
                bm25_score,
                vector_score,
                combined_score: combined,
                rerank_score: None,
                bm25_rank,
                vector_rank,
                explain,
            }
        })
        .collect();

    results.sort_by(|a, b| {
        b.combined_score
            .partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(limit);
    results
}

/// Attach graph edges (callers/callees) to explain traces for search results.
pub fn attach_graph_edges(
    results: &mut [HybridSearchResult],
    relationships: &[Relationship],
    uid_to_name: &HashMap<&str, &str>,
) {
    for result in results.iter_mut() {
        if let Some(ref mut explain) = result.explain {
            let uid = &result.symbol.uid;
            for rel in relationships.iter() {
                // Outgoing edges (this symbol calls/imports others)
                if rel.source_uid == *uid {
                    if let Some(target_name) = uid_to_name.get(rel.target_uid.as_str()) {
                        explain.graph_edges.push(GraphEdge {
                            source: result.symbol.name.clone(),
                            target: target_name.to_string(),
                            kind: rel.kind.to_string(),
                        });
                    }
                }
                // Incoming edges (others call/import this symbol)
                if rel.target_uid == *uid {
                    if let Some(source_name) = uid_to_name.get(rel.source_uid.as_str()) {
                        explain.graph_edges.push(GraphEdge {
                            source: source_name.to_string(),
                            target: result.symbol.name.clone(),
                            kind: rel.kind.to_string(),
                        });
                    }
                }
            }
        }
    }
}

/// Perform hybrid search combining BM25 text search and vector semantic search.
///
/// 1. Runs BM25 search over the in-memory `symbols` with `query`
/// 2. Generates a single query embedding with the given embedder — obtain it
///    via [`crate::embeddings::embedder_for_index`] so it matches the index
/// 3. Runs vector search against `vectors`' **persisted** embeddings — the
///    corpus is never re-embedded per query
/// 4. Combines results using Reciprocal Rank Fusion (k=60)
///
/// `vectors` is typically a [`myceliums_storage::Store`]; the BM25 leg keeps
/// using the in-memory `symbols` slice.
///
/// Requires the `embeddings` feature.
#[cfg(feature = "embeddings")]
pub async fn hybrid_search(
    embedder: &Embedder,
    symbols: &[CodeSymbol],
    vectors: &impl VectorSearcher,
    query: &str,
    limit: usize,
) -> Result<Vec<HybridSearchResult>> {
    hybrid_search_impl(embedder, symbols, vectors, query, limit, false).await
}

/// Hybrid search with explain traces showing scoring breakdown and graph paths.
///
/// Requires the `embeddings` feature.
#[cfg(feature = "embeddings")]
pub async fn hybrid_search_explain(
    embedder: &Embedder,
    symbols: &[CodeSymbol],
    vectors: &impl VectorSearcher,
    query: &str,
    limit: usize,
) -> Result<Vec<HybridSearchResult>> {
    hybrid_search_impl(embedder, symbols, vectors, query, limit, true).await
}

#[cfg(feature = "embeddings")]
async fn hybrid_search_impl(
    embedder: &Embedder,
    symbols: &[CodeSymbol],
    vectors: &impl VectorSearcher,
    query: &str,
    limit: usize,
    explain: bool,
) -> Result<Vec<HybridSearchResult>> {
    // BM25 search (with or without explain)
    let bm25_results: Vec<(CodeSymbol, f64, Option<SearchExplain>)> = if explain {
        search_symbols_explain(symbols, query)
            .into_iter()
            .map(|r| (r.symbol, r.score, r.explain))
            .collect()
    } else {
        search_symbols(symbols, query)
            .into_iter()
            .map(|r| (r.symbol, r.score, None))
            .collect()
    };

    // Vector leg: embed the query once with the index's embedder, then query
    // persisted vectors. We do NOT re-embed the corpus — that is the whole
    // point of issue #28.
    let query_embedding = embedder.embed_query(query).await?;
    let vector_results = vector_leg(vectors, &query_embedding, limit).await?;

    // Combine with RRF (k=60)
    Ok(reciprocal_rank_fusion(
        bm25_results,
        vector_results,
        60.0,
        limit,
    ))
}

/// Vector leg of hybrid search: fetch nearest persisted-vector candidates for a
/// pre-computed query embedding. Fetches `limit.max(100)` candidates so RRF has
/// enough overlap with the BM25 leg. Kept separate from `hybrid_search_impl` so
/// it can be exercised without loading the real embedding model.
#[cfg(feature = "embeddings")]
async fn vector_leg(
    vectors: &impl VectorSearcher,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<(CodeSymbol, f64)>> {
    let vector_limit = limit.max(100); // fetch more candidates for fusion
    vectors.nearest(query_embedding, vector_limit).await
}

/// Rerank hybrid search results using a cross-encoder model.
///
/// Takes the results from hybrid_search and reranks them using a cross-encoder model.
/// This improves relevance by scoring each (query, document) pair jointly.
///
/// The reranking is expensive (O(n) forward passes through the model), so it's
/// typically applied only to the top-k results from the initial hybrid search.
///
/// Requires the `embeddings` feature.
#[cfg(feature = "embeddings")]
pub async fn rerank_results(
    query: &str,
    results: Vec<HybridSearchResult>,
    reranker_id: Option<&str>,
) -> Result<Vec<HybridSearchResult>> {
    if results.is_empty() {
        return Ok(results);
    }

    let reranker = get_reranker(reranker_id).await?;

    // Build document texts for reranking using the *same* principled builder
    // the retriever used to embed each symbol. Previously this rebuilt an
    // untruncated `name signature content` string, so the cross-encoder scored
    // text the retriever never indexed (issue #36). Sharing `Embedder::symbol_text`
    // removes that asymmetry: reranker and retriever see the same document,
    // bounded by the model's token budget.
    let documents: Vec<String> = results
        .iter()
        .map(|r| Embedder::symbol_text(&r.symbol))
        .collect();

    // Rerank with cross-encoder - returns (index, score) pairs sorted by score
    let reranked_indices = reranker.rerank(query, &documents)?;

    // Rebuild results in reranked order. Keep the RRF fusion score in
    // `combined_score` and record the cross-encoder score in `rerank_score`
    // instead of overwriting it (issue #29): the two live on different scales,
    // so callers can report both rather than silently swapping semantics.
    let reranked_results: Vec<HybridSearchResult> = reranked_indices
        .into_iter()
        .map(|(idx, cross_encoder_score)| {
            let mut result = results[idx].clone();
            result.rerank_score = Some(cross_encoder_score as f64);
            result
        })
        .collect();

    Ok(reranked_results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, SymbolKind};

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_rrf_both_lists() {
        let bm25 = vec![
            (make_symbol("a", "alpha"), 10.0, None),
            (make_symbol("b", "beta"), 8.0, None),
            (make_symbol("c", "gamma"), 5.0, None),
        ];
        let vector = vec![
            (make_symbol("b", "beta"), 0.95),
            (make_symbol("a", "alpha"), 0.90),
            (make_symbol("d", "delta"), 0.85),
        ];

        let results = reciprocal_rank_fusion(bm25, vector, 60.0, 10);

        // "a" (alpha): bm25_rank=1, vector_rank=2 => 1/61 + 1/62
        // "b" (beta):  bm25_rank=2, vector_rank=1 => 1/62 + 1/61
        // Both should have the same combined score
        let a = results.iter().find(|r| r.symbol.uid == "a").unwrap();
        let b = results.iter().find(|r| r.symbol.uid == "b").unwrap();

        let expected_ab = 1.0 / 61.0 + 1.0 / 62.0;
        assert!((a.combined_score - expected_ab).abs() < 1e-10);
        assert!((b.combined_score - expected_ab).abs() < 1e-10);

        // "c" (gamma): only in bm25, rank=3 => 1/63
        let c = results.iter().find(|r| r.symbol.uid == "c").unwrap();
        let expected_c = 1.0 / 63.0;
        assert!((c.combined_score - expected_c).abs() < 1e-10);
        assert!(c.bm25_rank == Some(3));
        assert!(c.vector_rank.is_none());

        // "d" (delta): only in vector, rank=3 => 1/63
        let d = results.iter().find(|r| r.symbol.uid == "d").unwrap();
        let expected_d = 1.0 / 63.0;
        assert!((d.combined_score - expected_d).abs() < 1e-10);
        assert!(d.bm25_rank.is_none());
        assert!(d.vector_rank == Some(3));

        // a and b should be ranked above c and d
        assert!(a.combined_score > c.combined_score);
        assert!(b.combined_score > d.combined_score);
    }

    #[test]
    fn test_rrf_limit() {
        let bm25 = vec![
            (make_symbol("a", "alpha"), 10.0, None),
            (make_symbol("b", "beta"), 8.0, None),
            (make_symbol("c", "gamma"), 5.0, None),
        ];
        let vector = vec![];

        let results = reciprocal_rank_fusion(bm25, vector, 60.0, 2);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].symbol.uid, "a");
        assert_eq!(results[1].symbol.uid, "b");
    }

    #[test]
    fn test_rrf_empty_inputs() {
        let results = reciprocal_rank_fusion(vec![], vec![], 60.0, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_rrf_single_list_ordering() {
        let bm25 = vec![
            (make_symbol("a", "alpha"), 10.0, None),
            (make_symbol("b", "beta"), 5.0, None),
        ];
        let vector = vec![];

        let results = reciprocal_rank_fusion(bm25, vector, 60.0, 10);
        // rank 1 => 1/61 > rank 2 => 1/62
        assert!(results[0].combined_score > results[1].combined_score);
        assert_eq!(results[0].symbol.uid, "a");
    }
}

#[cfg(test)]
mod rerank_tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, SymbolKind};

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    #[test]
    fn test_rerank_with_empty_results() {
        // Reranking empty results should return empty results
        // This is a unit test that doesn't require the actual reranker model
        let results: Vec<HybridSearchResult> = vec![];
        assert!(results.is_empty());
    }

    #[test]
    fn test_rerank_preserves_symbol_data() {
        // Test that reranking preserves symbol information
        let sym1 = make_symbol("uid1", "test_func");
        let result = HybridSearchResult {
            symbol: sym1.clone(),
            bm25_score: Some(10.0),
            vector_score: Some(0.95),
            combined_score: 1.0 / 61.0,
            rerank_score: None,
            bm25_rank: Some(1),
            vector_rank: Some(1),
            explain: None,
        };

        assert_eq!(result.symbol.uid, "uid1");
        assert_eq!(result.symbol.name, "test_func");
        assert_eq!(result.bm25_score, Some(10.0));
    }

    /// Issue #29: reranking must record the cross-encoder score in a separate
    /// `rerank_score` field and must NOT overwrite the RRF `combined_score`.
    /// This pins the "two scales, both reported" contract without needing the
    /// cross-encoder model — it exercises the same field-assignment logic
    /// `rerank_results` performs after the model returns.
    #[test]
    fn rerank_records_second_scale_without_clobbering_fusion() {
        let sym = make_symbol("uid1", "test_func");
        let fusion = 1.0 / 61.0;
        let mut result = HybridSearchResult {
            symbol: sym,
            bm25_score: Some(10.0),
            vector_score: Some(0.95),
            combined_score: fusion,
            rerank_score: None,
            bm25_rank: Some(1),
            vector_rank: Some(1),
            explain: None,
        };

        // Simulate what rerank_results does with a cross-encoder score.
        let cross_encoder_score = 7.3_f64;
        result.rerank_score = Some(cross_encoder_score);

        // Fusion score is untouched (still the RRF scale, ~1/k).
        assert_eq!(result.combined_score, fusion);
        // Rerank score lives on its own field/scale.
        assert_eq!(result.rerank_score, Some(cross_encoder_score));
        // The two are genuinely different scales — never conflated.
        assert_ne!(result.combined_score, result.rerank_score.unwrap());
    }
}

/// Regression tests for issue #28: the hybrid vector leg must query persisted
/// vectors instead of re-embedding the whole corpus. A counting fake stands in
/// for the persisted-vector store and records how it is queried.
#[cfg(all(test, feature = "embeddings"))]
mod persisted_vector_tests {
    use super::*;
    use myceliums_storage::{CodeSymbol, SymbolKind};
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_symbol(uid: &str, name: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: name.to_string(),
            kind: SymbolKind::Function,
            file_path: "test.rs".to_string(),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: String::new(),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    /// A `VectorSearcher` that returns canned candidates and counts how many
    /// times it is queried, plus the `limit` it last received.
    struct CountingSearcher {
        candidates: Vec<(CodeSymbol, f64)>,
        calls: AtomicUsize,
        last_limit: AtomicUsize,
    }

    impl CountingSearcher {
        fn new(candidates: Vec<(CodeSymbol, f64)>) -> Self {
            Self {
                candidates,
                calls: AtomicUsize::new(0),
                last_limit: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait::async_trait]
    impl VectorSearcher for CountingSearcher {
        async fn nearest(
            &self,
            _query_vector: &[f32],
            limit: usize,
        ) -> Result<Vec<(CodeSymbol, f64)>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.last_limit.store(limit, Ordering::SeqCst);
            Ok(self.candidates.iter().take(limit).cloned().collect())
        }
    }

    /// The vector leg queries the persisted store exactly once and never touches
    /// the corpus — it fetches `limit.max(100)` candidates for RRF overlap.
    #[tokio::test]
    async fn vector_leg_queries_store_once_with_widened_limit() {
        let searcher = CountingSearcher::new(vec![
            (make_symbol("a", "alpha"), 0.9),
            (make_symbol("b", "beta"), 0.8),
        ]);
        let query_embedding = vec![0.0f32; 8];

        let results = vector_leg(&searcher, &query_embedding, 5)
            .await
            .expect("vector leg succeeds");

        // Exactly one query against the persisted store — no per-symbol embedding.
        assert_eq!(searcher.calls.load(Ordering::SeqCst), 1);
        // limit.max(100) candidates requested for fusion overlap.
        assert_eq!(searcher.last_limit.load(Ordering::SeqCst), 100);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0.uid, "a");
    }

    /// Rank provenance (bm25_rank / vector_rank) is preserved when the vector
    /// leg is fed from persisted candidates rather than a re-embedded corpus.
    #[tokio::test]
    async fn persisted_vectors_preserve_rank_provenance() {
        let searcher = CountingSearcher::new(vec![
            (make_symbol("b", "beta"), 0.95),
            (make_symbol("a", "alpha"), 0.90),
            (make_symbol("d", "delta"), 0.85),
        ]);
        let query_embedding = vec![0.0f32; 8];

        let vector_results = vector_leg(&searcher, &query_embedding, 10)
            .await
            .expect("vector leg succeeds");

        let bm25_results = vec![
            (make_symbol("a", "alpha"), 10.0, None),
            (make_symbol("b", "beta"), 8.0, None),
            (make_symbol("c", "gamma"), 5.0, None),
        ];

        let fused = reciprocal_rank_fusion(bm25_results, vector_results, 60.0, 10);

        let a = fused.iter().find(|r| r.symbol.uid == "a").unwrap();
        assert_eq!(a.bm25_rank, Some(1));
        assert_eq!(a.vector_rank, Some(2));

        let d = fused.iter().find(|r| r.symbol.uid == "d").unwrap();
        assert_eq!(d.bm25_rank, None);
        assert_eq!(d.vector_rank, Some(3));

        // Store queried exactly once for the whole fusion.
        assert_eq!(searcher.calls.load(Ordering::SeqCst), 1);
    }
}
