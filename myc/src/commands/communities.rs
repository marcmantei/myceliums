//! `myc communities`.

use crate::repo::{graph_service, resolve_repo};
use anyhow::Result;

pub(crate) async fn cmd_communities(repo: &str) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let communities = graph.communities().await?;
    let notice = myceliums_core::CommunityStatus::load(&graph.store())
        .await
        .ok()
        .flatten()
        .and_then(|status| status.notice());

    if communities.is_empty() {
        match notice {
            Some(notice) => println!("{notice}"),
            None => println!("No communities detected. Try re-analyzing with: myc analyze <path>"),
        }
        return Ok(());
    }
    if let Some(notice) = &notice {
        println!("⚠ {notice}");
        println!();
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
