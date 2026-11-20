//! `myc impact`.

use crate::repo::{graph_service, resolve_repo_or_latest};
use anyhow::{Context, Result};
use myceliums_core::{detect_impact, run_git_diff};

pub(crate) async fn cmd_impact(
    repo: Option<&str>,
    depth: u32,
    diff: Option<&str>,
    json: bool,
) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let diff_text = match diff {
        Some(d) => {
            let path = std::path::Path::new(d);
            if path.exists() && path.is_file() {
                std::fs::read_to_string(path)
                    .with_context(|| format!("Failed to read diff file: {}", d))?
            } else {
                d.to_string()
            }
        }
        None => run_git_diff(&repo_info.path)
            .with_context(|| format!("Failed to run git diff in {}", repo_info.path))?,
    };

    if diff_text.trim().is_empty() && !json {
        println!("No changes detected (empty diff).");
        return Ok(());
    }

    let symbols = graph.symbols().await?;
    let relationships = graph.relationships().await?;

    let report = detect_impact(&diff_text, &symbols, &relationships, depth);
    if json {
        let mut value = serde_json::to_value(&report)?;
        if let Some(obj) = value.as_object_mut() {
            obj.insert("repo".into(), serde_json::json!(repo_info.name));
        }
        println!("{value}");
        return Ok(());
    }

    println!("Impact Report for {}", repo_info.name);
    println!("{}", "=".repeat(60));

    if report.directly_changed.is_empty() {
        println!("\nNo symbols directly affected by the diff.");
    } else {
        println!(
            "\nDirectly changed symbols ({}):",
            report.directly_changed.len()
        );
        for sym in &report.directly_changed {
            println!(
                "  [{}] {} ({}) -- {}",
                sym.change_type, sym.qualified_name, sym.kind, sym.file_path
            );
        }
    }

    if !report.indirectly_affected.is_empty() {
        println!(
            "\nIndirectly affected symbols ({}):",
            report.indirectly_affected.len()
        );
        for sym in &report.indirectly_affected {
            println!(
                "  [{} at distance {}] {} ({}) -- {}",
                sym.relationship, sym.distance, sym.qualified_name, sym.kind, sym.file_path
            );
        }
    }

    if !report.affected_files.is_empty() {
        println!("\nAffected files ({}):", report.affected_files.len());
        for file in &report.affected_files {
            println!("  {}", file);
        }
    }

    println!("\nRisk score: {:.2} / 10.00", report.risk_score);
    Ok(())
}
