//! `myc export`.

use crate::repo::{graph_service, resolve_repo};
use anyhow::{Context, Result};
use myceliums_core::{compute_uid_to_community_label, export_graphml, export_neo4j_cypher};
use myceliums_storage::RepoInfo;
use std::collections::HashMap;
use std::path::Path;

pub(crate) async fn cmd_export(
    repo: &str,
    output: Option<&Path>,
    format: &str,
    width: u32,
    height: u32,
) -> Result<()> {
    let (_, repo_info) = resolve_repo(repo)?;
    let service = graph_service();
    let graph = service.graph(&repo_info.id).await?;

    let symbols = graph.symbols().await?;
    let rels = graph.relationships().await?;
    let communities = graph.communities().await?;

    match format {
        "json" => export_json(&repo_info, &symbols, &rels, &communities, output),
        "graphml" => cli_export_graphml(&symbols, &rels, output),
        "cypher" | "neo4j" => cli_export_cypher(&symbols, &rels, output),
        "svg" => export_svg(&repo_info, &symbols, &rels, width, height, output),
        _ => anyhow::bail!(
            "Unknown format '{}'. Supported: json, graphml, cypher/neo4j, svg",
            format
        ),
    }
}

fn export_json(
    repo_info: &RepoInfo,
    symbols: &[myceliums_storage::models::CodeSymbol],
    rels: &[myceliums_storage::models::Relationship],
    communities: &[myceliums_storage::models::Community],
    output: Option<&Path>,
) -> Result<()> {
    let nodes: Vec<serde_json::Value> = symbols
        .iter()
        .map(|s| {
            serde_json::json!({
                "id": s.uid,
                "name": s.name,
                "qualified_name": s.qualified_name,
                "kind": s.kind.to_string(),
                "file_path": s.file_path,
                "start_line": s.start_line,
                "end_line": s.end_line,
            })
        })
        .collect();

    let edges: Vec<serde_json::Value> = rels
        .iter()
        .map(|r| {
            serde_json::json!({
                "source": r.source_uid,
                "target": r.target_uid,
                "kind": r.kind.to_string(),
            })
        })
        .collect();

    let community_list: Vec<serde_json::Value> = communities
        .iter()
        .map(|c| {
            serde_json::json!({
                "id": c.uid,
                "label": c.label,
                "member_count": c.member_count,
                "top_symbols": c.top_symbols,
                "summary": c.summary,
            })
        })
        .collect();

    let export = serde_json::json!({
        "metadata": {
            "repository": repo_info.name,
            "path": repo_info.path,
            "analyzed_at": repo_info.analyzed_at,
            "symbols": nodes.len(),
            "relationships": edges.len(),
            "communities": community_list.len(),
        },
        "nodes": nodes,
        "edges": edges,
        "communities": community_list,
    });

    let json = serde_json::to_string_pretty(&export)?;

    if let Some(path) = output {
        std::fs::write(path, &json)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges, {} communities to {}",
            nodes.len(),
            edges.len(),
            community_list.len(),
            path.display()
        );
    } else {
        println!("{}", json);
    }

    Ok(())
}

/// Escape a string for use inside XML attribute values or text content.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn cli_export_graphml(
    symbols: &[myceliums_storage::models::CodeSymbol],
    rels: &[myceliums_storage::models::Relationship],
    output: Option<&Path>,
) -> Result<()> {
    let xml = export_graphml(symbols, rels);
    if let Some(path) = output {
        std::fs::write(path, &xml)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges (GraphML) to {}",
            symbols.len(),
            rels.len(),
            path.display()
        );
    } else {
        println!("{}", xml);
    }
    Ok(())
}

fn svg_truncate(s: &str, max_chars: usize) -> String {
    let mut iter = s.chars();
    let truncated: String = iter.by_ref().take(max_chars).collect();
    if iter.next().is_some() {
        format!("{}…", truncated)
    } else {
        truncated
    }
}

fn kind_color(kind: &myceliums_storage::models::SymbolKind) -> &'static str {
    use myceliums_storage::models::SymbolKind;
    // Matches the website's dark theme color palette
    match kind {
        SymbolKind::Function => "#6366F1",     // indigo (accent)
        SymbolKind::Method => "#8B5CF6",       // violet
        SymbolKind::Class => "#EC4899",        // pink
        SymbolKind::Interface => "#06B6D4",    // cyan
        SymbolKind::TypeAlias => "#A855F7",    // purple
        SymbolKind::Variable => "#10B981",     // emerald (success)
        SymbolKind::Constant => "#F59E0B",     // amber (warning)
        SymbolKind::Enum => "#EF4444",         // red (danger)
        SymbolKind::Module => "#14B8A6",       // teal
        SymbolKind::Import => "#64748B",       // slate
        SymbolKind::Section => "#818CF8",      // accent-glow
        SymbolKind::Document => "#34D399",     // emerald-light
        SymbolKind::Rationale => "#FBBF24",    // yellow
        SymbolKind::Email => "#EC7063",        // coral (warm red)
        SymbolKind::Conversation => "#5DADE2", // bright blue
        SymbolKind::Person => "#AF7AC5",       // orchid (light purple)
        SymbolKind::Attachment => "#F8B88B",   // peach
    }
}

fn force_directed_layout(
    n: usize,
    edges: &[(usize, usize)],
    width: u32,
    height: u32,
) -> Vec<(f64, f64)> {
    if n == 0 {
        return Vec::new();
    }
    let w = width as f64;
    let h = height as f64;
    let margin = 60.0_f64;
    let cx = w / 2.0;
    let cy = h / 2.0;
    let r_init = ((w - 2.0 * margin).min(h - 2.0 * margin)) / 2.5;
    let mut pos: Vec<(f64, f64)> = (0..n)
        .map(|i| {
            let angle = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
            (cx + r_init * angle.cos(), cy + r_init * angle.sin())
        })
        .collect();
    if n == 1 {
        pos[0] = (cx, cy);
        return pos;
    }
    let area = (w - 2.0 * margin) * (h - 2.0 * margin);
    let k = (area / n as f64).sqrt();
    let iterations: usize = if n <= 100 {
        300
    } else if n <= 500 {
        150
    } else {
        50
    };
    let mut temp = w / 10.0;
    let mut disp: Vec<(f64, f64)> = vec![(0.0, 0.0); n];
    for _ in 0..iterations {
        for d in disp.iter_mut() {
            *d = (0.0, 0.0);
        }
        for i in 0..n {
            for j in (i + 1)..n {
                let dx = pos[j].0 - pos[i].0;
                let dy = pos[j].1 - pos[i].1;
                let dist = (dx * dx + dy * dy).sqrt().max(0.01);
                let force = k * k / dist;
                let fx = force * dx / dist;
                let fy = force * dy / dist;
                disp[i].0 -= fx;
                disp[i].1 -= fy;
                disp[j].0 += fx;
                disp[j].1 += fy;
            }
        }
        for &(si, ti) in edges {
            let dx = pos[ti].0 - pos[si].0;
            let dy = pos[ti].1 - pos[si].1;
            let dist = (dx * dx + dy * dy).sqrt().max(0.01);
            let force = dist * dist / k;
            let fx = force * dx / dist;
            let fy = force * dy / dist;
            disp[si].0 += fx;
            disp[si].1 += fy;
            disp[ti].0 -= fx;
            disp[ti].1 -= fy;
        }
        for i in 0..n {
            let mag = (disp[i].0 * disp[i].0 + disp[i].1 * disp[i].1)
                .sqrt()
                .max(0.001);
            let scale = temp.min(mag) / mag;
            pos[i].0 += disp[i].0 * scale;
            pos[i].1 += disp[i].1 * scale;
            pos[i].0 = pos[i].0.clamp(margin, w - margin);
            pos[i].1 = pos[i].1.clamp(margin, h - margin);
        }
        temp *= 0.95;
    }
    pos
}

fn cli_export_cypher(
    symbols: &[myceliums_storage::CodeSymbol],
    rels: &[myceliums_storage::Relationship],
    output: Option<&Path>,
) -> Result<()> {
    let out = export_neo4j_cypher(symbols, rels);
    if let Some(path) = output {
        std::fs::write(path, &out)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges (Cypher) to {}",
            symbols.len(),
            rels.len(),
            path.display()
        );
    } else {
        println!("{}", out);
    }
    Ok(())
}

fn export_svg(
    repo_info: &RepoInfo,
    symbols: &[myceliums_storage::models::CodeSymbol],
    rels: &[myceliums_storage::models::Relationship],
    width: u32,
    height: u32,
    output: Option<&Path>,
) -> Result<()> {
    const MAX_SVG_NODES: usize = 500;
    use std::collections::HashSet;

    // For large graphs, sample the most connected nodes
    let (sampled_symbols, sampled_rels) = if symbols.len() > MAX_SVG_NODES {
        // Count connections per symbol
        let mut conn_count: HashMap<&str, u32> = HashMap::new();
        for r in rels {
            *conn_count.entry(r.source_uid.as_str()).or_insert(0) += 1;
            *conn_count.entry(r.target_uid.as_str()).or_insert(0) += 1;
        }

        // Sort symbols by connection count, take top MAX_SVG_NODES
        let mut ranked: Vec<(usize, u32)> = symbols
            .iter()
            .enumerate()
            .map(|(i, s)| (i, conn_count.get(s.uid.as_str()).copied().unwrap_or(0)))
            .collect();
        ranked.sort_by_key(|b| std::cmp::Reverse(b.1));

        let keep_indices: HashSet<usize> =
            ranked.iter().take(MAX_SVG_NODES).map(|(i, _)| *i).collect();
        let keep_uids: HashSet<&str> = keep_indices
            .iter()
            .map(|&i| symbols[i].uid.as_str())
            .collect();

        let sampled_syms: Vec<myceliums_storage::models::CodeSymbol> = symbols
            .iter()
            .enumerate()
            .filter(|(i, _)| keep_indices.contains(i))
            .map(|(_, s)| s.clone())
            .collect();

        let sampled_rs: Vec<myceliums_storage::models::Relationship> = rels
            .iter()
            .filter(|r| {
                keep_uids.contains(r.source_uid.as_str())
                    && keep_uids.contains(r.target_uid.as_str())
            })
            .cloned()
            .collect();

        eprintln!(
            "  SVG: sampled {} of {} nodes (top by connections), {} edges",
            sampled_syms.len(),
            symbols.len(),
            sampled_rs.len()
        );
        (sampled_syms, sampled_rs)
    } else {
        (symbols.to_vec(), rels.to_vec())
    };

    let symbols = &sampled_symbols;
    let rels = &sampled_rels;

    let uid_to_community = compute_uid_to_community_label(symbols, rels).unwrap_or_default();
    let uid_to_idx: HashMap<&str, usize> = symbols
        .iter()
        .enumerate()
        .map(|(i, s)| (s.uid.as_str(), i))
        .collect();

    let n = symbols.len();
    let edge_pairs: Vec<(usize, usize)> = rels
        .iter()
        .filter_map(|r| {
            let si = uid_to_idx.get(r.source_uid.as_str()).copied()?;
            let ti = uid_to_idx.get(r.target_uid.as_str()).copied()?;
            (si != ti).then_some((si, ti))
        })
        .collect();
    let pos = force_directed_layout(n, &edge_pairs, width, height);

    const COMMUNITY_COLORS: &[&str] = &[
        "rgba(99,102,241,0.08)",
        "rgba(16,185,129,0.08)",
        "rgba(245,158,11,0.08)",
        "rgba(236,72,153,0.08)",
        "rgba(139,92,246,0.08)",
        "rgba(6,182,212,0.08)",
        "rgba(249,115,22,0.08)",
        "rgba(100,116,139,0.08)",
    ];

    let mut community_color_counter = 0usize;
    let mut community_color_idx: HashMap<String, usize> = HashMap::new();
    let mut community_bounds: HashMap<String, (f64, f64, f64, f64, usize)> = HashMap::new();
    for (i, s) in symbols.iter().enumerate() {
        if let Some(label) = uid_to_community.get(&s.uid) {
            let (px, py) = pos[i];
            if !community_color_idx.contains_key(label) {
                let c = community_color_counter % COMMUNITY_COLORS.len();
                community_color_counter += 1;
                community_color_idx.insert(label.clone(), c);
            }
            let cidx = community_color_idx[label];
            let entry = community_bounds
                .entry(label.clone())
                .or_insert((px, py, px, py, cidx));
            entry.0 = entry.0.min(px);
            entry.1 = entry.1.min(py);
            entry.2 = entry.2.max(px);
            entry.3 = entry.3.max(py);
        }
    }

    let w = width as f64;
    let h = height as f64;
    #[allow(dead_code)]
    const NODE_R: f64 = 8.0;
    const EDGE_PAD: f64 = 18.0;
    let mut svg = String::with_capacity(64 * 1024);

    svg.push_str(&format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{width}\" height=\"{height}\" \
         viewBox=\"0 0 {width} {height}\">\n"
    ));
    // Dark background matching website (#09090B)
    svg.push_str("  <defs>\n    <filter id=\"glow\"><feGaussianBlur stdDeviation=\"2\" result=\"blur\"/><feMerge><feMergeNode in=\"blur\"/><feMergeNode in=\"SourceGraphic\"/></feMerge></filter>\n  </defs>\n");
    svg.push_str(&format!(
        "  <rect width=\"{width}\" height=\"{height}\" fill=\"#09090B\"/>\n"
    ));
    svg.push_str(&format!(
        "  <text x=\"{:.1}\" y=\"24\" font-size=\"14\" text-anchor=\"middle\" \
         fill=\"rgba(255,255,255,0.9)\" font-family=\"system-ui, -apple-system, sans-serif\" \
         font-weight=\"600\">{}</text>\n",
        w / 2.0,
        xml_escape(&repo_info.name)
    ));

    for (label, (min_x, min_y, max_x, max_y, cidx)) in &community_bounds {
        let rx = (min_x - EDGE_PAD).max(2.0);
        let ry = (min_y - EDGE_PAD).max(2.0);
        let rw = ((max_x + EDGE_PAD).min(w - 2.0) - rx).max(0.0);
        let rh = ((max_y + EDGE_PAD).min(h - 2.0) - ry).max(0.0);
        svg.push_str(&format!(
            "  <rect x=\"{rx:.1}\" y=\"{ry:.1}\" width=\"{rw:.1}\" height=\"{rh:.1}\" \
             rx=\"12\" ry=\"12\" fill=\"{}\" stroke=\"rgba(255,255,255,0.04)\" stroke-width=\"1\"/>\n",
            COMMUNITY_COLORS[*cidx]
        ));
        svg.push_str(&format!(
            "  <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"9\" fill=\"rgba(255,255,255,0.35)\" \
             font-family=\"system-ui, -apple-system, sans-serif\" letter-spacing=\"0.05em\">{}</text>\n",
            rx + 6.0,
            (ry + 14.0).min(h - 4.0),
            xml_escape(&svg_truncate(label, 20))
        ));
    }

    for &(si, ti) in &edge_pairs {
        let (x1, y1) = pos[si];
        let (x2, y2) = pos[ti];
        let mx = (x1 + x2) / 2.0;
        let my = (y1 + y2) / 2.0;
        let dx = x2 - x1;
        let dy = y2 - y1;
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let offset = (len * 0.2).min(20.0);
        let qx = mx - dy / len * offset;
        let qy = my + dx / len * offset;
        svg.push_str(&format!(
            "  <path d=\"M{x1:.1},{y1:.1} Q{qx:.1},{qy:.1} {x2:.1},{y2:.1}\" \
             stroke=\"rgba(255,255,255,0.07)\" stroke-width=\"0.5\" fill=\"none\"/>\n"
        ));
    }

    for (i, s) in symbols.iter().enumerate() {
        let (px, py) = pos[i];
        let color = kind_color(&s.kind);
        svg.push_str(&format!(
            "  <circle cx=\"{px:.1}\" cy=\"{py:.1}\" r=\"4\" fill=\"{color}\" \
             opacity=\"0.85\" filter=\"url(#glow)\"><title>{}</title></circle>\n",
            xml_escape(&s.qualified_name)
        ));
        svg.push_str(&format!(
            "  <text x=\"{px:.1}\" y=\"{:.1}\" font-size=\"7\" text-anchor=\"middle\" \
             fill=\"rgba(255,255,255,0.5)\" font-family=\"system-ui, -apple-system, sans-serif\">{}</text>\n",
            py + 4.0 + 8.0,
            xml_escape(&svg_truncate(&s.name, 12))
        ));
    }

    const LEGEND_ENTRIES: &[(&str, &str)] = &[
        ("Function", "#6366F1"),
        ("Method", "#8B5CF6"),
        ("Class", "#EC4899"),
        ("Interface", "#06B6D4"),
        ("Variable", "#10B981"),
        ("Constant", "#F59E0B"),
        ("Enum", "#EF4444"),
        ("Module", "#14B8A6"),
        ("Section", "#818CF8"),
        ("Document", "#34D399"),
        ("Rationale", "#FBBF24"),
    ];
    let legend_x = 10.0_f64;
    let legend_rows = LEGEND_ENTRIES.len() as f64;
    let legend_y = h - legend_rows * 14.0 - 10.0;
    svg.push_str(&format!(
        "  <rect x=\"{:.1}\" y=\"{:.1}\" width=\"100\" height=\"{:.1}\" rx=\"6\" \
         fill=\"rgba(9,9,11,0.8)\" stroke=\"rgba(255,255,255,0.08)\" stroke-width=\"1\"/>\n",
        legend_x - 2.0,
        legend_y - 4.0,
        legend_rows * 14.0 + 8.0
    ));
    for (i, (name, color)) in LEGEND_ENTRIES.iter().enumerate() {
        let ly = legend_y + i as f64 * 14.0 + 10.0;
        svg.push_str(&format!(
            "  <circle cx=\"{:.1}\" cy=\"{ly:.1}\" r=\"3\" fill=\"{color}\" opacity=\"0.9\"/>\
             <text x=\"{:.1}\" y=\"{:.1}\" font-size=\"8\" fill=\"rgba(255,255,255,0.6)\" \
             font-family=\"system-ui, -apple-system, sans-serif\">{name}</text>\n",
            legend_x + 5.0,
            legend_x + 13.0,
            ly + 3.0,
        ));
    }

    svg.push_str("</svg>\n");

    if let Some(path) = output {
        std::fs::write(path, &svg)
            .with_context(|| format!("Failed to write to {}", path.display()))?;
        eprintln!(
            "Exported {} nodes, {} edges (SVG) to {}",
            n,
            edge_pairs.len(),
            path.display()
        );
    } else {
        println!("{}", svg);
    }

    Ok(())
}
