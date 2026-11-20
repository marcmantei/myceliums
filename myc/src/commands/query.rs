//! `myc query`.

use crate::repo::{graph_service, resolve_repo_or_latest};
use anyhow::Result;

pub(crate) async fn cmd_query(query: &str, repo: Option<&str>) -> Result<()> {
    let (_, repo_info) = resolve_repo_or_latest(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let executor = myceliums_cypher::CypherExecutor::from_shared(
        graph.symbols().await?.shared(),
        graph.files().await?.shared(),
        graph.relationships().await?.shared(),
    );
    let results = executor.execute(query)?;

    if results.is_empty() {
        println!("(no results)");
        return Ok(());
    }

    // Print as a table
    let keys: Vec<String> = results[0].keys().cloned().collect();
    let header = keys.join(" | ");
    println!("{}", header);
    println!("{}", "-".repeat(header.len().max(40)));

    for row in &results {
        let vals: Vec<String> = keys
            .iter()
            .map(|k| {
                let v = row.get(k).cloned().unwrap_or(serde_json::Value::Null);
                match v {
                    serde_json::Value::String(s) => s,
                    serde_json::Value::Null => "null".to_string(),
                    other => other.to_string(),
                }
            })
            .collect();
        println!("{}", vals.join(" | "));
    }

    println!();
    println!("{} row(s)", results.len());
    Ok(())
}
