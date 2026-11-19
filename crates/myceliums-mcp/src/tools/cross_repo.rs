//! Intent isolation and cross-repository comparison/adaptation.

use super::prelude::*;

/// The embedding model of an index could not be loaded.
fn embedder_error(e: impl std::fmt::Display) -> ToolError {
    ToolError::recoverable(
        format!("Cannot load the embedding model for this index: {e}."),
        "re-run analyze on that repository without skip_embeddings",
    )
}

impl MyceliumsMcp {
    /// A result over two repositories: one freshness line for each.
    async fn two_repo_text(
        &self,
        source: &Graph<'_>,
        target: &Graph<'_>,
        body: String,
    ) -> TextResult {
        let source_status = self.index_status(source).await?;
        let target_status = self.index_status(target).await?;
        text(format!(
            "Source {source_status}\nTarget {target_status}\n\n{body}"
        ))
    }
}

#[tool_router(router = cross_repo_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "isolate_intent",
        description = "Isolate the symbols implementing a specific intent/feature in a repository. Uses hybrid search to find seed symbols, then expands via call graph traversal with community-aware pruning. Returns the IntentSlice: seed symbols, expanded symbols, internal relationships, and structural metadata.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn isolate_intent(
        &self,
        Parameters(params): Parameters<IsolateIntentParams>,
    ) -> TextResult {
        use myceliums_core::cross_repo::{self, IsolateConfig};

        let graph = self.graph(&params.repo_id).await?;
        let store = graph.store();

        let registry = registry()?;
        let repo_name = registry
            .get(&params.repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.repo_id.clone());

        let symbols = graph.symbols().await?;
        let relationships = graph.relationships().await?;
        let communities = graph.communities().await?;

        let config = IsolateConfig {
            max_symbols: params.max_symbols.unwrap_or(50).clamp(1, 200),
            expansion_depth: params.depth.unwrap_or(2).clamp(1, 5),
        };

        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(embedder_error)?;
        let slice = cross_repo::isolate_intent_hybrid(
            &embedder,
            &params.intent,
            &params.repo_id,
            &repo_name,
            &symbols,
            store.as_ref(),
            &relationships,
            &communities,
            &config,
        )
        .await
        .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;
        graph.verify()?;

        self.graph_text(&graph, format::format_intent_slice(&slice))
            .await
    }

    #[tool(
        name = "differentiate_intent",
        description = "Compare how two repositories implement the same intent/feature. Isolates the relevant symbols in each repo, aligns them via embedding similarity, and reports structural differences. Returns: matched symbol pairs, unmatched symbols, and structural comparison metrics.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn differentiate_intent(
        &self,
        Parameters(params): Parameters<DifferentiateIntentParams>,
    ) -> TextResult {
        use myceliums_core::cross_repo::{self, DifferentiateConfig, IsolateConfig};

        let registry = registry()?;

        let max_symbols = params.max_symbols.unwrap_or(50).clamp(1, 200);
        let config = IsolateConfig {
            max_symbols,
            expansion_depth: 2,
        };

        // Isolate intent in source repo
        let src_graph = self.graph(&params.source_repo_id).await?;
        let src_store = src_graph.store();
        let src_name = registry
            .get(&params.source_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.source_repo_id.clone());
        let src_symbols = src_graph.symbols().await?;
        let src_rels = src_graph.relationships().await?;
        let src_communities = src_graph.communities().await?;

        let src_embedder = myceliums_core::embedder_for_index(&src_store)
            .await
            .map_err(embedder_error)?;
        let source_slice = cross_repo::isolate_intent_hybrid(
            &src_embedder,
            &params.intent,
            &params.source_repo_id,
            &src_name,
            &src_symbols,
            src_store.as_ref(),
            &src_rels,
            &src_communities,
            &config,
        )
        .await
        .map_err(|e| ToolError::internal("source isolation failed", e))?;
        src_graph.verify()?;

        // Isolate intent in target repo
        let tgt_graph = self.graph(&params.target_repo_id).await?;
        let tgt_store = tgt_graph.store();
        let tgt_name = registry
            .get(&params.target_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.target_repo_id.clone());
        let tgt_symbols = tgt_graph.symbols().await?;
        let tgt_rels = tgt_graph.relationships().await?;
        let tgt_communities = tgt_graph.communities().await?;

        let tgt_embedder = myceliums_core::embedder_for_index(&tgt_store)
            .await
            .map_err(embedder_error)?;
        let target_slice = cross_repo::isolate_intent_hybrid(
            &tgt_embedder,
            &params.intent,
            &params.target_repo_id,
            &tgt_name,
            &tgt_symbols,
            tgt_store.as_ref(),
            &tgt_rels,
            &tgt_communities,
            &config,
        )
        .await
        .map_err(|e| ToolError::internal("target isolation failed", e))?;
        tgt_graph.verify()?;

        // Get vectors for differentiation
        let src_vectors = src_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;
        let tgt_vectors = tgt_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;

        let diff_config = DifferentiateConfig {
            similarity_threshold: params.similarity_threshold.unwrap_or(0.65).clamp(0.0, 1.0),
        };

        let report = cross_repo::differentiate_with_vectors(
            &source_slice,
            &target_slice,
            &src_vectors,
            &tgt_vectors,
            &diff_config,
        );

        self.two_repo_text(
            &src_graph,
            &tgt_graph,
            format::format_differentiation_report(&report),
        )
        .await
    }

    #[tool(
        name = "plan_adaptation",
        description = "Generate an actionable adaptation plan for migrating one repository's approach to another. Compares intent implementations, then produces ordered steps with dependency tracking, effort estimates, and risk analysis.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn plan_adaptation(
        &self,
        Parameters(params): Parameters<PlanAdaptationParams>,
    ) -> TextResult {
        use myceliums_core::cross_repo::{self, DifferentiateConfig, IsolateConfig};

        let registry = registry()?;

        let max_symbols = params.max_symbols.unwrap_or(50).clamp(1, 200);
        let config = IsolateConfig {
            max_symbols,
            expansion_depth: 2,
        };

        // Isolate both repos
        let src_graph = self.graph(&params.source_repo_id).await?;
        let src_store = src_graph.store();
        let src_name = registry
            .get(&params.source_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.source_repo_id.clone());
        let src_symbols = src_graph.symbols().await?;
        let src_rels = src_graph.relationships().await?;
        let src_communities = src_graph.communities().await?;

        let src_embedder = myceliums_core::embedder_for_index(&src_store)
            .await
            .map_err(embedder_error)?;
        let source_slice = cross_repo::isolate_intent_hybrid(
            &src_embedder,
            &params.intent,
            &params.source_repo_id,
            &src_name,
            &src_symbols,
            src_store.as_ref(),
            &src_rels,
            &src_communities,
            &config,
        )
        .await
        .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;
        src_graph.verify()?;

        let tgt_graph = self.graph(&params.target_repo_id).await?;
        let tgt_store = tgt_graph.store();
        let tgt_name = registry
            .get(&params.target_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.target_repo_id.clone());
        let tgt_symbols = tgt_graph.symbols().await?;
        let tgt_rels = tgt_graph.relationships().await?;
        let tgt_communities = tgt_graph.communities().await?;

        let tgt_embedder = myceliums_core::embedder_for_index(&tgt_store)
            .await
            .map_err(embedder_error)?;
        let target_slice = cross_repo::isolate_intent_hybrid(
            &tgt_embedder,
            &params.intent,
            &params.target_repo_id,
            &tgt_name,
            &tgt_symbols,
            tgt_store.as_ref(),
            &tgt_rels,
            &tgt_communities,
            &config,
        )
        .await
        .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;
        tgt_graph.verify()?;

        // Differentiate
        let src_vectors = src_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;
        let tgt_vectors = tgt_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| ToolError::internal("cross-repo analysis failed", e))?;

        let diff_config = DifferentiateConfig {
            similarity_threshold: 0.65,
        };

        let report = cross_repo::differentiate_with_vectors(
            &source_slice,
            &target_slice,
            &src_vectors,
            &tgt_vectors,
            &diff_config,
        );

        // Generate adaptation plan
        let direction = params.direction.unwrap_or_default().as_str();
        let plan = cross_repo::plan_adaptation(&report, direction);

        self.two_repo_text(
            &src_graph,
            &tgt_graph,
            format::format_adaptation_plan(&plan),
        )
        .await
    }
}
