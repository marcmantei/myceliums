//! `myc wiki`.

use crate::repo::{data_dir, resolve_repo};
use anyhow::Result;
use myceliums_core::{export_wiki, WikiExportConfig};
use myceliums_storage::{RepoRegistry, Store};
use std::path::Path;

pub(crate) async fn cmd_wiki(repo: &str, output: &Path, format: Option<&str>) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;
    let communities = store.get_communities().await?;

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
