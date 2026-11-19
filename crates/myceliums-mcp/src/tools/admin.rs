//! Repository lifecycle — analyze, list, delete — and the server's call
//! statistics. The analysis itself lives in [`crate::analysis`].

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
        context: RequestContext<RoleServer>,
    ) -> TextResult {
        let control = crate::analysis::RunControl {
            sink: crate::analysis::ProgressSink::for_request(&context),
            observer: None,
            ct: context.ct.clone(),
        };
        self.analyze_with(params, control).await
    }

    #[tool(
        name = "server_stats",
        description = "Tool-call statistics of this MCP server since it started: calls, isError results, protocol errors, p50/p95/max latency and result size (bytes, ~tokens) per tool. Kept in memory on this machine only; nothing is ever sent anywhere.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    pub(crate) async fn server_stats(&self) -> TextResult {
        text(self.stats.snapshot().to_text())
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
        // Not while an analysis of it runs: its swap would bring it back.
        let _guard = crate::analysis::RunningGuard::acquire(&data_dir(), &params.repo_id)
            .map_err(|busy| busy.for_delete(&params.repo_id))?;
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

impl MyceliumsMcp {
    /// `analyze` with explicit progress and cancellation (tests drive these).
    pub(crate) async fn analyze_with(
        &self,
        params: AnalyzeParams,
        control: crate::analysis::RunControl,
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
                        let graph = self.graph(&repo_id).await?;
                        let indexed = graph.files().await?.shared();
                        let freshness =
                            crate::index_status::freshness(repo_info, indexed, true).await;
                        (!freshness.is_stale()).then_some((repo_id, reason, graph))
                    }
                    CacheDecision::ReanalyzeNeeded { .. } => None,
                };
                if let Some((repo_id, reason, graph)) = cached {
                    info!("Using cached analysis: {}", reason);
                    let store = graph.store();
                    let symbols = graph.symbols().await?;
                    let files = graph.files().await?;
                    let relationships = graph.relationships().await?;
                    let communities = graph.communities().await?;
                    let processes = graph.processes().await?;
                    let embedding_stats = load_embedding_stats(&store, symbols.len()).await?;
                    let calls = crate::index_status::call_resolution(&store)
                        .await
                        .map(|stats| stats.summary());
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
                        calls,
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

        // Honor the project's .myceliums.toml (analysis filters, embedding
        // model) the same way the CLI does.
        let config_path = abs_path.join(myceliums_core::config::CONFIG_FILENAME);
        let config = if config_path.exists() {
            let cfg = myceliums_core::config::ProjectConfig::load(&config_path).map_err(|e| {
                ToolError::recoverable(
                    format!("Invalid {}: {e}", config_path.display()),
                    "fix or remove that config file, then call analyze again",
                )
            })?;
            Some(cfg)
        } else {
            None
        };

        // Write path: `repo_id` was derived from the canonical project path
        // above, not supplied by the caller, and this run registers it.
        let data_dir = data_dir();
        let guard = crate::analysis::RunningGuard::acquire(&data_dir, &repo_id)
            .map_err(|busy| busy.for_analyze(&repo_id))?;
        let output = crate::analysis::Rebuild {
            repo_id,
            repo_name,
            project: abs_path,
            config,
            skip_embeddings: params.skip_embeddings.unwrap_or(false),
            data_dir,
            service: self.service.clone(),
            started_at,
            analyzed_commit,
            guard,
        }
        .run(control)
        .await?;
        text(format::format_analyze(&output))
    }
}
