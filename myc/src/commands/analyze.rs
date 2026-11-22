//! `myc analyze`.

use crate::progress::IndicatifReporter;
use crate::repo::{data_dir, registry_path};
use anyhow::{Context, Result};
use myceliums_core::analyzer::{self, Analyzer};
use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
use myceliums_core::config::{self, ProjectConfig};
use myceliums_core::{CommunityPlan, ProcessTracer};
use myceliums_storage::{RepoInfo, RepoRegistry, Store};
use std::path::{Path, PathBuf};

/// Check if a path is a dangerous ancestor (/, /Users, /home, drive roots).
fn is_dangerous_ancestor(path: &Path) -> bool {
    // Root directories that should never be indexed
    let dangerous = ["/", "/Users", "/home"];
    for ancestor in &dangerous {
        if path.as_os_str() == *ancestor {
            return true;
        }
    }

    // On Windows, check for drive roots (C:\, D:\, etc.)
    #[cfg(windows)]
    {
        if let Some(s) = path.to_str() {
            // Drive root pattern: "C:\", "D:\", etc.
            if s.len() == 3 && s.chars().nth(1) == Some(':') && s.ends_with('\\') {
                return true;
            }
        }
    }

    false
}

/// Check if a target path is a problematic parent of home directory.
/// This catches attempts to analyze /Users, /home, or root on systems
/// where home is a subdirectory.
fn is_home_ancestor(target: &Path, home: &Path) -> bool {
    // Check if target is an ancestor of home (e.g., /Users when home is /Users/marc)
    match home.strip_prefix(target) {
        Ok(_) => target != home, // target is an ancestor of home (but not home itself)
        Err(_) => false,
    }
}

pub(crate) fn check_session_safeguards(path: &Path, no_git_check: bool) -> Result<()> {
    // Fail closed: if canonicalize fails, refuse the operation rather than proceeding
    let abs_path = std::fs::canonicalize(path).with_context(|| {
        format!(
            "Could not canonicalize path: {}. This may indicate a symlink loop or permission issue. Use --no-git-check if you are certain this is safe.",
            path.display()
        )
    })?;

    // Home directory guard — enhanced
    if let Some(home) = dirs::home_dir() {
        if let Ok(canonical_home) = std::fs::canonicalize(&home) {
            // Refuse exact home directory
            if abs_path == canonical_home {
                anyhow::bail!(
                    "Refusing to analyze home directory (~). \
                     This would index hundreds of thousands of files and consume \
                     excessive CPU and memory. Specify a project directory instead:\n\n  \
                     myc analyze ./my-project"
                );
            }

            // Refuse ancestors of home (/, /Users, /home, drive roots)
            if is_home_ancestor(&abs_path, &canonical_home) {
                anyhow::bail!(
                    "Refusing to analyze parent directory of home (~). \
                     Path: {} would index your entire home directory and more. \
                     Specify a project directory instead:\n\n  \
                     myc analyze ./my-project",
                    abs_path.display()
                );
            }

            // Refuse known dangerous ancestors
            if is_dangerous_ancestor(&abs_path) {
                anyhow::bail!(
                    "Refusing to analyze system root or shared user directory: {}. \
                     This would consume excessive resources. \
                     Specify a project directory instead:\n\n  \
                     myc analyze ./my-project",
                    abs_path.display()
                );
            }
        }
    }

    // Git repository guard
    if !no_git_check && !abs_path.join(".git").exists() {
        anyhow::bail!(
            "No .git directory found at {}. \
             Myceliums is designed for git repositories. \
             Use --no-git-check to override.",
            abs_path.display()
        );
    }

    Ok(())
}

/// How an analysis run treats embedding coverage.
///
/// Embedding behaviour is two decisions that must travel together: whether to
/// generate vectors at all, and whether an incompletely embedded index is a
/// failure. Passing them as two independent `bool` arguments is what let the
/// session bootstrap silently drop `--strict-embeddings` (#51) — a single value
/// makes the combination explicit at every call site and unrepresentable in
/// contradictory states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EmbeddingPolicy {
    /// Generate embeddings; a partial index is reported but tolerated.
    Embed,
    /// Generate embeddings and fail the run if the index is not complete.
    EmbedStrict,
    /// Do not generate embeddings at all (fast path; no vectors are written).
    Skip,
}

impl EmbeddingPolicy {
    /// Resolve the CLI flags into a policy.
    ///
    /// `--skip-embeddings` and `--strict-embeddings` are mutually exclusive
    /// (clap rejects the combination), so skipping always wins here.
    pub(crate) fn from_flags(skip_embeddings: bool, strict_embeddings: bool) -> Self {
        match (skip_embeddings, strict_embeddings) {
            (true, _) => Self::Skip,
            (false, true) => Self::EmbedStrict,
            (false, false) => Self::Embed,
        }
    }

    /// The policy for a session bootstrap run.
    ///
    /// Session startup skips embeddings for speed by default, but
    /// `--strict-embeddings` opts into the slower, verified path. This is the
    /// single place that decision is made, so the session command cannot drift
    /// back into ignoring the flag (#51).
    pub(crate) fn for_session(strict_embeddings: bool) -> Self {
        if strict_embeddings {
            Self::EmbedStrict
        } else {
            Self::Skip
        }
    }

    /// Whether embedding generation is bypassed entirely.
    fn skips_embeddings(self) -> bool {
        matches!(self, Self::Skip)
    }

    /// Whether an incompletely embedded index should fail the run.
    pub(crate) fn is_strict(self) -> bool {
        matches!(self, Self::EmbedStrict)
    }
}

/// What to tell the user before a run embeds every symbol from scratch — no
/// previous index of this embedder to take vectors from: the model, that a
/// first index takes the longest (and later runs reuse unchanged vectors),
/// the one-time download when the weights are not cached, and the fast
/// option (#157). `None` when the previous vectors carry over.
fn first_index_notice(
    section: &myceliums_core::config::EmbeddingSection,
    previous: Option<&myceliums_core::PreviousVectors>,
) -> Option<String> {
    use myceliums_core::{embedding_model_spec, local_model_cached, IndexEmbeddingMeta};
    let configured = IndexEmbeddingMeta::from_config(section).ok()?;
    if previous.is_some_and(|p| p.meta().fingerprint() == configured.fingerprint()) {
        return None;
    }
    let mut notice = format!(
        "Embedding every symbol with {}{}: a first index takes the longest; later runs \
         reuse the vectors of unchanged symbols.",
        configured.model,
        if configured.chunking.is_some() {
            " (long symbols in several chunks)"
        } else {
            ""
        }
    );
    if configured.provider == "local" {
        if let Some(spec) = embedding_model_spec(&configured.model) {
            if !local_model_cached(spec.id) {
                notice.push_str(&format!(
                    " The model is downloaded once (about {} MB).",
                    spec.download_mb
                ));
            }
        }
        if configured.model != "multilingual-e5-small" {
            notice.push_str(
                " For a faster first index, set `model = \"multilingual-e5-small\"` and \
                 `chunking = false` in the [embedding] section of .myceliums.toml.",
            );
        }
    }
    Some(notice)
}

/// Why the index at `db_path` must be re-embedded for the embedding
/// configuration of the project at `root`, or `None` when its vectors already
/// match (or embeddings are skipped, or the index records no embedder: a run
/// without vectors has nothing to redo). Best effort: anything unreadable is
/// `None`, and the usual freshness check decides.
async fn embedder_change(
    root: &Path,
    db_path: &Path,
    repo_id: &str,
    skip_embeddings: bool,
) -> Option<String> {
    use myceliums_core::IndexEmbeddingMeta;
    if skip_embeddings {
        return None;
    }
    let config_path = root.join(config::CONFIG_FILENAME);
    let section = if config_path.exists() {
        ProjectConfig::load(&config_path).ok()?.embedding
    } else {
        Default::default()
    };
    let configured = IndexEmbeddingMeta::from_config(&section).ok()?;
    let store = Store::open_existing(db_path, repo_id).await.ok()?;
    let json = store
        .get_index_meta(IndexEmbeddingMeta::META_KEY)
        .await
        .ok()??;
    let recorded: IndexEmbeddingMeta = serde_json::from_str(&json).ok()?;
    (recorded.fingerprint() != configured.fingerprint()).then(|| {
        format!(
            "the index was embedded with {} (template {}), the configuration asks for {} (template {})",
            recorded.identity(),
            recorded.text_version,
            configured.identity(),
            configured.text_version
        )
    })
}

pub(crate) async fn cmd_analyze(
    path: &Path,
    force: bool,
    max_age_minutes: u64,
    embedding_policy: EmbeddingPolicy,
    skip_communities: bool,
) -> Result<()> {
    use myceliums_core::lock::{AnalysisLock, LockOutcome};

    let skip_embeddings = embedding_policy.skips_embeddings();

    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let repo_id = analyzer::repo_id_from_path(&abs_path);
    RepoRegistry::validate_repo_id(&repo_id)?;
    let repo_name = analyzer::repo_name_from_path(&abs_path);
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);

    // Cache check: skip re-analysis if cache is fresh (unless --force)
    if !force {
        let registry = RepoRegistry::load(&registry_path())?;
        // Registered, but the index directory is gone: nothing to reuse.
        if registry.get(&repo_id).is_some() && !db_path.is_dir() {
            println!(
                "Re-analysis needed: the index {} is missing",
                db_path.display()
            );
        } else if let Some(change) =
            embedder_change(&abs_path, &db_path, &repo_id, skip_embeddings).await
        {
            // The index's vectors come from another embedder than the one
            // configured now (a model edit, or a release with a new default
            // model, template or chunking, #157): only a full run re-embeds.
            println!("Re-analysis needed: {change}");
        } else if let Some(repo_info) = registry.get(&repo_id) {
            let cache_config = CacheCheckConfig {
                max_age_minutes,
                ..Default::default()
            };
            match cache::check_cache(repo_info, &abs_path, &cache_config) {
                CacheDecision::UseCached { repo_id, reason } => {
                    println!("Using cached analysis for {} ({})", repo_name, repo_id);
                    println!("  Reason: {}", reason);
                    println!("  Analyzed at: {}", repo_info.analyzed_at);
                    println!(
                        "  Symbols: {}, Files: {}",
                        repo_info.symbol_count, repo_info.file_count
                    );
                    println!();
                    println!("Use --force to re-analyze.");
                    // Reusing a cached index must not weaken the strict
                    // contract: verify the *stored* accounting instead of
                    // returning success without ever checking coverage.
                    if embedding_policy.is_strict() {
                        enforce_strict_embeddings(&db_path, &repo_id).await?;
                    }
                    return Ok(());
                }
                CacheDecision::ReanalyzeNeeded { reason } => {
                    println!("Re-analysis needed: {}", reason);
                }
            }
        }
    }

    // Acquire analysis lock — prevents concurrent indexing of the same repo
    let _lock = match AnalysisLock::acquire(&data_dir(), &repo_id)? {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::AlreadyRunning { pid } => {
            println!(
                "Analysis already in progress for {} (PID {}). Skipping.",
                repo_name, pid
            );
            return Ok(());
        }
    };

    // Check for project-level config
    let config_path = abs_path.join(config::CONFIG_FILENAME);
    let project_config = if config_path.exists() {
        let cfg = ProjectConfig::load(&config_path)?;
        println!("Using config from {}", config_path.display());
        Some(cfg)
    } else {
        None
    };

    // Set up progress display (on stderr so stdout stays clean for hooks)
    let is_terminal = std::io::IsTerminal::is_terminal(&std::io::stderr());
    let progress_reporter: std::sync::Arc<dyn myceliums_core::progress::ProgressReporter> =
        if is_terminal {
            std::sync::Arc::new(IndicatifReporter::new())
        } else {
            std::sync::Arc::new(myceliums_core::SilentReporter)
        };

    if is_terminal {
        // Don't print "Analyzing..." when progress bar is active
    } else {
        println!("Analyzing {} ...", abs_path.display());
    }

    // Unchanged symbols keep their vectors: a run that does not embed
    // (--skip-embeddings, the session default, or a model that fails) must
    // not drop the embeddings of the index it replaces (#151).
    // Only at the current vector geometry, as the registry records it, and
    // only from an index that exists (read, never created here).
    let geometry = RepoRegistry::load(&registry_path())?
        .get(&repo_id)
        .map(|info| info.vector_geometry_version);
    let previous = match (geometry, Store::open_existing(&db_path, &repo_id).await) {
        (Some(geometry), Ok(live)) => myceliums_core::PreviousVectors::load(&live, geometry)
            .await
            .unwrap_or_else(|e| {
                eprintln!("Warning: cannot read the previous index's vectors: {e}");
                None
            }),
        _ => None,
    };
    if !skip_embeddings {
        let section = project_config
            .as_ref()
            .map(|c| c.embedding.clone())
            .unwrap_or_default();
        if let Some(notice) = first_index_notice(&section, previous.as_ref()) {
            println!("{notice}");
        }
    }
    // The new index is built next to the live one and swapped in once it is
    // complete (#175), as the MCP `analyze` tool does: readers keep the
    // previous index, whole, until the swap, and a failed run leaves it as
    // it was. It used to be emptied and rebuilt in place.
    let staged = myceliums_core::StagedIndex::create(&db_path)?;
    let store = Store::open(staged.path(), &repo_id).await?;
    let community_plan = CommunityPlan::new(
        project_config.as_ref().map(|c| &c.community),
        skip_communities,
        "`--skip-communities`",
    );

    let analyzer = if let Some(cfg) = project_config {
        Analyzer::with_config(store, abs_path.clone(), cfg)
    } else {
        Analyzer::new(store, abs_path.clone())
    }
    .set_skip_embeddings(skip_embeddings)
    .with_previous_vectors(previous)
    .with_progress(progress_reporter);
    let result = analyzer.analyze().await?;
    drop(analyzer);

    println!("  Symbols:       {}", result.symbol_count);
    println!("  Files:         {}", result.file_count);
    println!("  Relationships: {}", result.relationship_count);
    // How many call sites got an edge, and why the others did not (#139).
    println!("  Calls:         {}", result.call_resolution.summary());
    if let Some(skipped) = result.skip_summary() {
        println!("  Skipped:       {skipped}");
    }
    let kept = if result.embeddings_reused > 0 {
        format!(
            ", {} kept from the previous index",
            result.embeddings_reused
        )
    } else {
        String::new()
    };
    if skip_embeddings {
        println!(
            "  Embeddings:    {} (skipped{kept}; {}/{} symbols have a vector)",
            result.embedding_count, result.symbols_embedded, result.symbols_total
        );
    } else {
        println!(
            "  Embeddings:    {} ({}/{} symbols{kept})",
            result.embedding_count, result.symbols_embedded, result.symbols_total
        );
        if result.embedding_failures > 0 {
            // Impossible to miss: a partial index degrades search silently, so
            // shout about it here.
            eprintln!(
                "  ⚠ Embedding failures: {} — {} of {} symbols have no vector \
                 and are invisible to semantic and hybrid search",
                result.embedding_failures, result.symbols_embedded, result.symbols_total
            );
        }
    }
    if result.mentions_count > 0 {
        println!("  Mentions:      {}", result.mentions_count);
    }

    // Phase 2: community detection and process tracing, timed so a stall
    // shows which phase it is in (#172).
    let store = Store::open(staged.path(), &repo_id).await?;
    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;

    if let CommunityPlan::Run(options) = &community_plan {
        eprintln!(
            "Detecting communities (budget {})...",
            options
                .budget
                .time
                .map_or("unlimited".to_string(), |t| format!("{} s", t.as_secs()))
        );
    }
    let communities_started = std::time::Instant::now();
    let detection = community_plan
        .execute(&store, &symbols, &relationships, &repo_id)
        .await?;
    let communities_ms = communities_started.elapsed().as_secs_f64() * 1000.0;

    let processes_started = std::time::Instant::now();
    let processes = ProcessTracer::trace(&symbols, &relationships, &repo_id)?;
    let process_count = store.store_processes(&processes).await?;
    let processes_ms = processes_started.elapsed().as_secs_f64() * 1000.0;
    // The swap: the complete new index replaces the previous one.
    drop(store);
    staged.commit()?;

    println!(
        "  Communities:   {} ({})",
        detection.communities.len(),
        detection.status.state
    );
    if let Some(notice) = detection.status.notice() {
        eprintln!("  ⚠ {notice}");
    }
    println!("  Processes:     {}", process_count);

    if let Some(timing) = result.timing.clone() {
        let timing = timing.with_graph_phases(communities_ms, processes_ms);
        let embed = skip_embeddings.then_some("skipped");
        println!("  Phases:        {}", timing.phase_line(embed));
        println!();
        println!("{}", timing.format_report());
    }

    // Get current git commit for cache tracking
    let analyzed_commit = cache::get_head_commit(&abs_path).ok();

    // Record this analysis for drift comparisons (detect_architecture_drift).
    let snapshot = myceliums_core::build_snapshot(&repo_id, &symbols, &relationships);
    if let Err(e) =
        myceliums_core::record_snapshot(&data_dir(), &snapshot, analyzed_commit.as_deref())
    {
        eprintln!("Warning: failed to save snapshot: {}", e);
    }

    // Update registry
    let mut registry = RepoRegistry::load(&registry_path())?;
    registry.register(RepoInfo {
        id: repo_id.clone(),
        name: repo_name,
        path: abs_path.to_string_lossy().to_string(),
        analyzed_at: chrono::Utc::now().to_rfc3339(),
        symbol_count: result.symbol_count as u32,
        file_count: result.file_count as u32,
        analyzed_commit,
        vector_geometry_version: myceliums_storage::schema::VECTOR_GEOMETRY_VERSION,
        classification_version: myceliums_storage::schema::SYMBOL_CLASSIFICATION_VERSION,
        call_record_version: myceliums_storage::schema::CALL_RECORD_VERSION,
    });
    registry.save()?;

    println!();
    println!("Repository ID: {}", repo_id);

    // Strict mode (CI): a partial index is a build failure, not a warning.
    if embedding_policy.is_strict() && result.has_embedding_failures() {
        anyhow::bail!(
            "strict-embeddings: {} of {} symbols failed to embed; the index is \
             partial and would silently degrade search",
            result.embedding_failures,
            result.symbols_total
        );
    }
    Ok(())
}

/// Fail when a cached index is not fully embedded.
///
/// `--strict-embeddings` asserts a property of the index that is *used*, not
/// merely of the run that happened to build it. A fresh cache short-circuits
/// analysis, so the assertion is re-checked against the accounting persisted in
/// the index. An index written before embedding accounting existed has no
/// record; that is unverifiable rather than clean, so strict mode rejects it and
/// points at the re-analysis that would produce a verdict.
pub(crate) async fn enforce_strict_embeddings(db_path: &Path, repo_id: &str) -> Result<()> {
    let store = Store::open_existing(db_path, repo_id).await?;
    match myceliums_core::EmbeddingStats::load(&store).await? {
        // Reuse the canonical wording rather than restating it: this message
        // and EmbeddingStats::partial_index_warning had already drifted apart
        // once, which is what #48 reported.
        Some(stats) if stats.is_partial() => anyhow::bail!(
            "strict-embeddings: cached index is partial — {}. Re-run with --force to rebuild.",
            stats
                .partial_index_warning()
                .expect("is_partial() => warning")
        ),
        Some(_) => Ok(()),
        None => anyhow::bail!(
            "strict-embeddings: cached index has no embedding accounting, so its \
             coverage cannot be verified (it predates embedding accounting or was \
             built with --skip-embeddings). Re-run with --force to rebuild."
        ),
    }
}

/// Analyze a repository for interactive session startup (the `myc` session
/// bootstrap and auto-analyze hooks).
///
/// Session startup trades embedding coverage for speed: it forces a fresh index
/// and, by default, skips embedding generation entirely (`myc analyze . --force`
/// adds semantic vectors later). That default is a *default*, not a law — when
/// the caller asks for strict embeddings, the session path must generate
/// vectors and enforce full coverage like any other analysis. Hardcoding the
/// policy here is what made `--strict-embeddings` a silent no-op (#51).
pub(crate) async fn cmd_analyze_for_session(
    path: &Path,
    embedding_policy: EmbeddingPolicy,
) -> Result<()> {
    const SESSION_FORCE_REANALYZE: bool = true;
    const SESSION_CACHE_MAX_AGE_MINUTES: u64 = 60;

    cmd_analyze(
        path,
        SESSION_FORCE_REANALYZE,
        SESSION_CACHE_MAX_AGE_MINUTES,
        embedding_policy,
        false,
    )
    .await
}

/// What `--watch` does with one batch of file events.
#[derive(Debug, PartialEq, Eq)]
enum WatchPlan {
    /// Re-analyze everything: a changed or removed path decides what is
    /// indexed or what imports bind.
    Full,
    /// Re-index these changed files incrementally.
    Changed(Vec<PathBuf>),
    /// Drop these removed files' rows.
    Removed(Vec<PathBuf>),
}

/// The plan for `event`. A change *or removal* of `.myceliums.toml`, a
/// `package.json`, a tsconfig or another file that decides what JS/TS
/// imports bind needs a full analysis (#169): incremental re-indexing reads
/// source files only, and a removal only drops the file's own rows, so the
/// edges through those imports would stay as they were.
async fn plan_watch_event(
    analyzer: &Analyzer,
    event: myceliums_core::watch::WatchEvent,
) -> WatchPlan {
    use myceliums_core::watch::WatchEvent;
    let (paths, removed) = match event {
        WatchEvent::FilesChanged(paths) => (paths, false),
        WatchEvent::FilesRemoved(paths) => (paths, true),
    };
    if analyzer.needs_full_analysis(&paths).await {
        WatchPlan::Full
    } else if removed {
        WatchPlan::Removed(paths)
    } else {
        WatchPlan::Changed(paths)
    }
}

/// The project's `.myceliums.toml`, when there is a readable one.
fn watched_config(abs_path: &Path) -> Option<ProjectConfig> {
    let config_path = abs_path.join(config::CONFIG_FILENAME);
    config_path
        .exists()
        .then(|| ProjectConfig::load(&config_path).ok())
        .flatten()
}

pub(crate) async fn cmd_watch(
    path: &Path,
    skip_embeddings: bool,
    skip_communities: bool,
) -> Result<()> {
    use myceliums_core::watch::start_watching;

    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;
    let repo_id = analyzer::repo_id_from_path(&abs_path);
    RepoRegistry::validate_repo_id(&repo_id)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);

    // Reloaded after every full analysis: `.myceliums.toml` may have changed.
    let mut project_config = watched_config(&abs_path);

    println!(
        "Watching {} for changes (Ctrl+C to stop)...",
        abs_path.display()
    );

    let (mut rx, _debouncer) = start_watching(&abs_path)?;

    while let Some(event) = rx.recv().await {
        let store = Store::open(&db_path, &repo_id).await?;
        let a = if let Some(ref cfg) = project_config {
            Analyzer::with_config(store, abs_path.clone(), cfg.clone())
        } else {
            Analyzer::new(store, abs_path.clone())
        }
        .set_skip_embeddings(skip_embeddings);
        let relative = |paths: &[PathBuf]| -> String {
            paths
                .iter()
                .filter_map(|p| p.strip_prefix(&abs_path).ok())
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        match plan_watch_event(&a, event).await {
            WatchPlan::Full => {
                drop(a);
                println!(
                    "  A file that decides what is indexed or what imports bind changed: full re-analysis"
                );
                let policy = EmbeddingPolicy::from_flags(skip_embeddings, false);
                if let Err(e) = cmd_analyze(&abs_path, true, 0, policy, skip_communities).await {
                    eprintln!("    Error during full re-analysis: {e}");
                }
                project_config = watched_config(&abs_path);
            }
            WatchPlan::Changed(paths) => {
                println!(
                    "  Re-indexing {} changed file(s): {}",
                    paths.len(),
                    relative(&paths)
                );
                match a.analyze_files(&paths).await {
                    Ok(result) => {
                        println!(
                            "    Updated: {} symbols, {} files, {} relationships",
                            result.symbol_count, result.file_count, result.relationship_count
                        );
                    }
                    Err(e) => {
                        eprintln!("    Error during incremental analysis: {}", e);
                    }
                }
            }
            WatchPlan::Removed(paths) => {
                println!(
                    "  Removing {} deleted file(s): {}",
                    paths.len(),
                    relative(&paths)
                );
                for p in &paths {
                    if let Err(e) = a.handle_file_deleted(p).await {
                        eprintln!("    Error removing file data: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #169: in `--watch`, changing or deleting a file that decides what is
    /// indexed or what imports bind re-analyzes everything; other changes
    /// and deletions stay incremental.
    #[tokio::test]
    async fn watch_events_on_resolution_configs_need_a_full_analysis() {
        use myceliums_core::watch::WatchEvent;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(root.join("src")).unwrap();
        let store = Store::open(&dir.path().join("db"), "watch-test")
            .await
            .unwrap();
        let analyzer = Analyzer::new(store, root.clone());
        let at = |rel: &str| vec![root.join(rel)];
        for (event, full) in [
            (WatchEvent::FilesRemoved(at("package.json")), true),
            (WatchEvent::FilesRemoved(at("apps/web/tsconfig.json")), true),
            (WatchEvent::FilesRemoved(at(config::CONFIG_FILENAME)), true),
            (WatchEvent::FilesChanged(at("tsconfig.base.json")), true),
            (WatchEvent::FilesChanged(at(config::CONFIG_FILENAME)), true),
            (WatchEvent::FilesRemoved(at("src/a.ts")), false),
            (WatchEvent::FilesChanged(at("src/a.ts")), false),
        ] {
            let label = format!("{event:?}");
            let plan = plan_watch_event(&analyzer, event).await;
            assert_eq!(plan == WatchPlan::Full, full, "{label}: {plan:?}");
        }
        assert_eq!(
            plan_watch_event(&analyzer, WatchEvent::FilesRemoved(at("src/a.ts"))).await,
            WatchPlan::Removed(at("src/a.ts"))
        );
    }
    use crate::cli::{Cli, Commands};
    use clap::Parser;
    use myceliums_storage::Store;
    use tempfile::TempDir;

    /// A run that embeds from scratch says so — model, longer first index,
    /// the fast option — and a run whose previous vectors carry over is quiet.
    #[test]
    fn the_first_index_notice_names_the_model_and_the_fast_option() {
        let section = myceliums_core::config::EmbeddingSection::default();
        let notice = first_index_notice(&section, None).expect("nothing to reuse");
        assert!(notice.contains("jina-embeddings-v2-base-code"), "{notice}");
        assert!(notice.contains("reuse the vectors"), "{notice}");
        assert!(notice.contains("multilingual-e5-small"), "{notice}");
        let fast = myceliums_core::config::EmbeddingSection {
            model: "multilingual-e5-small".to_string(),
            chunking: Some(false),
            ..Default::default()
        };
        let notice = first_index_notice(&fast, None).unwrap();
        assert!(!notice.contains("faster first index"), "{notice}");
    }

    /// An index embedded with another model or template than the project
    /// now configures is re-analyzed instead of reused (#157); the same
    /// embedder, a skipped embedding run and an index without an embedder
    /// record are left to the freshness check.
    #[tokio::test]
    async fn an_embedder_change_needs_a_re_analysis() {
        use myceliums_core::IndexEmbeddingMeta;
        let root = TempDir::new().unwrap();
        let db = root.path().join("db");
        let store = Store::open(&db, "r").await.unwrap();
        store
            .store_symbols(&[myceliums_storage::CodeSymbol {
                uid: "u".into(),
                name: "f".into(),
                qualified_name: "f".into(),
                kind: myceliums_storage::SymbolKind::Function,
                file_path: "a.rs".into(),
                start_line: 1,
                end_line: 1,
                signature: "fn f()".into(),
                content: "fn f() {}".into(),
                repo_id: "r".into(),
                metadata: None,
            }])
            .await
            .unwrap();
        assert_eq!(embedder_change(root.path(), &db, "r", false).await, None);

        let write_config = |model: &str| {
            std::fs::write(
                root.path().join(config::CONFIG_FILENAME),
                format!("[embedding]\nmodel = \"{model}\"\n"),
            )
            .unwrap();
        };
        write_config("multilingual-e5-small");
        let section = ProjectConfig::load(&root.path().join(config::CONFIG_FILENAME))
            .unwrap()
            .embedding;
        let recorded = IndexEmbeddingMeta::from_config(&section).unwrap();
        store
            .set_index_meta(
                IndexEmbeddingMeta::META_KEY,
                &serde_json::to_string(&recorded).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(embedder_change(root.path(), &db, "r", false).await, None);

        write_config("granite-embedding-small-english-r2");
        let change = embedder_change(root.path(), &db, "r", false)
            .await
            .expect("a model change needs a re-analysis");
        assert!(change.contains("multilingual-e5-small"), "{change}");
        assert!(
            change.contains("granite-embedding-small-english-r2"),
            "{change}"
        );
        // A run that does not embed has nothing to redo.
        assert_eq!(embedder_change(root.path(), &db, "r", true).await, None);
    }

    #[test]
    fn test_safeguard_rejects_home_directory() {
        let home = dirs::home_dir().expect("need home dir for test");
        let result = check_session_safeguards(&home, false);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("home directory"),
            "Expected home directory error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_safeguard_rejects_non_git_directory() {
        let tmp = TempDir::new().unwrap();
        let result = check_session_safeguards(tmp.path(), false);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains(".git"),
            "Expected git error, got: {}",
            err_msg
        );
    }

    #[test]
    fn test_safeguard_allows_non_git_with_override() {
        let tmp = TempDir::new().unwrap();
        let result = check_session_safeguards(tmp.path(), true);
        assert!(result.is_ok());
    }

    #[test]
    fn test_safeguard_allows_git_directory() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join(".git")).unwrap();
        let result = check_session_safeguards(tmp.path(), false);
        assert!(result.is_ok());
    }

    #[test]
    fn test_is_dangerous_ancestor() {
        // Test known dangerous ancestors
        assert!(is_dangerous_ancestor(Path::new("/")));
        assert!(is_dangerous_ancestor(Path::new("/Users")));
        assert!(is_dangerous_ancestor(Path::new("/home")));

        // Test that regular paths are not dangerous
        assert!(!is_dangerous_ancestor(Path::new("/Users/marc")));
        assert!(!is_dangerous_ancestor(Path::new("/home/user")));
        assert!(!is_dangerous_ancestor(Path::new("/var/tmp")));
    }

    #[test]
    fn test_is_home_ancestor() {
        // Create temporary paths for testing
        let tmp = TempDir::new().unwrap();
        let home = tmp.path();

        // Parent should be detected as ancestor
        if let Some(parent) = home.parent() {
            assert!(is_home_ancestor(parent, home));
        }

        // Home itself should not be ancestor of itself
        assert!(!is_home_ancestor(home, home));

        // Non-parent should not be ancestor
        let other = TempDir::new().unwrap();
        assert!(!is_home_ancestor(other.path(), home));
    }

    #[test]
    fn test_safeguard_nonexistent_path_fails() {
        let result = check_session_safeguards(Path::new("/nonexistent/path/abcdef"), false);
        // Should fail (fail closed on canonicalize error)
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("canonicalize"),
            "Expected canonicalize error, got: {}",
            err_msg
        );
    }

    #[test]
    fn strict_flag_resolves_to_strict_policy() {
        let policy = EmbeddingPolicy::from_flags(false, true);
        assert_eq!(policy, EmbeddingPolicy::EmbedStrict);
        assert!(policy.is_strict());
        assert!(!policy.skips_embeddings());
    }

    #[test]
    fn plain_analyze_embeds_without_strictness() {
        let policy = EmbeddingPolicy::from_flags(false, false);
        assert_eq!(policy, EmbeddingPolicy::Embed);
        assert!(!policy.is_strict());
        assert!(!policy.skips_embeddings());
    }

    #[test]
    fn skip_flag_disables_embedding_and_strictness() {
        let policy = EmbeddingPolicy::from_flags(true, false);
        assert_eq!(policy, EmbeddingPolicy::Skip);
        assert!(policy.skips_embeddings());
        assert!(!policy.is_strict());
    }

    /// `--skip-embeddings` and `--strict-embeddings` are mutually exclusive at
    /// the CLI layer; if they ever reach the resolver together, skipping wins
    /// rather than silently claiming a verified index.
    #[test]
    fn skip_wins_over_strict_when_both_are_set() {
        assert_eq!(
            EmbeddingPolicy::from_flags(true, true),
            EmbeddingPolicy::Skip
        );
    }

    /// clap must reject the contradictory flag pair outright, so the conflict
    /// never becomes a silent precedence decision.
    #[test]
    fn cli_rejects_skip_combined_with_strict() {
        let parsed = Cli::try_parse_from([
            "myc",
            "analyze",
            ".",
            "--skip-embeddings",
            "--strict-embeddings",
        ]);
        assert!(
            parsed.is_err(),
            "--skip-embeddings with --strict-embeddings must be rejected"
        );
    }

    /// #172: `--skip-communities` reaches the analyze command, and becomes a
    /// skip plan that names the flag.
    #[test]
    fn skip_communities_flag_parses_into_a_skip_plan() {
        let parsed = Cli::try_parse_from(["myc", "analyze", ".", "--skip-communities"]).unwrap();
        let Commands::Analyze {
            skip_communities, ..
        } = parsed.command
        else {
            panic!("not analyze");
        };
        assert!(skip_communities);
        assert_eq!(
            CommunityPlan::new(None, skip_communities, "`--skip-communities`"),
            CommunityPlan::Skip("`--skip-communities`".into())
        );
        assert!(matches!(
            CommunityPlan::new(None, false, "`--skip-communities`"),
            CommunityPlan::Run(_)
        ));
    }

    /// The bug in one assertion: `myc session --strict-embeddings` must not
    /// resolve to the skip-everything policy that made the flag a no-op.
    #[test]
    fn session_honors_strict_embeddings_flag() {
        let parsed = Cli::try_parse_from(["myc", "session", "--strict-embeddings"])
            .expect("session must accept --strict-embeddings");
        let Commands::Session {
            strict_embeddings, ..
        } = parsed.command
        else {
            panic!("expected the session subcommand");
        };
        assert!(strict_embeddings, "flag must reach the session command");

        let policy = EmbeddingPolicy::for_session(strict_embeddings);
        assert_eq!(policy, EmbeddingPolicy::EmbedStrict);
        assert!(
            policy.is_strict(),
            "strict session bootstrap must enforce full coverage"
        );
        assert!(
            !policy.skips_embeddings(),
            "strict session bootstrap must generate vectors, not skip them"
        );
    }

    /// Session startup stays fast by default: no flag means no embeddings.
    #[test]
    fn session_skips_embeddings_by_default() {
        let parsed =
            Cli::try_parse_from(["myc", "session"]).expect("session must parse without flags");
        let Commands::Session {
            strict_embeddings, ..
        } = parsed.command
        else {
            panic!("expected the session subcommand");
        };
        assert!(!strict_embeddings);

        let policy = EmbeddingPolicy::for_session(strict_embeddings);
        assert_eq!(policy, EmbeddingPolicy::Skip);
        assert!(policy.skips_embeddings());
    }

    /// An index with no persisted accounting cannot be certified as complete,
    /// so strict mode must fail rather than assume success.
    #[tokio::test]
    async fn strict_mode_rejects_cached_index_without_accounting() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("db");
        // Materialize an index that has never recorded embedding accounting.
        Store::open(&db_path, "repo-under-test").await.unwrap();

        let err = enforce_strict_embeddings(&db_path, "repo-under-test")
            .await
            .expect_err("unverifiable coverage must fail strict mode");
        let msg = err.to_string();
        assert!(
            msg.contains("strict-embeddings"),
            "error must name the flag it enforces, got: {msg}"
        );
        assert!(
            msg.contains("--force"),
            "error must tell the user how to recover, got: {msg}"
        );
    }

    /// A fully-embedded cached index satisfies strict mode without re-analysis.
    #[tokio::test]
    async fn strict_mode_accepts_fully_embedded_cached_index() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("db");
        let store = Store::open(&db_path, "repo-under-test").await.unwrap();
        myceliums_core::EmbeddingStats::complete(12, 12)
            .record(&store)
            .await
            .unwrap();

        enforce_strict_embeddings(&db_path, "repo-under-test")
            .await
            .expect("a complete index must pass strict mode");
    }

    /// The silent-success path: reusing a partial index under `--strict-embeddings`
    /// previously exited 0. It must now fail and say how bad the coverage is.
    #[tokio::test]
    async fn strict_mode_rejects_partial_cached_index() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("db");
        let store = Store::open(&db_path, "repo-under-test").await.unwrap();
        myceliums_core::EmbeddingStats {
            symbols_total: 10,
            symbols_embedded: 4,
            embedding_failures: 6,
        }
        .record(&store)
        .await
        .unwrap();

        let err = enforce_strict_embeddings(&db_path, "repo-under-test")
            .await
            .expect_err("a partial cached index must fail strict mode");
        let msg = err.to_string();
        assert!(
            msg.contains("4 of 10"),
            "error must quantify coverage: {msg}"
        );
        assert!(
            msg.contains("6 embedding failures"),
            "error must report the failure count: {msg}"
        );
    }
}
