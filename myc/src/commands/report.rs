//! `myc report`.

use crate::repo::{data_dir, resolve_repo};
use anyhow::{Context, Result};
use myceliums_core::{compute_god_nodes, compute_surprising_connections};
use myceliums_storage::{RepoRegistry, Store};
use std::path::{Path, PathBuf};

pub(crate) async fn cmd_report(repo: &str, output: Option<&Path>) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let db_path = RepoRegistry::repo_db_path(&data_dir(), &repo_info.id);
    let store = Store::open(&db_path, &repo_info.id).await?;

    let symbols = store.get_symbols().await?;
    let relationships = store.get_relationships().await?;
    let communities = store.get_communities().await?;
    let processes = store.get_processes().await?;
    let files = store.get_files().await?;

    let god_nodes = compute_god_nodes(&symbols, &relationships, 10, 20);
    let surprising = compute_surprising_connections(&symbols, &relationships, 0.1, 50)?;

    let out_path = output
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("GRAPH_REPORT.md"));

    let now = chrono::Utc::now().format("%Y-%m-%d").to_string();

    let mut md = String::new();

    // ── Header ─────────────────────────────────────────────────────────
    md.push_str(&format!(
        "# Graph Report — {}\n\nGenerated: {}  |  Repo ID: `{}`\n\n",
        repo_info.name, now, repo_info.id
    ));

    // ── Overview ───────────────────────────────────────────────────────
    md.push_str("## Overview\n\n");
    md.push_str(&format!(
        "| Metric | Count |\n|--------|-------|\n\
         | Symbols | {} |\n\
         | Files | {} |\n\
         | Relationships | {} |\n\
         | Communities | {} |\n\
         | Processes | {} |\n\n",
        symbols.len(),
        files.len(),
        relationships.len(),
        communities.len(),
        processes.len(),
    ));

    // ── God Nodes ──────────────────────────────────────────────────────
    md.push_str("## God Nodes (Top 10 by Degree Centrality)\n\n");
    if god_nodes.is_empty() {
        md.push_str(
            "_No function calls detected. Graph structure limited to imports/containment._\n\n",
        );
    } else {
        md.push_str("| # | Name | Kind | Degree | In | Out | File | High Coupling |\n");
        md.push_str("|---|------|------|--------|----|-----|------|---------------|\n");
        for (i, n) in god_nodes.iter().enumerate() {
            md.push_str(&format!(
                "| {} | `{}` | {} | {} | {} | {} | `{}` | {} |\n",
                i + 1,
                n.name,
                n.kind,
                n.degree,
                n.in_degree,
                n.out_degree,
                n.file_path,
                if n.is_high_coupling {
                    "⚠️ yes"
                } else {
                    "no"
                },
            ));
        }
        md.push('\n');
        let high = god_nodes.iter().filter(|n| n.is_high_coupling).count();
        if high > 0 {
            md.push_str(&format!(
                "> **{} high-coupling node(s)** (degree > 20) detected. Consider splitting \
                 responsibilities or introducing abstraction layers.\n\n",
                high
            ));
        }
    }

    // ── Surprising Connections ─────────────────────────────────────────
    md.push_str("## Surprising Cross-Community Connections\n\n");
    if surprising.is_empty() {
        if communities.len() < 2 {
            md.push_str(
                "_All symbols are in a single community — no cross-community edges exist._\n\n",
            );
        } else {
            md.push_str("_No surprising connections found above the minimum threshold._\n\n");
        }
    } else {
        md.push_str(
            "Ranked by surprise score (1.0 = unique link between rarely-interacting communities):\n\n",
        );
        md.push_str("| # | Score | Source | Target | Communities |\n");
        md.push_str("|---|-------|--------|--------|-------------|\n");
        for (i, c) in surprising.iter().enumerate() {
            md.push_str(&format!(
                "| {} | {:.3} | `{}` | `{}` | {} → {} |\n",
                i + 1,
                c.surprise_score,
                c.source_name,
                c.target_name,
                c.source_community,
                c.target_community,
            ));
        }
        md.push('\n');
    }

    // ── Community Summary ──────────────────────────────────────────────
    md.push_str("## Community Summary\n\n");
    if communities.is_empty() {
        md.push_str("_No communities detected. Re-run `myc analyze` to generate them._\n\n");
    } else {
        md.push_str("| # | Label | Members | Top Symbols | Summary |\n");
        md.push_str("|---|-------|---------|-------------|----------|\n");
        for (i, c) in communities.iter().enumerate() {
            md.push_str(&format!(
                "| {} | {} | {} | {} | {} |\n",
                i + 1,
                c.label,
                c.member_count,
                c.top_symbols.replace('|', "\\|"),
                c.summary.replace('|', "\\|"),
            ));
        }
        md.push('\n');
    }

    // ── Suggested Queries ──────────────────────────────────────────────
    md.push_str("## Suggested Cypher Queries\n\n");
    md.push_str("Use `myc query` or the `cypher_query` MCP tool with these queries:\n\n");
    md.push_str("```cypher\n// Find all callers of a god node\nMATCH (caller)-[:CALLS]->(target {name: \"<god_node_name>\"})\nRETURN caller.name, caller.file_path\nORDER BY caller.name\n```\n\n");
    md.push_str("```cypher\n// List functions with the most outgoing calls\nMATCH (s)-[:CALLS]->()\nRETURN s.name, s.kind, s.file_path, count(*) AS out_degree\nORDER BY out_degree DESC\nLIMIT 20\n```\n\n");
    md.push_str("```cypher\n// Find all symbols in a specific file\nMATCH (s {file_path: \"<path/to/file>\"})\nRETURN s.name, s.kind, s.start_line\nORDER BY s.start_line\n```\n\n");
    md.push_str("```cypher\n// Find dead code (functions with no callers)\nMATCH (s)\nWHERE s.kind = \"Function\"\nAND NOT ()-[:CALLS]->(s)\nRETURN s.name, s.file_path\nLIMIT 50\n```\n");

    std::fs::write(&out_path, &md)
        .with_context(|| format!("Failed to write report to {}", out_path.display()))?;

    println!("Report written to {}", out_path.display());
    println!(
        "  {} symbols, {} communities, {} god nodes, {} surprising connections",
        symbols.len(),
        communities.len(),
        god_nodes.len(),
        surprising.len(),
    );

    Ok(())
}
