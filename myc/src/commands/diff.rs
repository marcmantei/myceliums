//! `myc diff`.

use crate::repo::{data_dir, graph_service, resolve_repo};
use anyhow::Result;

pub(crate) async fn cmd_diff(repo: &str) -> Result<()> {
    let (repo_id, _repo_info) = resolve_repo(repo)?;

    // Build a snapshot from the current graph state
    let service = graph_service();
    let graph = service.graph(&repo_id).await?;
    let symbols = graph.symbols().await?;
    let relationships = graph.relationships().await?;
    let current = myceliums_core::build_snapshot(&repo_id, &symbols, &relationships);

    // The previous analysis, as the MCP drift tool picks it.
    let Some((_, previous)) = myceliums_core::previous_snapshot(&data_dir(), &repo_id, &current)?
    else {
        println!(
            "No earlier analysis of '{}' is recorded, so there is nothing to compare yet.\n\
             Run `myc analyze <path>` again after the code changes (each analysis records a snapshot).",
            repo_id
        );
        return Ok(());
    };

    // Diff
    let diff = myceliums_core::diff_snapshots(&previous, &current);

    // Print
    let total_changes = diff.added_symbols.len()
        + diff.removed_symbols.len()
        + diff.added_edges.len()
        + diff.removed_edges.len();

    if total_changes == 0 {
        println!(
            "No changes detected since the previous analysis ({}).",
            diff.previous_snapshot_at
        );
        return Ok(());
    }

    println!(
        "Graph diff for '{}' (snapshot {} -> now):\n",
        repo_id, diff.previous_snapshot_at
    );

    if !diff.added_symbols.is_empty() {
        println!("+ Added symbols ({}):", diff.added_symbols.len());
        for entry in &diff.added_symbols {
            println!("    + {}", entry.label);
        }
        println!();
    }

    if !diff.removed_symbols.is_empty() {
        println!("- Removed symbols ({}):", diff.removed_symbols.len());
        for entry in &diff.removed_symbols {
            println!("    - {}", entry.label);
        }
        println!();
    }

    if !diff.added_edges.is_empty() {
        println!("+ Added relationships ({}):", diff.added_edges.len());
        for entry in &diff.added_edges {
            println!("    + {}", entry.label);
        }
        println!();
    }

    if !diff.removed_edges.is_empty() {
        println!("- Removed relationships ({}):", diff.removed_edges.len());
        for entry in &diff.removed_edges {
            println!("    - {}", entry.label);
        }
        println!();
    }

    println!(
        "Summary: +{} -{} symbols, +{} -{} relationships",
        diff.added_symbols.len(),
        diff.removed_symbols.len(),
        diff.added_edges.len(),
        diff.removed_edges.len(),
    );

    Ok(())
}

// ── Email commands ───────────────────────────────────────────────────
