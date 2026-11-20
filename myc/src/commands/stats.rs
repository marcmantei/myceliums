//! `myc stats`.

use crate::repo::{graph_service, resolve_repo};
use anyhow::Result;

pub(crate) async fn cmd_stats(repo: &str) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let symbols = graph.symbols().await?;
    let files = graph.files().await?;
    let rels = graph.relationships().await?;
    let communities = graph.communities().await?;
    let processes = graph.processes().await?;

    println!("Repository: {}", repo_info.name);
    println!("Path:       {}", repo_info.path);
    println!("Analyzed:   {}", repo_info.analyzed_at);
    println!();
    println!("  Symbols:       {}", symbols.len());
    println!("  Files:         {}", files.len());
    println!("  Relationships: {}", rels.len());
    println!("  Communities:   {}", communities.len());
    println!("  Processes:     {}", processes.len());

    // Symbol breakdown
    let mut kind_counts = std::collections::HashMap::new();
    for sym in &symbols {
        *kind_counts.entry(sym.kind.to_string()).or_insert(0u32) += 1;
    }
    println!();
    println!("  Symbol breakdown:");
    let mut kinds: Vec<_> = kind_counts.into_iter().collect();
    kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (kind, count) in kinds {
        println!("    {:<15} {}", kind, count);
    }

    // Relationship breakdown
    let mut rel_counts = std::collections::HashMap::new();
    for rel in &rels {
        *rel_counts.entry(rel.kind.to_string()).or_insert(0u32) += 1;
    }
    if !rel_counts.is_empty() {
        println!();
        println!("  Relationship breakdown:");
        let mut rel_kinds: Vec<_> = rel_counts.into_iter().collect();
        rel_kinds.sort_by_key(|a| std::cmp::Reverse(a.1));
        for (kind, count) in rel_kinds {
            println!("    {:<15} {}", kind, count);
        }
    }

    // Language breakdown
    let mut lang_counts = std::collections::HashMap::new();
    for file in &files {
        *lang_counts.entry(file.language.clone()).or_insert(0u32) += 1;
    }
    println!();
    println!("  Languages:");
    let mut langs: Vec<_> = lang_counts.into_iter().collect();
    langs.sort_by_key(|a| std::cmp::Reverse(a.1));
    for (lang, count) in langs {
        println!("    {:<15} {} files", lang, count);
    }

    Ok(())
}
