//! `myc processes`.

use crate::repo::{data_dir, resolve_repo};
use anyhow::Result;
use myceliums_core::ProcessFilter;
use myceliums_storage::{RepoRegistry, Store};

pub(crate) async fn cmd_processes(
    repo: &str,
    entry: Option<String>,
    filter: Option<String>,
    limit: Option<usize>,
    min_steps: Option<u32>,
) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let processes = store.get_processes().await?;

    if processes.is_empty() {
        println!("No processes detected. Try re-analyzing with: myc analyze <path>");
        return Ok(());
    }

    // Check if any filters are applied
    let has_filters = entry.is_some() || filter.is_some() || limit.is_some() || min_steps.is_some();

    // Apply filters
    let filter_obj = ProcessFilter {
        entry,
        filter,
        limit,
        min_steps,
    };
    let filtered = filter_obj.apply(&processes);

    if filtered.is_empty() {
        println!("No processes match filter criteria.");
        return Ok(());
    }

    println!("Processes in {}:", repo_info.name);
    if has_filters {
        println!("(filtered)");
    }
    println!();

    for (i, proc) in filtered.iter().enumerate() {
        println!(
            "  {}. {} ({} steps, entry: {})",
            i + 1,
            proc.name,
            proc.step_count,
            proc.entry_point
        );
        println!("     Flow: {}", proc.description);
        println!();
    }

    Ok(())
}
