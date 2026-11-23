//! Repository lifecycle — analyze, list, delete — and the server's call
//! statistics. The analysis itself lives in [`crate::analysis`].

use super::prelude::*;

#[tool_router(router = admin_router, vis = "pub(crate)")]
impl MyceliumsMcp {
    #[tool(
        name = "analyze",
        description = "Index a project (20+ languages, Markdown, PDF, email) into the graph. Run it when there is no index or the Index line says stale. Reuses a fresh index unless force=true; embeddings for search by meaning follow in the background. Writes only to the myceliums data directory.",
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
        let mut out = self.stats.snapshot().to_text();
        if let Some(jobs) = crate::embedding_jobs::summary() {
            out.push_str(&format!("\n\n{jobs}"));
        }
        text(out)
    }

    #[tool(
        name = "list_repos",
        description = "List the indexed repositories: id, path, indexed commit. Tools default to the repository of the working directory; pass another id as repo_id.",
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
        // The index directory, or the link to its generation with the
        // generation (#175).
        myceliums_core::remove_index(&db_path)
            .map_err(|e| ToolError::internal("cannot remove the index directory", e))?;
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
        let skip_embeddings = params.skip_embeddings.unwrap_or(false);
        let skip_communities = params.skip_communities.unwrap_or(false);

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

        // Cache check: return cached results if fresh enough
        if !force {
            let registry = registry()?;
            // A registered repository whose index directory is gone is not
            // cached: analyze rebuilds it (#118).
            let indexed = RepoRegistry::repo_db_path(&data_dir(), &repo_id).is_dir();
            if let Some(repo_info) = registry.get(&repo_id).filter(|_| indexed) {
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
                            crate::index_status::freshness(repo_info, indexed, graph.store(), true)
                                .await;
                        (!freshness.is_stale()).then_some((repo_id, reason, graph))
                    }
                    CacheDecision::ReanalyzeNeeded { .. } => None,
                };
                if let Some((repo_id, reason, graph)) = cached {
                    let store = graph.store();
                    let symbols = graph.symbols().await?;
                    // The vectors must come from the embedder the project
                    // configures now (#157).
                    let plan =
                        embedding_plan(&store, config.as_ref(), skip_embeddings, symbols.len())
                            .await?;
                    if let EmbeddingPlan::Rebuild(why) = &plan {
                        info!("Re-analyzing {repo_id}: {why}");
                    } else {
                        info!("Using cached analysis: {}", reason);
                        let files = graph.files().await?;
                        let relationships = graph.relationships().await?;
                        let communities = graph.communities().await?;
                        let processes = graph.processes().await?;
                        let embedding_stats = load_embedding_stats(&store, symbols.len()).await?;
                        let calls = crate::index_status::call_resolution(&store)
                            .await
                            .map(|stats| stats.summary());
                        let community_status = myceliums_core::CommunityStatus::load(&store)
                            .await
                            .ok()
                            .flatten();
                        // The two reads above bypassed the tables: check they
                        // come from the generation the counts do.
                        graph.verify()?;
                        let mut output = AnalyzeOutput {
                            repo_id: repo_id.clone(),
                            symbols: symbols.len(),
                            files: files.len(),
                            relationships: relationships.len(),
                            communities: communities.len(),
                            communities_state: community_status
                                .as_ref()
                                .map(|s| s.state.to_string()),
                            communities_notice: community_status.as_ref().and_then(|s| s.notice()),
                            phases: None,
                            processes: processes.len(),
                            symbols_embedded: embedding_stats.symbols_embedded,
                            symbols_total: embedding_stats.symbols_total,
                            embedding_failures: embedding_stats.embedding_failures,
                            embeddings_reused: 0,
                            embeddings_pending: 0,
                            embedding_model: None,
                            calls,
                            skipped: None,
                            cached: true,
                        };
                        let mut note = crate::embedding_jobs::status_line(&repo_id);
                        if let EmbeddingPlan::Resume { pending, model } = plan {
                            if crate::embedding_jobs::is_running(&repo_id) {
                                // The status line says how far it is.
                            } else {
                                match crate::analysis::RunningGuard::acquire(&data_dir(), &repo_id)
                                {
                                    Ok(guard) => {
                                        output.embeddings_pending = pending;
                                        output.embedding_model = Some(model.clone());
                                        // Its failures are being retried.
                                        output.embedding_failures = 0;
                                        note = None;
                                        crate::embedding_jobs::start(
                                            crate::embedding_jobs::PendingJob {
                                                repo_id: repo_id.clone(),
                                                db_path: RepoRegistry::repo_db_path(
                                                    &data_dir(),
                                                    &repo_id,
                                                ),
                                                model,
                                                pending,
                                                options: embed_options(config.as_ref()),
                                                service: self.service.clone(),
                                                guard,
                                            },
                                        );
                                    }
                                    Err(busy) => {
                                        note = Some(busy.for_analyze(&repo_id).message());
                                    }
                                }
                            }
                        }
                        let mut answer = format::format_analyze(&output);
                        if let Some(note) = note {
                            answer.push_str(&format!("\n{note}"));
                        }
                        return text(answer);
                    }
                }
            }
        }

        // Recorded as the index time and commit: files edited (or commits
        // made) while the analysis runs then count as changes since indexing.
        let started_at = chrono::Utc::now().to_rfc3339();
        let analyzed_commit = cache::get_head_commit(&abs_path).ok();

        // Write path: `repo_id` was derived from the canonical project path
        // above, not supplied by the caller, and this run registers it.
        let data_dir = data_dir();
        // A background embedding of this repository holds its lock, and the
        // index it embeds is about to be replaced (another embedder, a moved
        // HEAD, force): stop it — what it stored carries over — and wait for
        // it (#157).
        if crate::embedding_jobs::cancel(&repo_id)
            && !crate::embedding_jobs::wait_until_stopped(&repo_id, JOB_STOP_WAIT).await
        {
            return Err(ToolError::recoverable(
                format!(
                    "The background embedding of {repo_id:?} was asked to stop for this analysis \
                     and is still finishing its current step (a model batch, or the model \
                     download)."
                ),
                "call analyze again in a moment",
            ));
        }
        let guard = crate::analysis::RunningGuard::acquire(&data_dir, &repo_id)
            .map_err(|busy| busy.for_analyze(&repo_id))?;
        let output = crate::analysis::Rebuild {
            repo_id,
            repo_name,
            project: abs_path,
            config,
            skip_embeddings,
            skip_communities,
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

/// How long `analyze` waits for a background embedding it stopped.
const JOB_STOP_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// What the cached index needs for the embedder the project configures now
/// (#157).
enum EmbeddingPlan {
    /// Nothing: its vectors are current (or embeddings are skipped).
    Current,
    /// Its vectors are of the configured embedder, but some symbols have
    /// none: embed them in the background.
    Resume { pending: usize, model: String },
    /// Its vectors are of another embedder, or it has none and records no
    /// embedder: a full analysis, which embeds in the background.
    Rebuild(String),
}

/// Compares the embedder the index records with the one `config`
/// configures: a changed model, template or chunking re-analyzes the index,
/// as `myc analyze` does; symbols still without a vector are embedded.
async fn embedding_plan(
    store: &myceliums_storage::Store,
    config: Option<&myceliums_core::config::ProjectConfig>,
    skip_embeddings: bool,
    symbol_count: usize,
) -> Result<EmbeddingPlan, ToolError> {
    use myceliums_core::IndexEmbeddingMeta;
    if skip_embeddings || symbol_count == 0 {
        return Ok(EmbeddingPlan::Current);
    }
    let section = config.map(|c| c.embedding.clone()).unwrap_or_default();
    let configured = IndexEmbeddingMeta::from_config(&section).map_err(|e| {
        ToolError::recoverable(
            format!("Invalid [embedding] configuration: {e}"),
            "fix the [embedding] section of .myceliums.toml, or call analyze with \
             skip_embeddings=true",
        )
    })?;
    let recorded = store
        .get_index_meta(IndexEmbeddingMeta::META_KEY)
        .await
        .map_err(|e| ToolError::internal("cannot read embedding metadata", e))?;
    let Some(recorded) = recorded else {
        return Ok(EmbeddingPlan::Rebuild(
            "the index has no embeddings yet".to_string(),
        ));
    };
    let Ok(recorded) = serde_json::from_str::<IndexEmbeddingMeta>(&recorded) else {
        return Ok(EmbeddingPlan::Rebuild(
            "the index's embedding metadata is unreadable".to_string(),
        ));
    };
    if recorded.fingerprint() != configured.fingerprint() {
        return Ok(EmbeddingPlan::Rebuild(format!(
            "its vectors come from {}, the project configures {}",
            recorded.fingerprint(),
            configured.fingerprint()
        )));
    }
    let stats = load_embedding_stats(store, symbol_count).await?;
    Ok(if stats.is_partial() {
        EmbeddingPlan::Resume {
            pending: stats.symbols_total.saturating_sub(stats.symbols_embedded),
            model: recorded.model,
        }
    } else {
        EmbeddingPlan::Current
    })
}

/// Embedding batch limits from `[analysis]`.
fn embed_options(
    config: Option<&myceliums_core::config::ProjectConfig>,
) -> myceliums_core::EmbedOptions {
    config
        .map(|c| {
            myceliums_core::EmbedOptions::new(
                c.analysis.embedding_batch_size,
                c.analysis.embedding_batch_tokens,
            )
        })
        .unwrap_or_default()
}
