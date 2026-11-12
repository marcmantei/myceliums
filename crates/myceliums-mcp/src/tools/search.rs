//! Text, semantic, hybrid, and Cypher search over the graph.

use super::prelude::*;

#[tool_router(router = search_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "context_search",
        description = "Search for functions, classes, and symbols in the knowledge graph. Preferred over grep for finding code entities — returns structured results with file locations, types, and relevance scores."
    )]
    pub(crate) async fn context_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self
            .service
            .resolve_repo_id(params.repo_id.as_deref())
            .map_err(service_error)?;
        let explain = params.explain.unwrap_or(false);
        let results = self
            .service
            .search_context(&repo_id, &params.query, explain)
            .await
            .map_err(service_error)?;

        let limit = params.limit.unwrap_or(20);
        let items: Vec<SearchResultItem> = results
            .into_iter()
            .take(limit)
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
        Ok(Json(TextOutput {
            text: format::format_search_results(&params.query, &items),
        }))
    }

    #[tool(
        name = "search_documents",
        description = "Search through all analyzed code content using BM25 text search"
    )]
    async fn search_documents(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        self.context_search(Parameters(params)).await
    }

    #[tool(
        name = "hybrid_search",
        description = "Search using hybrid BM25 + vector semantic search with Reciprocal Rank Fusion for better search quality"
    )]
    pub(crate) async fn hybrid_search(
        &self,
        Parameters(params): Parameters<HybridSearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let store = self.store(&repo_id).await?;

        let symbols = self.symbols(&repo_id).await?;

        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to load embedding model: {}", e),
                    None,
                )
            })?;

        let explain = params.explain.unwrap_or(false);
        let limit = params.limit.unwrap_or(20);
        let mut results = if explain {
            core_hybrid_search_explain(&embedder, &symbols, store.as_ref(), &params.query, limit)
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
        } else {
            core_hybrid_search(&embedder, &symbols, store.as_ref(), &params.query, limit)
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?
        };

        // Apply reranking if requested, using the reranker recorded at indexing time
        if params.rerank.unwrap_or(false) {
            let reranker_id = embedder.meta().reranker.clone();
            results = rerank_results(&params.query, results, reranker_id.as_deref())
                .await
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        }

        // Attach graph edges for explain mode
        if explain {
            let relationships = self.relationships(&repo_id).await?;
            let uid_to_name: std::collections::HashMap<&str, &str> = symbols
                .iter()
                .map(|s| (s.uid.as_str(), s.name.as_str()))
                .collect();
            attach_graph_edges(&mut results, &relationships, &uid_to_name);
        }

        let items: Vec<HybridSearchResultItem> = results
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
        let warning = partial_index_warning(&store).await;
        Ok(Json(TextOutput {
            text: format::with_index_warning(
                warning,
                format::format_hybrid_results(&params.query, &items),
            ),
        }))
    }

    #[tool(
        name = "cypher_query",
        description = "Execute a Cypher query against the knowledge graph. Supports MATCH, RETURN, WHERE, ORDER BY, LIMIT, SKIP, CONTAINS, IS NULL. Blocks write operations."
    )]
    pub(crate) async fn cypher_query(
        &self,
        Parameters(params): Parameters<CypherQueryParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let symbols = self.symbols(&params.repo_id).await?;
        let files = self.files(&params.repo_id).await?;
        let relationships = self.relationships(&params.repo_id).await?;

        let executor = myceliums_cypher::CypherExecutor::from_shared(
            symbols.shared(),
            files.shared(),
            relationships.shared(),
        );
        let results = executor
            .execute(&params.query)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let json_results = serde_json::to_value(&results)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_cypher_results(&params.query, &json_results),
        }))
    }

    #[tool(
        name = "semantic_search",
        description = "Search for symbols using semantic similarity (vector embeddings). Returns symbols most similar in meaning to the query."
    )]
    pub(crate) async fn semantic_search(
        &self,
        Parameters(params): Parameters<SearchParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let repo_id = self.resolve_repo_id(params.repo_id.as_deref())?;
        let store = self.store(&repo_id).await?;
        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Failed to load embedding model: {}", e),
                    None,
                )
            })?;
        let query_vector = embedder.embed_query(&params.query).await.map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to embed query: {}", e), None)
        })?;
        let limit = params.limit.unwrap_or(10);
        let results = store
            .vector_search(&query_vector, limit)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let items: Vec<SearchResultItem> = results
            .into_iter()
            .map(|(sym, score)| SearchResultItem {
                name: sym.name,
                qualified_name: sym.qualified_name,
                kind: sym.kind.to_string(),
                file_path: sym.file_path,
                start_line: sym.start_line,
                end_line: sym.end_line,
                signature: sym.signature,
                score: score as f64,
                explain: None,
            })
            .collect();
        let warning = partial_index_warning(&store).await;
        Ok(Json(TextOutput {
            text: format::with_index_warning(
                warning,
                format::format_search_results(&params.query, &items),
            ),
        }))
    }
}
