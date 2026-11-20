//! `myc wiki`.

use crate::repo::{graph_service, resolve_repo};
use anyhow::Result;
use myceliums_core::{export_wiki, WikiExportConfig};
use std::path::Path;

pub(crate) async fn cmd_wiki(repo: &str, output: &Path, format: Option<&str>) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let symbols = graph.symbols().await?;
    let relationships = graph.relationships().await?;
    let communities = graph.communities().await?;

    let config = WikiExportConfig {
        obsidian_vault: format == Some("obsidian"),
        ..Default::default()
    };

    let result = export_wiki(
        &symbols,
        &relationships,
        &communities,
        &repo_info.name,
        output,
        &config,
    )?;

    eprintln!(
        "Exported {} communities, {} symbols, {} relationships to {}",
        result.community_count,
        result.symbol_count,
        result.relationship_count,
        output.display()
    );

    Ok(())
}
