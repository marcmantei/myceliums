//! Code search (keyword, semantic, hybrid) and Cypher queries over the graph.
//!
//! `search_code` is the one search tool: its `mode` picks the ranking. The
//! older per-mode tools (`context_search`, `semantic_search`,
//! `hybrid_search`, and `search_documents`, which was `context_search` under
//! another name) are deprecated aliases over the same implementation.

use super::prelude::*;
use myceliums_core::{Embedder, HybridParams, TestPolicy};
use std::sync::Arc;

/// Most results a search tool returns per page.
const SEARCH: Budget = Budget::new(20, 100);
/// Hybrid search pages are cut from at most this many top results. Up to
/// 100 the vector leg's candidate pool is fixed, so every page comes from the
/// same fused ranking.
const HYBRID_WINDOW: usize = 100;
/// With `rerank`, this many top hybrid results are reranked, whatever the
/// page: reranking a window that grew with `offset` would reorder it and make
/// pages overlap.
const RERANK_WINDOW: usize = 50;

/// Said when the index predates the test/product classification (#146) and
/// the caller asked for test code to be reordered: nothing is reordered.
fn unclassified_notice(graph: &Graph<'_>, tests: &TestPolicy) -> Option<&'static str> {
    let stale = graph.info().classification_version
        < myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION;
    (stale && !tests.is_neutral()).then_some(
        "Note: this index was built before test code was classified, so tests rank like \
         product code. Re-run analyze to rank them below it.",
    )
}

/// With `explain`, the line naming the rows of a page that `tests` moved down
/// (#146), if any.
fn demoted_rows<'a>(
    explain: bool,
    tests: &TestPolicy,
    page: impl Iterator<Item = &'a myceliums_storage::CodeSymbol>,
) -> Option<String> {
    if !explain {
        return None;
    }
    let rows: Vec<String> = page
        .enumerate()
        .filter(|(_, symbol)| tests.demotes(symbol.is_test()))
        .map(|(i, _)| (i + 1).to_string())
        .collect();
    let class = match tests.ranking {
        myceliums_core::TestRanking::Prefer => "product code",
        _ => "test code",
    };
    (!rows.is_empty()).then(|| {
        format!(
            "Demoted as {class} (tests={}, a symbol at position p placed at p / {}): rows {}.",
            tests.ranking.as_str(),
            tests.weight,
            rows.join(", ")
        )
    })
}

/// The embedding model recorded for an index could not be loaded: that is
/// fixable (keyword search still works; re-indexing picks a usable model).
fn embedder_error(e: impl std::fmt::Display) -> ToolError {
    ToolError::recoverable(
        format!("Cannot load the embedding model for this index: {e}."),
        "call search_code with mode=keyword (needs no model), or re-run analyze",
    )
}

/// Whether the index holds embedding vectors, read from the metadata
/// recorded at index time (no model load, no vector scan). An index built
/// with `skip_embeddings` has none; so has a legacy index without metadata,
/// as far as `auto` is concerned (it never loads a model it may not have).
async fn has_vectors(store: &Store) -> bool {
    let meta = store
        .get_index_meta(myceliums_core::IndexEmbeddingMeta::META_KEY)
        .await
        .ok()
        .flatten();
    if meta.is_none() {
        return false;
    }
    match myceliums_core::EmbeddingStats::load(store).await {
        Ok(Some(stats)) => stats.symbols_embedded > 0,
        _ => true,
    }
}

/// The embedder of an index that holds vectors. An index without any is a
/// recoverable error: no model is loaded (or downloaded) for an index that
/// could not use it.
async fn vector_embedder(store: &Store) -> Result<Arc<Embedder>, ToolError> {
    if !has_vectors(store).await && crate::embedding_jobs::is_running(store.repo_id()) {
        return Err(ToolError::recoverable(
            "This index's embeddings are being built in the background and no symbol has a vector yet.",
            "call search_code with mode=keyword now; search by meaning works once the first symbols are embedded (the Index line shows progress)",
        ));
    }
    if !has_vectors(store).await {
        return Err(ToolError::recoverable(
            "This index has no embeddings (built with skip_embeddings, or before embedding metadata was recorded), so it cannot be searched by meaning.",
            "call search_code with mode=keyword, or re-run analyze without skip_embeddings",
        ));
    }
    myceliums_core::embedder_for_index(store)
        .await
        .map_err(embedder_error)
}

#[tool_router(router = search_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "search_code",
        description = "Find where something is implemented: functions, methods, classes and types by name (parseConfig, Item.AsString) or by what the code does (\"builds the Set-Cookie header\"). Use it instead of grep to locate code; results give kind, file and line range. mode=auto (default) ranks by keyword and meaning when the index has embeddings. Test code ranks below product code unless tests=include or tests=prefer.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn search_code(
        &self,
        Parameters(params): Parameters<SearchCodeParams>,
    ) -> TextResult {
        retry_on_index_change(|| self.search_code_once(params.clone())).await
    }

    #[tool(
        name = "context_search",
        description = "Search for functions, classes, and symbols in the knowledge graph by name and signature (keyword ranking).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn context_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> TextResult {
        require_query("query", &params.query)?;
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let page = Page::new(params.limit, params.offset, SEARCH);
        let body = self
            .keyword_results(
                &graph,
                &params.query,
                page,
                params.explain.unwrap_or(false),
                TestPolicy::default(),
            )
            .await?;
        self.graph_text(&graph, body).await
    }

    #[tool(
        name = "search_documents",
        description = "Same as context_search (keyword ranking over names and signatures); it never was a separate BM25 document search.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_documents(&self, Parameters(params): Parameters<SearchParams>) -> TextResult {
        self.context_search(Parameters(params)).await
    }

    #[tool(
        name = "hybrid_search",
        description = "Search using hybrid BM25 + vector semantic search with Reciprocal Rank Fusion for better search quality",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn hybrid_search(
        &self,
        Parameters(params): Parameters<HybridSearchParams>,
    ) -> TextResult {
        require_query("query", &params.query)?;
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let embedder = vector_embedder(&graph.store()).await?;
        let page = Page::new(params.limit, params.offset, SEARCH);
        let body = self
            .hybrid_results(
                &graph,
                &embedder,
                &params.query,
                page,
                params.rerank.unwrap_or(false),
                params.explain.unwrap_or(false),
                TestPolicy::default(),
            )
            .await?;
        self.graph_text(&graph, body).await
    }

    #[tool(
        name = "cypher_query",
        description = "Execute a read-only Cypher query against the knowledge graph. Supports MATCH, RETURN, WHERE, ORDER BY, LIMIT, SKIP, CONTAINS, IS NULL; write clauses are rejected. Rows are paged (limit/offset).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn cypher_query(
        &self,
        Parameters(params): Parameters<CypherQueryParams>,
    ) -> TextResult {
        require_query("query", &params.query)?;
        let graph = self.graph(&params.repo_id).await?;
        let symbols = graph.symbols().await?;
        let files = graph.files().await?;
        let relationships = graph.relationships().await?;

        let executor = myceliums_cypher::CypherExecutor::from_shared(
            symbols.shared(),
            files.shared(),
            relationships.shared(),
        );
        // Every executor error is about the query (syntax, unsupported
        // clause, unknown label, result too large): bad input.
        let results = executor.execute(&params.query).map_err(|e| {
            ToolError::invalid(format!(
                "Cypher query rejected: {e}. Supported: MATCH … WHERE … RETURN … ORDER BY/SKIP/LIMIT (read-only)."
            ))
        })?;
        let rows: Vec<serde_json::Value> = results
            .iter()
            .map(serde_json::to_value)
            .collect::<Result<_, _>>()
            .map_err(|e| ToolError::internal("cannot serialize Cypher rows", e))?;
        let page = Page::new(params.limit, params.offset, Budget::ROWS).take(rows);
        let table = format::format_cypher_results(&page.items);
        let body = with_notice(
            format!(
                "Cypher: {}\n\n{}\n",
                params.query,
                untrusted("query results", &table)
            ),
            page.notice
                .as_deref()
                .map(|n| format!("{n} Or narrow the query with WHERE/LIMIT."))
                .as_deref(),
        );
        self.graph_text(&graph, body).await
    }

    #[tool(
        name = "semantic_search",
        description = "Search for symbols using semantic similarity (vector embeddings). Returns symbols most similar in meaning to the query.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn semantic_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> TextResult {
        require_query("query", &params.query)?;
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let store = graph.store();
        let embedder = vector_embedder(&store).await?;
        let page = Page::new(params.limit, params.offset, Budget::new(10, SEARCH.max));
        let body = self
            .semantic_results(
                &graph,
                &embedder,
                &params.query,
                page,
                TestPolicy::default(),
            )
            .await?;
        self.graph_text(&graph, body).await
    }
}

/// The search rankings behind `search_code` and its deprecated aliases. Each
/// returns one framed page of results, without the freshness line.
impl MyceliumsMcp {
    /// Keyword ranking over names, qualified names and signatures, test
    /// code placed by `tests`.
    async fn keyword_results(
        &self,
        graph: &Graph<'_>,
        query: &str,
        page: Page,
        explain: bool,
        tests: TestPolicy,
    ) -> Result<String, ToolError> {
        let symbols = graph.symbols().await?;
        let results = myceliums_core::search_symbols_ranked(&symbols, query, explain, &tests);
        let page = page.take(results);
        let demoted = demoted_rows(explain, &tests, page.items.iter().map(|r| &r.symbol));
        let items: Vec<SearchResultItem> = page
            .items
            .into_iter()
            .map(|r| SearchResultItem {
                name: r.symbol.name,
                qualified_name: r.symbol.qualified_name,
                kind: r.symbol.kind.to_string(),
                file_path: r.symbol.file_path,
                start_line: r.symbol.start_line,
                end_line: r.symbol.end_line,
                signature: r.symbol.signature,
                score: r.score,
                explain: r.explain.and_then(|e| serde_json::to_value(e).ok()),
            })
            .collect();
        Ok(with_notice(
            with_notice(
                format::framed_results(query, &items, format::format_search_results),
                demoted.as_deref(),
            ),
            page.notice.as_deref(),
        ))
    }

    /// Vector similarity only, test code placed by `tests`.
    async fn semantic_results(
        &self,
        graph: &Graph<'_>,
        embedder: &Arc<Embedder>,
        query: &str,
        page: Page,
        tests: TestPolicy,
    ) -> Result<String, ToolError> {
        let query_vector = embedder
            .embed_query(query)
            .await
            .map_err(|e| ToolError::internal("cannot embed the query", e))?;
        let store = &graph.store();
        let results =
            myceliums_core::semantic_search(store.as_ref(), &query_vector, page.fetch(), &tests)
                .await
                .map_err(|e| ToolError::internal("vector search failed", e))?;
        // Every store read before `verify()`: one after it is not covered.
        let warning = partial_index_warning(store).await;
        graph.verify()?;
        let page = page.take_open(results);
        let items: Vec<SearchResultItem> = page
            .items
            .into_iter()
            .map(|(sym, score)| SearchResultItem {
                name: sym.name,
                qualified_name: sym.qualified_name,
                kind: sym.kind.to_string(),
                file_path: sym.file_path,
                start_line: sym.start_line,
                end_line: sym.end_line,
                signature: sym.signature,
                score,
                explain: None,
            })
            .collect();
        Ok(format::with_index_warning(
            warning,
            with_notice(
                format::framed_results(query, &items, format::format_search_results),
                page.notice.as_deref(),
            ),
        ))
    }

    /// Keyword and vector rankings fused (Reciprocal Rank Fusion), optionally
    /// reranked by a cross-encoder; test code placed by `tests` in both.
    #[allow(clippy::too_many_arguments)]
    async fn hybrid_results(
        &self,
        graph: &Graph<'_>,
        embedder: &Arc<Embedder>,
        query: &str,
        page: Page,
        rerank: bool,
        explain: bool,
        tests: TestPolicy,
    ) -> Result<String, ToolError> {
        // `embedder` was chosen by a store read: pin the snapshot to that
        // generation before the first table read could move it (see
        // `RepoGraph::verify`).
        graph.verify()?;
        let store = &graph.store();
        let symbols = graph.symbols().await?;
        // Ranked top-k: ask for one past the page to learn whether more
        // exist, within a window that does not depend on the page.
        let fetch = if rerank {
            RERANK_WINDOW
        } else {
            page.fetch().min(HYBRID_WINDOW)
        };
        let params = HybridParams::with_tests(tests);
        let mut results = if explain {
            core_hybrid_search_explain(embedder, &symbols, store.as_ref(), query, fetch, &params)
                .await
                .map_err(|e| ToolError::internal("hybrid search failed", e))?
        } else {
            myceliums_core::hybrid_search_with(
                embedder,
                &symbols,
                store.as_ref(),
                query,
                fetch,
                &params,
            )
            .await
            .map_err(|e| ToolError::internal("hybrid search failed", e))?
        };
        // The vector half read the store directly; so does the warning.
        let warning = partial_index_warning(store).await;
        graph.verify()?;

        // Apply reranking if requested, using the reranker recorded at indexing time
        if rerank {
            let reranker_id = embedder.meta().reranker.clone();
            results = rerank_results(query, results, reranker_id.as_deref(), &tests)
                .await
                .map_err(|e| ToolError::internal("reranking failed", e))?;
        }

        // Attach graph edges for explain mode
        if explain {
            let relationships = graph.relationships().await?;
            let uid_to_name: std::collections::HashMap<&str, &str> = symbols
                .iter()
                .map(|s| (s.uid.as_str(), s.name.as_str()))
                .collect();
            attach_graph_edges(&mut results, &relationships, &uid_to_name);
        }

        let page = page.take_open(results);
        let demoted = demoted_rows(explain, &tests, page.items.iter().map(|r| &r.symbol));
        let items: Vec<HybridSearchResultItem> = page
            .items
            .into_iter()
            .map(|r| HybridSearchResultItem {
                name: r.symbol.name,
                qualified_name: r.symbol.qualified_name,
                kind: r.symbol.kind.to_string(),
                file_path: r.symbol.file_path,
                start_line: r.symbol.start_line,
                end_line: r.symbol.end_line,
                signature: r.symbol.signature,
                combined_score: r.combined_score,
                rerank_score: r.rerank_score,
                bm25_rank: r.bm25_rank,
                vector_rank: r.vector_rank,
                bm25_score: r.bm25_score,
                vector_score: r.vector_score,
                explain: r.explain.and_then(|e| serde_json::to_value(e).ok()),
            })
            .collect();
        Ok(format::with_index_warning(
            warning,
            with_notice(
                with_notice(
                    format::framed_results(query, &items, format::format_hybrid_results),
                    demoted.as_deref(),
                ),
                page.notice.as_deref(),
            ),
        ))
    }
}

impl MyceliumsMcp {
    /// One run of `search_code` (see [`retry_on_index_change`]).
    async fn search_code_once(&self, params: SearchCodeParams) -> TextResult {
        require_query("query", &params.query)?;
        let graph = self.graph_for(params.repo_id.as_deref()).await?;
        let page = Page::new(params.limit, params.offset, SEARCH);
        let explain = params.explain.unwrap_or(false);
        let rerank = params.rerank.unwrap_or(false);
        let tests = TestPolicy::new(params.tests.unwrap_or_default().into());
        let query = params.query.as_str();
        let body = match params.mode.unwrap_or_default() {
            SearchMode::Keyword => {
                self.keyword_results(&graph, query, page, explain, tests)
                    .await?
            }
            SearchMode::Semantic => {
                let store = graph.store();
                let embedder = vector_embedder(&store).await?;
                self.semantic_results(&graph, &embedder, query, page, tests)
                    .await?
            }
            SearchMode::Hybrid => {
                let embedder = vector_embedder(&graph.store()).await?;
                self.hybrid_results(&graph, &embedder, query, page, rerank, explain, tests)
                    .await?
            }
            SearchMode::Auto => {
                let store = graph.store();
                let embedder = if has_vectors(&store).await {
                    myceliums_core::embedder_for_index(&store).await.ok()
                } else {
                    None
                };
                match embedder {
                    Some(embedder) => {
                        self.hybrid_results(&graph, &embedder, query, page, rerank, explain, tests)
                            .await?
                    }
                    None if crate::embedding_jobs::is_running(&graph.info().id) => format!(
                        "Keyword search (embeddings are being built in the background; search \
                         by meaning starts once the first symbols are embedded).\n\n{}",
                        self.keyword_results(&graph, query, page, explain, tests)
                            .await?
                    ),
                    None => format!(
                        "Keyword search (this index has no usable embeddings; re-run analyze \
                         without skip_embeddings for search by meaning).\n\n{}",
                        self.keyword_results(&graph, query, page, explain, tests)
                            .await?
                    ),
                }
            }
        };
        let body = with_notice(body, unclassified_notice(&graph, &tests));
        self.graph_text(&graph, body).await
    }
}
