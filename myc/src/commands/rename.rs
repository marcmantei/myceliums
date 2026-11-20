//! `myc rename`.

use crate::repo::{graph_service, resolve_repo_or_latest};
use anyhow::Result;
use myceliums_core::RenamePlan;

pub(crate) async fn cmd_rename(
    symbol_name: &str,
    new_name: &str,
    repo: Option<&str>,
    apply: bool,
) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let symbols = graph.symbols().await?;
    let relationships = graph.relationships().await?;

    let plan = RenamePlan::create(&symbols, &relationships, symbol_name, new_name)?;

    if plan.edits.is_empty() {
        println!("No references found for '{}'.", symbol_name);
        return Ok(());
    }

    println!(
        "Rename Plan: '{}' -> '{}' ({} edit(s))",
        plan.symbol_name,
        plan.new_name,
        plan.edits.len()
    );
    println!("{}", "=".repeat(60));

    // Group edits by file for display
    let mut current_file = String::new();
    for edit in &plan.edits {
        if edit.file_path != current_file {
            println!();
            println!("  {}:", edit.file_path);
            current_file = edit.file_path.clone();
        }
        println!("    L{}: - {}", edit.line, edit.old_text.trim());
        println!("    L{}: + {}", edit.line, edit.new_text.trim());
    }

    println!();

    if apply {
        let applied = plan.apply()?;
        println!("Applied {} edit(s) across {} file(s).", applied, {
            let mut files: Vec<&str> = plan.edits.iter().map(|e| e.file_path.as_str()).collect();
            files.sort_unstable();
            files.dedup();
            files.len()
        });
    } else {
        println!("Preview only. Use --apply to modify files.");
    }

    Ok(())
}
