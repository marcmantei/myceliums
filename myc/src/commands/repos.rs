//! `myc repos`.

use crate::repo::{data_dir, lookup_repo_id, registry_path};
use crate::ui::{format_bytes, prompt_yes_no, truncate_str};
use anyhow::Result;
use myceliums_core::embedding_cache_info;
use myceliums_storage::RepoRegistry;
use std::path::{Path, PathBuf};

pub(crate) async fn cmd_list(json: bool) -> Result<()> {
    let registry = RepoRegistry::load(&registry_path())?;
    let repos = registry.list();

    if json {
        let json_repos: Vec<serde_json::Value> = repos
            .iter()
            .map(|r| {
                serde_json::json!({
                    "id": r.id,
                    "name": r.name,
                    "path": r.path,
                    "analyzed_at": r.analyzed_at,
                    "symbol_count": r.symbol_count,
                    "file_count": r.file_count,
                    "analyzed_commit": r.analyzed_commit,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json_repos)?);
        return Ok(());
    }

    if repos.is_empty() {
        println!("No repositories analyzed yet.");
        println!("Run: myc analyze <path>");
        return Ok(());
    }

    println!("{:<30} {:<10} {:<10} Path", "Name", "Symbols", "Files");
    println!("{}", "-".repeat(80));

    for repo in repos {
        println!(
            "{:<30} {:<10} {:<10} {}",
            repo.name, repo.symbol_count, repo.file_count, repo.path
        );
    }

    Ok(())
}

pub(crate) async fn cmd_delete(repo: &str) -> Result<()> {
    let mut registry = RepoRegistry::load(&registry_path())?;

    let repo_id = lookup_repo_id(&registry, repo)?;

    // Not while an analysis of it runs (CLI or MCP server): its swap or
    // in-place rebuild would bring the index back.
    use myceliums_core::lock::{AnalysisLock, LockOutcome};
    let _lock = match AnalysisLock::acquire(&data_dir(), &repo_id)? {
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::AlreadyRunning { pid } => anyhow::bail!(
            "cannot delete {repo_id}: an analysis of it is running (PID {pid}); \
             wait until it finishes, then delete again"
        ),
    };

    let info = registry.remove(&repo_id).unwrap();
    registry.save()?;

    // The index directory, or the link to its generation with the
    // generation (#175).
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_id);
    myceliums_core::remove_index(&db_path)?;

    println!("Deleted: {} ({})", info.name, repo_id);
    Ok(())
}

/// Compute the total size of a directory tree in bytes.
fn dir_size_bytes(path: &Path) -> u64 {
    walkdir::WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}

/// Find orphaned data directories (exist on disk but not in the registry).
pub(crate) fn find_orphans() -> Result<Vec<(String, PathBuf, u64)>> {
    let data_path = data_dir().join("data");
    if !data_path.exists() {
        return Ok(vec![]);
    }
    let registry = RepoRegistry::load(&registry_path()).unwrap_or_default();
    let mut orphans = Vec::new();
    // Leftovers of interrupted rebuilds whose process died are recovered
    // (an index restored, the rest deleted); those of running processes are
    // internal entries, as is the lock directory: never orphans.
    myceliums_core::recover_leftovers(&data_path);
    for entry in std::fs::read_dir(&data_path)? {
        let entry = entry?;
        // A directory, or a link to an index generation (#175).
        if entry.path().is_dir() {
            let dir_name = entry.file_name().to_string_lossy().to_string();
            if myceliums_core::is_internal_entry(&dir_name) {
                continue;
            }
            if registry.get(&dir_name).is_none() {
                let size = dir_size_bytes(&entry.path());
                orphans.push((dir_name, entry.path(), size));
            }
        }
    }
    Ok(orphans)
}

pub(crate) async fn cmd_status() -> Result<()> {
    let data = data_dir();

    println!("Myceliums Status");
    println!("{}", "=".repeat(60));
    println!();

    // Repositories
    let registry = RepoRegistry::load(&registry_path()).unwrap_or_default();
    let repos = registry.list();

    if repos.is_empty() {
        println!("  No repositories analyzed yet.");
    } else {
        println!(
            "  {:<24} {:>6} {:>8} {:>10}  Last Analyzed",
            "Name", "Files", "Symbols", "Size"
        );
        println!("  {}", "\u{2500}".repeat(74));

        let mut total_size = 0u64;
        for repo in &repos {
            let db_path = RepoRegistry::repo_db_path(&data, &repo.id);
            let size = if db_path.exists() {
                dir_size_bytes(&db_path)
            } else {
                0
            };
            total_size += size;

            let analyzed = repo
                .analyzed_at
                .split('T')
                .next()
                .unwrap_or(&repo.analyzed_at);
            println!(
                "  {:<24} {:>6} {:>8} {:>10}  {}",
                truncate_str(&repo.name, 24),
                repo.file_count,
                repo.symbol_count,
                format_bytes(size),
                analyzed,
            );
        }

        println!();
        println!(
            "  Total: {} repositories, {}",
            repos.len(),
            format_bytes(total_size)
        );
    }

    // Overall data directory
    println!();
    if data.exists() {
        let total_data_size = dir_size_bytes(&data);
        println!("  Data directory: {}", data.display());
        println!("  Total size:     {}", format_bytes(total_data_size));
    } else {
        println!("  Data directory: not created yet");
    }

    // Orphans
    let orphans = find_orphans()?;
    if !orphans.is_empty() {
        let orphan_total: u64 = orphans.iter().map(|(_, _, s)| s).sum();
        println!(
            "  Orphaned dirs:  {} ({})",
            orphans.len(),
            format_bytes(orphan_total)
        );
        for (name, _, size) in &orphans {
            println!("    - {} ({})", name, format_bytes(*size));
        }
    }

    // FastEmbed model cache
    {
        println!();
        let cache_info = embedding_cache_info();
        println!("  FastEmbed model cache:");
        println!("    Location: {}", cache_info.cache_dir.display());
        println!("    Size:     {}", format_bytes(cache_info.size_bytes));
        if cache_info.is_cached {
            println!("    Status:   cached");
        } else {
            println!("    Status:   not downloaded");
        }
    }

    println!();
    Ok(())
}

pub(crate) async fn cmd_clean(
    repo: Option<String>,
    orphans: bool,
    all: bool,
    cache: bool,
    auto_yes: bool,
) -> Result<()> {
    let data = data_dir();

    if all {
        if data.exists() {
            let total = dir_size_bytes(&data);
            println!("This will delete ALL myceliums data at {}", data.display());
            println!("Will free: {}", format_bytes(total));
            println!();
            if auto_yes || prompt_yes_no("Delete all myceliums data?")? {
                std::fs::remove_dir_all(&data)?;
                println!("Deleted all myceliums data.");
            } else {
                println!("Cancelled.");
            }
        } else {
            println!("No myceliums data directory found.");
        }
        return Ok(());
    }

    if orphans {
        let found = find_orphans()?;
        if found.is_empty() {
            println!("No orphaned data directories found.");
            return Ok(());
        }
        let orphan_total: u64 = found.iter().map(|(_, _, s)| s).sum();
        println!("Found {} orphaned data directories:", found.len());
        for (name, _, size) in &found {
            println!("  - {} ({})", name, format_bytes(*size));
        }
        println!();
        println!("Will free: {}", format_bytes(orphan_total));
        println!();
        if auto_yes || prompt_yes_no("Delete orphaned directories?")? {
            for (name, path, _) in &found {
                myceliums_core::remove_index(path)?;
                println!("  Deleted: {}", name);
            }
            println!("Done.");
        } else {
            println!("Cancelled.");
        }
        return Ok(());
    }

    if cache {
        let cache_info = embedding_cache_info();
        if cache_info.is_cached {
            println!(
                "FastEmbed model cache: {} ({})",
                cache_info.cache_dir.display(),
                format_bytes(cache_info.size_bytes)
            );
            if auto_yes || prompt_yes_no("Delete model cache?")? {
                std::fs::remove_dir_all(&cache_info.cache_dir)?;
                println!("Deleted model cache.");
            } else {
                println!("Cancelled.");
            }
        } else {
            println!("No model cache found.");
        }
        return Ok(());
    }

    if let Some(repo_ref) = repo {
        // Clean a specific repo (with confirmation, unlike bare delete)
        let registry = RepoRegistry::load(&registry_path())?;
        let repo_id = lookup_repo_id(&registry, &repo_ref)?;

        let db_path = RepoRegistry::repo_db_path(&data, &repo_id);
        let size = if db_path.exists() {
            dir_size_bytes(&db_path)
        } else {
            0
        };
        println!("Repository: {} ({})", repo_ref, format_bytes(size));
        if auto_yes || prompt_yes_no("Delete this repository's data?")? {
            cmd_delete(&repo_ref).await?;
        } else {
            println!("Cancelled.");
        }
        return Ok(());
    }

    println!("Usage:");
    println!("  myc clean <repo>     — delete a specific repository's data");
    println!("  myc clean --orphans  — remove orphaned data directories");
    println!("  myc clean --all      — remove ALL myceliums data");
    println!("  myc clean --cache    — remove fastembed model cache");
    println!();
    println!("Add --yes to skip confirmation prompts.");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    #[test]
    fn test_find_orphans_runs() {
        // Verify it doesn't panic
        let result = find_orphans();
        assert!(result.is_ok());
    }

    #[test]
    fn test_dir_size_bytes_empty() {
        let tmp = TempDir::new().unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 0);
    }

    #[test]
    fn test_dir_size_bytes_with_files() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "hello").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "world!").unwrap();
        assert_eq!(dir_size_bytes(tmp.path()), 11); // 5 + 6
    }

    // --- Embedding policy (#51) ---------------------------------------------
    //
    // The regression: `--strict-embeddings` was accepted by the CLI but the
    // session bootstrap hardcoded it off, so strict runs exited 0 on a partial
    // index. These tests pin the policy resolution and the cached-index gate.
}
