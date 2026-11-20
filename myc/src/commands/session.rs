//! `myc session`. The `--yes` hook mode lives in [`super::session_hook`].

use super::session_hook as hook;
use crate::commands::analyze::{
    check_session_safeguards, cmd_analyze_for_session, enforce_strict_embeddings, EmbeddingPolicy,
};
use crate::repo::{data_dir, registry_path};
use crate::ui::prompt_yes_no;
use anyhow::{Context, Result};
use myceliums_core::analyzer::{self};
use myceliums_core::cache::{self, CacheCheckConfig, CacheDecision};
use myceliums_storage::RepoRegistry;
use std::path::Path;

/// Count supported source files in a directory (without parsing them).
pub(crate) fn count_source_files(path: &Path) -> usize {
    use walkdir::WalkDir;

    let supported_extensions = ["ts", "tsx", "js", "jsx", "py"];

    WalkDir::new(path)
        .into_iter()
        .filter_entry(|e| {
            let name = e.file_name().to_string_lossy();
            !name.starts_with('.')
                && name != "node_modules"
                && name != "__pycache__"
                && name != "target"
                && name != "dist"
                && name != "build"
                && name != ".venv"
                && name != "venv"
        })
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_type().is_file()
                && e.path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .map(|ext| supported_extensions.contains(&ext))
                    .unwrap_or(false)
        })
        .count()
}

/// Estimate analysis time based on file count.
fn estimate_time(file_count: usize) -> &'static str {
    match file_count {
        0 => "no supported files found",
        1..=50 => "~5 seconds",
        51..=100 => "~10-15 seconds",
        101..=500 => "~30-60 seconds",
        501..=1000 => "~1-3 minutes",
        1001..=3000 => "~5-10 minutes",
        _ => "~10-20 minutes",
    }
}

pub(crate) async fn cmd_session(
    path: &Path,
    auto_yes: bool,
    timeout_secs: u64,
    strict_embeddings: bool,
    no_git_check: bool,
) -> Result<()> {
    if auto_yes {
        // The Claude Code SessionStart hook: never an error, whatever happens.
        let report = hook::run(path, timeout_secs, strict_embeddings, no_git_check).await;
        hook::emit(&report);
        return Ok(());
    }

    // Session startup skips embeddings for speed unless the caller demands a
    // verified, fully-embedded index.
    let embedding_policy = EmbeddingPolicy::for_session(strict_embeddings);

    let abs_path = std::fs::canonicalize(path)
        .with_context(|| format!("Path does not exist: {}", path.display()))?;

    let project_name = abs_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // Apply safeguards (home dir, git repo)
    check_session_safeguards(path, no_git_check)?;

    let repo_id = analyzer::repo_id_from_path(&abs_path);
    RepoRegistry::validate_repo_id(&repo_id)?;

    // Interactive mode — verbose output
    println!();
    println!("  Myceliums Session — {}", project_name);
    println!("  {}", "─".repeat(40));

    // Check if analysis already exists and is fresh
    let registry = RepoRegistry::load(&registry_path())?;
    if let Some(repo_info) = registry.get(&repo_id) {
        let cache_config = CacheCheckConfig::default();
        match cache::check_cache(repo_info, &abs_path, &cache_config) {
            CacheDecision::UseCached { reason, .. } => {
                println!("  Cache:     fresh ({})", reason);
                println!("  Analyzed:  {}", repo_info.analyzed_at);
                println!("  Symbols:   {}", repo_info.symbol_count);
                println!("  Files:     {}", repo_info.file_count);
                println!();
                if embedding_policy.is_strict() {
                    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
                    enforce_strict_embeddings(&db_path, &repo_id).await?;
                }
                println!("  Ready. MCP tools available via 'myc mcp'.");
                println!();
                return Ok(());
            }
            CacheDecision::ReanalyzeNeeded { reason } => {
                println!("  Cache:     stale ({})", reason);
                println!(
                    "  Previous:  {} symbols, {} files",
                    repo_info.symbol_count, repo_info.file_count
                );
                println!();

                if !prompt_yes_no("  Re-analyze now?")? {
                    println!("  Skipped. Using stale cache.");
                    return Ok(());
                }

                // Embeddings follow the session policy (skipped for speed by default)
                cmd_analyze_for_session(&abs_path, embedding_policy).await?;
                println!();
                println!("  Ready. MCP tools available via 'myc mcp'.");
                println!();
                return Ok(());
            }
        }
    }

    // No analysis exists — count files and offer to create one
    let file_count = count_source_files(&abs_path);
    let estimate = estimate_time(file_count);

    println!("  Status:    no analysis found");
    println!("  Files:     {} supported source files", file_count);
    println!("  Estimate:  {}", estimate);
    println!();

    if file_count == 0 {
        println!("  No TypeScript, JavaScript, or Python files found.");
        println!("  Myceliums currently supports: .ts, .tsx, .js, .jsx, .py");
        println!();
        return Ok(());
    }

    if !prompt_yes_no("  Analyze now?")? {
        println!("  Skipped. Run 'myc analyze .' later to create the index.");
        return Ok(());
    }

    println!();
    // Embeddings follow the session policy (skipped for speed by default)
    cmd_analyze_for_session(&abs_path, embedding_policy).await?;

    println!();
    println!("  Ready. MCP tools available via 'myc mcp'.");
    println!("  Tip: Run 'myc analyze . --force' later to add semantic search embeddings.");
    println!();

    Ok(())
}
