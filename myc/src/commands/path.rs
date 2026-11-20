//! `myc path`.

use crate::repo::{graph_service, resolve_repo_or_latest};
use anyhow::Result;

pub(crate) async fn cmd_path(
    from: &str,
    to: &str,
    repo: Option<&str>,
    max_depth: u32,
) -> Result<()> {
    let (repo_id, _info) = resolve_repo_or_latest(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_id).await?;

    let symbols = graph.symbols().await?;
    let relationships = graph.relationships().await?;

    // Resolve symbols using same pattern as MCP tool
    let from_symbol = resolve_symbol_cli(&symbols, from)?;
    let to_symbol = resolve_symbol_cli(&symbols, to)?;

    // Build bidirectional adjacency list across ALL relationship types
    let mut adjacency: std::collections::HashMap<&str, Vec<(&str, String)>> =
        std::collections::HashMap::new();
    for rel in &relationships {
        let edge_label = rel.kind.to_string();
        adjacency
            .entry(rel.source_uid.as_str())
            .or_default()
            .push((rel.target_uid.as_str(), edge_label.clone()));
        adjacency
            .entry(rel.target_uid.as_str())
            .or_default()
            .push((rel.source_uid.as_str(), format!("{}_REV", edge_label)));
    }

    let sym_by_uid: std::collections::HashMap<&str, &myceliums_storage::CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    // BFS
    let mut visited = std::collections::HashSet::new();
    let mut parent: std::collections::HashMap<&str, (&str, String)> =
        std::collections::HashMap::new();
    let mut queue = std::collections::VecDeque::new();

    queue.push_back((from_symbol.uid.as_str(), 0u32));
    visited.insert(from_symbol.uid.as_str());

    let mut found = false;
    while let Some((uid, depth)) = queue.pop_front() {
        if uid == to_symbol.uid.as_str() {
            found = true;
            break;
        }
        if depth >= max_depth {
            continue;
        }
        if let Some(neighbors) = adjacency.get(uid) {
            for (neighbor_uid, edge_type) in neighbors {
                if !visited.contains(neighbor_uid) {
                    visited.insert(neighbor_uid);
                    parent.insert(neighbor_uid, (uid, edge_type.clone()));
                    queue.push_back((neighbor_uid, depth + 1));
                }
            }
        }
    }

    if !found {
        println!(
            "No path found between '{}' and '{}' within {} hops.",
            from, to, max_depth
        );
        return Ok(());
    }

    // Reconstruct path
    let mut path_uids: Vec<(&str, String)> = Vec::new();
    let mut current = to_symbol.uid.as_str();
    while current != from_symbol.uid.as_str() {
        if let Some((prev, edge)) = parent.get(current) {
            path_uids.push((current, edge.clone()));
            current = prev;
        } else {
            break;
        }
    }
    path_uids.reverse();

    println!(
        "Shortest path from '{}' to '{}' ({} hops):\n",
        from_symbol.name,
        to_symbol.name,
        path_uids.len()
    );
    println!(
        " [start] {} ({}) {}:{}",
        from_symbol.name, from_symbol.kind, from_symbol.file_path, from_symbol.start_line,
    );
    for (i, (uid, edge)) in path_uids.iter().enumerate() {
        if let Some(sym) = sym_by_uid.get(uid) {
            println!("    --[{}]-->", edge);
            println!(
                " [{}] {} ({}) {}:{}",
                i + 1,
                sym.name,
                sym.kind,
                sym.file_path,
                sym.start_line,
            );
        }
    }

    Ok(())
}

fn resolve_symbol_cli<'a>(
    symbols: &'a [myceliums_storage::CodeSymbol],
    name: &str,
) -> Result<&'a myceliums_storage::CodeSymbol> {
    let matches: Vec<_> = symbols
        .iter()
        .filter(|s| s.qualified_name == name)
        .collect();
    if matches.len() == 1 {
        return Ok(matches[0]);
    }
    if matches.is_empty() {
        let by_name: Vec<_> = symbols.iter().filter(|s| s.name == name).collect();
        match by_name.len() {
            0 => anyhow::bail!("Symbol not found: {}", name),
            1 => Ok(by_name[0]),
            _ => {
                let names: Vec<_> = by_name.iter().map(|s| s.qualified_name.as_str()).collect();
                anyhow::bail!("Ambiguous symbol '{}'. Matches: {}", name, names.join(", "));
            }
        }
    } else {
        Ok(matches[0])
    }
}
