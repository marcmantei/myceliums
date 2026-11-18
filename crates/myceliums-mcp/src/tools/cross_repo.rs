//! Intent isolation and cross-repository comparison/adaptation.

use super::prelude::*;

#[tool_router(router = cross_repo_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "isolate_intent",
        description = "Isolate the symbols implementing a specific intent/feature in a repository. Uses hybrid search to find seed symbols, then expands via call graph traversal with community-aware pruning. Returns the IntentSlice: seed symbols, expanded symbols, internal relationships, and structural metadata."
    )]
    async fn isolate_intent(
        &self,
        Parameters(params): Parameters<IsolateIntentParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        use myceliums_core::cross_repo::{self, IsolateConfig};

        let store = self.store(&params.repo_id).await?;

        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let repo_name = registry
            .get(&params.repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.repo_id.clone());

        let symbols = self.symbols(&params.repo_id).await?;
        let relationships = self.relationships(&params.repo_id).await?;
        let communities = self.communities(&params.repo_id).await?;

        let config = IsolateConfig {
            max_symbols: params.max_symbols.unwrap_or(50),
            expansion_depth: params.depth.unwrap_or(2),
        };

        let embedder = myceliums_core::embedder_for_index(&store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
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
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        Ok(Json(TextOutput {
            text: format::format_intent_slice(&slice),
        }))
    }

    #[tool(
        name = "differentiate_intent",
        description = "Compare how two repositories implement the same intent/feature. Isolates the relevant symbols in each repo, aligns them via embedding similarity, and reports structural differences. Returns: matched symbol pairs, unmatched symbols, and structural comparison metrics."
    )]
    async fn differentiate_intent(
        &self,
        Parameters(params): Parameters<DifferentiateIntentParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        use myceliums_core::cross_repo::{self, DifferentiateConfig, IsolateConfig};

        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let max_symbols = params.max_symbols.unwrap_or(50);
        let config = IsolateConfig {
            max_symbols,
            expansion_depth: 2,
        };

        // Isolate intent in source repo
        let src_store = self.store(&params.source_repo_id).await?;
        let src_name = registry
            .get(&params.source_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.source_repo_id.clone());
        let src_symbols = self.symbols(&params.source_repo_id).await?;
        let src_rels = self.relationships(&params.source_repo_id).await?;
        let src_communities = self.communities(&params.source_repo_id).await?;

        let src_embedder = myceliums_core::embedder_for_index(&src_store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Source isolation: {}", e), None)
            })?;
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
        .map_err(|e| rmcp::ErrorData::internal_error(format!("Source isolation: {}", e), None))?;

        // Isolate intent in target repo
        let tgt_store = self.store(&params.target_repo_id).await?;
        let tgt_name = registry
            .get(&params.target_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.target_repo_id.clone());
        let tgt_symbols = self.symbols(&params.target_repo_id).await?;
        let tgt_rels = self.relationships(&params.target_repo_id).await?;
        let tgt_communities = self.communities(&params.target_repo_id).await?;

        let tgt_embedder = myceliums_core::embedder_for_index(&tgt_store)
            .await
            .map_err(|e| {
                rmcp::ErrorData::internal_error(format!("Target isolation: {}", e), None)
            })?;
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
        .map_err(|e| rmcp::ErrorData::internal_error(format!("Target isolation: {}", e), None))?;

        // Get vectors for differentiation
        let src_vectors = src_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_vectors = tgt_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let diff_config = DifferentiateConfig {
            similarity_threshold: params.similarity_threshold.unwrap_or(0.65),
        };

        let report = cross_repo::differentiate_with_vectors(
            &source_slice,
            &target_slice,
            &src_vectors,
            &tgt_vectors,
            &diff_config,
        );

        Ok(Json(TextOutput {
            text: format::format_differentiation_report(&report),
        }))
    }

    #[tool(
        name = "plan_adaptation",
        description = "Generate an actionable adaptation plan for migrating one repository's approach to another. Compares intent implementations, then produces ordered steps with dependency tracking, effort estimates, and risk analysis."
    )]
    async fn plan_adaptation(
        &self,
        Parameters(params): Parameters<PlanAdaptationParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        use myceliums_core::cross_repo::{self, DifferentiateConfig, IsolateConfig};

        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let max_symbols = params.max_symbols.unwrap_or(50);
        let config = IsolateConfig {
            max_symbols,
            expansion_depth: 2,
        };

        // Isolate both repos
        let src_store = self.store(&params.source_repo_id).await?;
        let src_name = registry
            .get(&params.source_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.source_repo_id.clone());
        let src_symbols = self.symbols(&params.source_repo_id).await?;
        let src_rels = self.relationships(&params.source_repo_id).await?;
        let src_communities = self.communities(&params.source_repo_id).await?;

        let src_embedder = myceliums_core::embedder_for_index(&src_store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
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
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let tgt_store = self.store(&params.target_repo_id).await?;
        let tgt_name = registry
            .get(&params.target_repo_id)
            .map(|r| r.name.clone())
            .unwrap_or_else(|| params.target_repo_id.clone());
        let tgt_symbols = self.symbols(&params.target_repo_id).await?;
        let tgt_rels = self.relationships(&params.target_repo_id).await?;
        let tgt_communities = self.communities(&params.target_repo_id).await?;

        let tgt_embedder = myceliums_core::embedder_for_index(&tgt_store)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
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
        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Differentiate
        let src_vectors = src_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let tgt_vectors = tgt_store
            .get_symbols_with_vectors()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

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
        let direction = params.direction.as_deref().unwrap_or("source_to_target");
        let plan = cross_repo::plan_adaptation(&report, direction);

        Ok(Json(TextOutput {
            text: format::format_adaptation_plan(&plan),
        }))
    }
}
