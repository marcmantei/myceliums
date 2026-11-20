//! `myc knowledge`.

use crate::repo::{graph_service, resolve_repo_or_latest};
use anyhow::Result;
use myceliums_core::search_symbols;
use std::collections::HashMap;

pub(crate) async fn cmd_knowledge(query: &str, repo: Option<&str>, limit: usize) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let symbols = graph.symbols().await?;
    let relationships = graph.relationships().await?;

    // Search for matching code symbols
    let search_results = search_symbols(&symbols, query);

    // Build UID lookup
    let uid_to_symbol: HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    // Find Mentions relationships targeting matched symbols
    let matched_uids: std::collections::HashSet<&str> = search_results
        .iter()
        .take(50)
        .map(|r| r.symbol.uid.as_str())
        .collect();

    let mention_rels: Vec<&myceliums_storage::Relationship> = relationships
        .iter()
        .filter(|r| {
            r.kind == myceliums_storage::RelationshipKind::Mentions
                && matched_uids.contains(r.target_uid.as_str())
        })
        .collect();

    println!("Knowledge Query: \"{}\"", query);
    println!("{}", "=".repeat(60));

    if mention_rels.is_empty() {
        println!("\nNo cross-domain mentions found for \"{}\".", query);
        println!("Tip: Run `myc analyze` on a directory containing both code and");
        println!("     content files (emails, docs, markdown) to build mentions.");
        return Ok(());
    }

    // Group by source symbol
    let mut by_source: HashMap<&str, Vec<&myceliums_storage::Relationship>> = HashMap::new();
    for rel in &mention_rels {
        by_source
            .entry(rel.source_uid.as_str())
            .or_default()
            .push(rel);
    }

    let total_mentions = mention_rels.len();
    let source_count = by_source.len();
    println!(
        "\nFound {} mention(s) across {} source(s):\n",
        total_mentions, source_count
    );

    let mut shown = 0;
    for (source_uid, rels) in &by_source {
        if shown >= limit {
            break;
        }
        if let Some(source) = uid_to_symbol.get(source_uid) {
            shown += 1;
            let kind_label = format!("{}", source.kind);
            println!(
                "  {}. {}: \"{}\" ({})",
                shown, kind_label, source.name, source.file_path
            );

            for rel in rels {
                if let Some(target) = uid_to_symbol.get(rel.target_uid.as_str()) {
                    println!(
                        "     → mentions: {} ({}) at {}:{}",
                        target.name, target.kind, target.file_path, target.start_line
                    );

                    // Parse metadata for context
                    if let Ok(meta) = serde_json::from_str::<
                        myceliums_core::mentions::MentionMetadata,
                    >(&rel.metadata)
                    {
                        if let Some(first_match) = meta.matches.first() {
                            println!("     Context: \"{}\"", first_match.context);
                            println!(
                                "     Source: {}, line {}",
                                source.file_path, first_match.line
                            );
                        }
                    }
                }
            }
            println!();
        }
    }

    Ok(())
}
