//! `myc communities`.

use crate::repo::{data_dir, resolve_repo};
use anyhow::Result;
use myceliums_storage::{RepoRegistry, Store};

pub(crate) async fn cmd_communities(repo: &str) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let communities = store.get_communities().await?;

    if communities.is_empty() {
        println!("No communities detected. Try re-analyzing with: myc analyze <path>");
        return Ok(());
    }

    println!("Communities in {}:", repo_info.name);
    println!();

    for (i, comm) in communities.iter().enumerate() {
        println!(
            "  {}. {} ({} members)",
            i + 1,
            comm.label,
            comm.member_count
        );
        if !comm.top_symbols.is_empty() {
            println!("     Top symbols: {}", comm.top_symbols);
        }
        if !comm.summary.is_empty() {
            println!("     {}", comm.summary);
        }
    }

    Ok(())
}
