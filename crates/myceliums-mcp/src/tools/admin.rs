//! Repository lifecycle: analyze, list, delete.

use super::prelude::*;

#[tool_router(router = admin_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "analyze",
        description = "Index a project into the knowledge graph: parses 20+ languages (TypeScript/JavaScript, Python, Go, Rust, Java, Kotlin, C/C++, C#, Ruby, PHP, Swift, …) plus Markdown, PDF and email, extracts symbols and call edges, detects communities and traces execution flows. Reuses a fresh index unless force=true. Writes only to the myceliums data directory, never to the project.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn analyze(
        &self,
        Parameters(params): Parameters<AnalyzeParams>,
    ) -> TextResult {
        let path = PathBuf::from(&params.path);
        let abs_path = std::fs::canonicalize(&path).map_err(|e| {
            ToolError::invalid(format!(
                "path {:?} is not an accessible directory: {e}",
                params.path
            ))
        })?;
        if !abs_path.is_dir() {
            return Err(ToolError::invalid(format!(
                "path {:?} is not a directory",
                params.path
            )));
        }

        let repo_id = analyzer::repo_id_from_path(&abs_path);
        // Derived ids are sanitized; checking keeps the write path honest too.
        RepoRegistry::validate_repo_id(&repo_id).map_err(repo_id_error)?;
        let repo_name = analyzer::repo_name_from_path(&abs_path);
        let force = params.force.unwrap_or(false);

        // Cache check: return cached results if fresh enough
        if !force {
            let registry = registry()?;
            if let Some(repo_info) = registry.get(&repo_id) {
                let cache_config = CacheCheckConfig {
                    max_age_minutes: params.max_age_minutes.unwrap_or(60),
                    ..Default::default()
                };
                // The core cache tolerates a few changed files; the freshness
                // line does not. Re-index whenever that line would call the
                // index stale, so "run analyze to refresh" always refreshes.
                let cached = match cache::check_cache(repo_info, &abs_path, &cache_config) {
                    CacheDecision::UseCached { repo_id, reason } => {
                        let indexed = self.files(&repo_id).await?.shared();
                        let freshness =
                            crate::index_status::freshness(repo_info, indexed, true).await;
                        (!freshness.is_stale()).then_some((repo_id, reason))
                    }
                    CacheDecision::ReanalyzeNeeded { .. } => None,
                };
                if let Some((repo_id, reason)) = cached {
                    info!("Using cached analysis: {}", reason);
                    let store = self.store(&repo_id).await?;
                    let symbols = self.symbols(&repo_id).await?;
                    let files = self.files(&repo_id).await?;
                    let relationships = self.relationships(&repo_id).await?;
                    let communities = self.communities(&repo_id).await?;
                    let processes = self.processes(&repo_id).await?;
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
                    return text(format::format_analyze(&output));
                }
            }
        }

        // Recorded as the index time and commit: files edited (or commits
        // made) while the analysis runs then count as changes since indexing.
        let started_at = chrono::Utc::now().to_rfc3339();
        let analyzed_commit = cache::get_head_commit(&abs_path).ok();

        // Write path: `repo_id` was derived from the canonical project path
        // above, not supplied by the caller, and this run registers it.
        // The index is about to be rewritten: drop the cached graph now, and
        // again once the new index is complete (below).
        self.service.invalidate(&repo_id);
        let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
        std::fs::create_dir_all(&db_path)
            .map_err(|e| ToolError::internal("cannot create the index directory", e))?;

        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| ToolError::internal("cannot open the index", e))?;
        store
            .delete_repo_data()
            .await
            .map_err(|e| ToolError::internal("cannot clear the old index", e))?;

        let skip_embeddings = params.skip_embeddings.unwrap_or(false);
        // Honor the project's .myceliums.toml (analysis filters, embedding
        // model) the same way the CLI does.
        let config_path = abs_path.join(myceliums_core::config::CONFIG_FILENAME);
        let analyzer = if config_path.exists() {
            let cfg = myceliums_core::config::ProjectConfig::load(&config_path).map_err(|e| {
                ToolError::recoverable(
                    format!("Invalid {}: {e}", config_path.display()),
                    "fix or remove that config file, then call analyze again",
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
            .map_err(|e| ToolError::internal("analysis failed", e))?;

        // Community detection and process tracing
        let store = Store::open(&db_path, &repo_id)
            .await
            .map_err(|e| ToolError::internal("cannot open the index", e))?;
        let symbols = store
            .get_symbols()
            .await
            .map_err(|e| ToolError::internal("cannot read symbols", e))?;
        let relationships = store
            .get_relationships()
            .await
            .map_err(|e| ToolError::internal("cannot read relationships", e))?;

        let communities = CommunityDetector::detect(&symbols, &relationships, &repo_id)
            .map_err(|e| ToolError::internal("community detection failed", e))?;
        let community_count = store
            .store_communities(&communities)
            .await
            .map_err(|e| ToolError::internal("cannot store communities", e))?;

        let processes = ProcessTracer::trace(&symbols, &relationships, &repo_id)
            .map_err(|e| ToolError::internal("process tracing failed", e))?;
        let process_count = store
            .store_processes(&processes)
            .await
            .map_err(|e| ToolError::internal("cannot store processes", e))?;

        let mut registry = registry()?;
        registry.register(RepoInfo {
            id: repo_id.clone(),
            name: repo_name,
            path: abs_path.to_string_lossy().to_string(),
            analyzed_at: started_at,
            symbol_count: result.symbol_count as u32,
            file_count: result.file_count as u32,
            analyzed_commit: analyzed_commit.clone(),
            vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
        });
        registry
            .save()
            .map_err(|e| ToolError::internal("cannot save the repository registry", e))?;
        // Reads from here on see the new index, not a graph cached mid-run.
        self.service.invalidate(&repo_id);

        // Record this analysis for detect_architecture_drift. Best effort: a
        // failed write costs the drift history, not the index.
        let snapshot = build_snapshot(&repo_id, &symbols, &relationships);
        if let Err(e) =
            myceliums_core::record_snapshot(&data_dir(), &snapshot, analyzed_commit.as_deref())
        {
            tracing::warn!("cannot record the graph snapshot: {e}");
        }

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
        text(format::format_analyze(&output))
    }

    #[tool(
        name = "list_repos",
        description = "List the analyzed repositories (id, name, path, when and from which commit they were indexed, size). Tools default to the repository of the working directory; pass another one's id as repo_id.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn list_repos(
        &self,
        Parameters(params): Parameters<ListReposParams>,
    ) -> Result<Json<ReposOutput>, ToolError> {
        let registry = registry()?;
        let mut repos = registry.list();
        // Most recently analyzed first (RFC 3339 in one offset sorts as text).
        repos.sort_by(|a, b| b.analyzed_at.cmp(&a.analyzed_at).then(a.id.cmp(&b.id)));
        let total_count = repos.len();
        let page = Page::new(params.limit, params.offset, Budget::LIST).take(repos);
        let notice = if total_count == 0 {
            Some("No repositories analyzed yet: run analyze with the project path.".to_string())
        } else {
            page.notice
        };
        Ok(Json(ReposOutput {
            repos: page
                .items
                .into_iter()
                .map(|r| RepoItem {
                    id: r.id.clone(),
                    name: r.name.clone(),
                    path: r.path.clone(),
                    analyzed_at: r.analyzed_at.clone(),
                    analyzed_commit: r.analyzed_commit.clone(),
                    symbols: r.symbol_count,
                    files: r.file_count,
                })
                .collect(),
            total_count,
            notice,
        }))
    }

    #[tool(
        name = "delete",
        description = "Permanently delete a repository's index (not the project files). Irreversible: requires confirm=true; only do it when the user asked for it.",
        annotations(
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    pub(crate) async fn delete(&self, Parameters(params): Parameters<DeleteParams>) -> TextResult {
        let mut registry = registry()?;

        let db_path = registry
            .registered_db_path(&data_dir(), &params.repo_id)
            .map_err(repo_id_error)?;
        if !params.confirm {
            return Err(ToolError::recoverable(
                format!(
                    "Refusing to delete the index of {:?} without confirm=true.",
                    params.repo_id
                ),
                "only if the user asked for the deletion, call delete again with confirm=true",
            ));
        }
        let info = registry
            .remove(&params.repo_id)
            .expect("registered_db_path accepted the id, so it is registered");
        registry
            .save()
            .map_err(|e| ToolError::internal("cannot save the repository registry", e))?;
        self.service.invalidate(&params.repo_id);
        if db_path.exists() {
            std::fs::remove_dir_all(&db_path)
                .map_err(|e| ToolError::internal("cannot remove the index directory", e))?;
        }
        text(format!("Deleted: {} ({})", info.name, params.repo_id))
    }
}
