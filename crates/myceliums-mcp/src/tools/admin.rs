//! Repository lifecycle: analyze, list, delete.

use super::prelude::*;

#[tool_router(router = admin_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "analyze",
        description = "Analyze a codebase and build its knowledge graph. Parses TypeScript and Python files, extracts symbols, resolves call relationships, detects communities, and traces execution flows. Uses cached analysis if fresh enough (set force=true to bypass cache)."
    )]
    async fn analyze(
        &self,
        Parameters(params): Parameters<AnalyzeParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let path = PathBuf::from(&params.path);
        let abs_path = std::fs::canonicalize(&path)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Invalid path: {}", e), None))?;

        let repo_id = analyzer::repo_id_from_path(&abs_path);
        // Derived ids are sanitized; checking keeps the write path honest too.
        RepoRegistry::validate_repo_id(&repo_id).map_err(repo_id_error)?;
        let repo_name = analyzer::repo_name_from_path(&abs_path);
        let force = params.force.unwrap_or(false);

        // Cache check: return cached results if fresh enough
        if !force {
            let registry = RepoRegistry::load(&registry_path())
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
            if let Some(repo_info) = registry.get(&repo_id) {
                let cache_config = CacheCheckConfig {
                    max_age_minutes: params.max_age_minutes.unwrap_or(60),
                    ..Default::default()
                };
                if let CacheDecision::UseCached { repo_id, reason } =
                    cache::check_cache(repo_info, &abs_path, &cache_config)
                {
                    info!("Using cached analysis: {}", reason);
                    let db_path = registered_db_path(&repo_id)?;
                    let store = Store::open(&db_path, &repo_id).await.map_err(|e| {
                        rmcp::ErrorData::internal_error(format!("Store error: {}", e), None)
                    })?;
                    let symbols = store
                        .get_symbols()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let files = store
                        .get_files()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let relationships = store
                        .get_relationships()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let communities = store
                        .get_communities()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let processes = store
                        .get_processes()
                        .await
                        .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
                    let embedding_stats = load_embedding_stats(&store, symbols.len()).await?;
                    let output = AnalyzeOutput {
                        repo_id,
                        symbols: symbols.len(),
                        files: files.len(),
                        relationships: relationships.len(),
                        communities: communities.len(),
                        processes: processes.len(),
                        symbols_embedded: embedding_stats.symbols_embedded,
                        symbols_total: embedding_stats.symbols_total,
                        embedding_failures: embedding_stats.embedding_failures,
                        cached: true,
                    };
                    return Ok(Json(TextOutput {
                        text: format::format_analyze(&output),
                    }));
                }
            }
        }

        // Write path: `repo_id` was derived from the canonical project path
        // above, not supplied by the caller, and this run registers it.
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        std::fs::create_dir_all(&db_path).map_err(|e| {
            rmcp::ErrorData::internal_error(format!("Failed to create dir: {}", e), None)
        })?;

        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Store error: {}", e), None))?;
        store
            .delete_repo_data()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Delete error: {}", e), None))?;

        let skip_embeddings = params.skip_embeddings.unwrap_or(false);
        // Honor the project's .myceliums.toml (analysis filters, embedding
        // model) the same way the CLI does.
        let config_path = abs_path.join(myceliums_core::config::CONFIG_FILENAME);
        let analyzer = if config_path.exists() {
            let cfg = myceliums_core::config::ProjectConfig::load(&config_path).map_err(|e| {
                rmcp::ErrorData::internal_error(
                    format!("Invalid {}: {}", config_path.display(), e),
                    None,
                )
            })?;
            Analyzer::with_config(store, abs_path.clone(), cfg)
        } else {
            Analyzer::new(store, abs_path.clone())
        };
        let analyzer = analyzer.set_skip_embeddings(skip_embeddings);
        let result = analyzer
            .analyze()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Analysis error: {}", e), None))?;

        // Community detection and process tracing
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("Store error: {}", e), None))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let communities = CommunityDetector::detect(&symbols, &relationships, &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let community_count = store
            .store_communities(&communities)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let processes = ProcessTracer::trace(&symbols, &relationships, &repo_id)
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        let process_count = store
            .store_processes(&processes)
            .await
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Get current git commit for cache tracking
        let analyzed_commit = cache::get_head_commit(&abs_path).ok();

        let mut registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        registry.register(RepoInfo {
            id: repo_id.clone(),
            name: repo_name,
            path: abs_path.to_string_lossy().to_string(),
            analyzed_at: chrono::Utc::now().to_rfc3339(),
            symbol_count: result.symbol_count as u32,
            file_count: result.file_count as u32,
            analyzed_commit,
            vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
        });
        registry
            .save()
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        // Save lightweight snapshot for future diff comparisons
        let snapshot = build_snapshot(&repo_id, &symbols, &relationships);
        let _ = save_snapshot(&data_dir(), &snapshot); // best-effort

        let embedding_stats = result.embedding_stats();
        let output = AnalyzeOutput {
            repo_id,
            symbols: result.symbol_count,
            files: result.file_count,
            relationships: result.relationship_count,
            communities: community_count,
            processes: process_count,
            symbols_embedded: embedding_stats.symbols_embedded,
            symbols_total: embedding_stats.symbols_total,
            embedding_failures: embedding_stats.embedding_failures,
            cached: false,
        };
        Ok(Json(TextOutput {
            text: format::format_analyze(&output),
        }))
    }

    #[tool(
        name = "list_repos",
        description = "List the analyzed repositories (id, name, path, last analysis, size). Read-only. Use it to find the repo_id the other tools expect."
    )]
    pub(crate) async fn list_repos(&self) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        Ok(Json(TextOutput {
            text: format::format_repo_list(&registry.list()),
        }))
    }

    #[tool(name = "delete", description = "Delete a repository's analysis data")]
    pub(crate) async fn delete(
        &self,
        Parameters(params): Parameters<DeleteParams>,
    ) -> Result<Json<TextOutput>, rmcp::ErrorData> {
        let mut registry = RepoRegistry::load(&registry_path())
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;

        let db_path = registry
            .registered_db_path(&data_dir(), &params.repo_id)
            .map_err(repo_id_error)?;
        let info = registry
            .remove(&params.repo_id)
            .expect("registered_db_path accepted the id, so it is registered");
        registry
            .save()
            .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        if db_path.exists() {
            std::fs::remove_dir_all(&db_path)
                .map_err(|e| rmcp::ErrorData::internal_error(format!("{}", e), None))?;
        }
        Ok(Json(TextOutput {
            text: format!("Deleted: {} ({})", info.name, params.repo_id),
        }))
    }
}
