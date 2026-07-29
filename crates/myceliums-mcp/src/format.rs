use crate::{
    AnalyzeOutput, CalleesOutput, CallersOutput, CentralityOutput, CommunitiesOutput,
    CommunityDetailOutput, CommunityMetricsOutput, CyclesOutput, DeadCodeOutput,
    DependenciesOutput, FileSymbolsOutput, FindPathOutput, GodNodesOutput, GraphDiffOutput,
    HybridSearchResultItem, KnowledgeGapsOutput, KnowledgeQueryOutput, ModuleCouplingOutput,
    ProcessItem, RationaleOutput, SearchResultItem, StatsOutput, SuggestedQuestionsOutput,
    SurprisingConnectionsOutput, SymbolContextOutput, SymbolDefinitionOutput,
};
use myceliums_core::{
    ArchDecisionRecord, ArchDiagram, ContractsReport, DriftReport, HotspotItem, LintReport,
    OwnershipReport,
};

pub fn format_analyze(o: &AnalyzeOutput) -> String {
    let status = if o.cached { "cached" } else { "fresh" };
    let mut out = format!(
        "Analysis complete ({status}):\n\
         \n\
         Repository:  {repo}\n\
         Symbols:     {sym}\n\
         Files:       {files}\n\
         Relations:   {rels}\n\
         Communities: {comm}\n\
         Processes:   {proc}\n\
         Embeddings:  {embedded}/{total} symbols",
        repo = o.repo_id,
        sym = o.symbols,
        files = o.files,
        rels = o.relationships,
        comm = o.communities,
        proc = o.processes,
        embedded = o.symbols_embedded,
        total = o.symbols_total,
    );
    if o.embedding_failures > 0 {
        out.push_str(&format!(
            "\n⚠ Embedding failures: {} — {} of {} symbols have no vector and are \
             invisible to semantic/hybrid search",
            o.embedding_failures, o.symbols_embedded, o.symbols_total
        ));
    }
    out
}

/// Prefix a search result body with a partial-index warning banner when one
/// applies. Keeps the warning impossible to miss at the top of the response
/// while leaving the result table untouched.
pub fn with_index_warning(warning: Option<String>, body: String) -> String {
    match warning {
        Some(w) => format!("⚠ {w}\n\n{body}"),
        None => body,
    }
}

pub fn format_search_results(query: &str, results: &[SearchResultItem]) -> String {
    if results.is_empty() {
        return format!("No results for \"{query}\".");
    }

    let mut out = format!("Found {} results for \"{}\":\n\n", results.len(), query);

    // Column widths
    let name_w = results
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 35);
    let kind_w = results
        .iter()
        .map(|r| r.kind.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 12);

    out.push_str(&format!(
        " {:<3} {:<name_w$}  {:<kind_w$}  {:<40}  {:>6}  Score\n",
        "#", "Name", "Kind", "File", "Lines",
    ));
    out.push_str(&format!(
        " {:-<3} {:-<name_w$}  {:-<kind_w$}  {:-<40}  {:->6}  -----\n",
        "", "", "", "", "",
    ));

    for (i, r) in results.iter().enumerate() {
        let short_file = shorten_path(&r.file_path, 40);
        let lines = if r.start_line == r.end_line {
            format!("{}", r.start_line)
        } else {
            format!("{}-{}", r.start_line, r.end_line)
        };
        let name = truncate(&r.name, name_w);
        let kind = truncate(&r.kind, kind_w);
        out.push_str(&format!(
            " {:<3} {:<name_w$}  {:<kind_w$}  {:<40}  {:>6}  {:.2}\n",
            i + 1,
            name,
            kind,
            short_file,
            lines,
            r.score,
        ));
    }

    out
}

pub fn format_hybrid_results(query: &str, results: &[HybridSearchResultItem]) -> String {
    if results.is_empty() {
        return format!("No results for \"{query}\".");
    }

    let mut out = format!(
        "Found {} results for \"{}\" (hybrid BM25 + vector):\n\n",
        results.len(),
        query
    );

    let name_w = results
        .iter()
        .map(|r| r.name.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 35);
    let kind_w = results
        .iter()
        .map(|r| r.kind.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 12);

    out.push_str(&format!(
        " {:<3} {:<name_w$}  {:<kind_w$}  {:<40}  {:>6}  {:>6}  {:>6}  {:>5}  {:>5}\n",
        "#", "Name", "Kind", "File", "Lines", "Fusion", "Rerank", "BM25", "Vec",
    ));
    out.push_str(&format!(
        " {:-<3} {:-<name_w$}  {:-<kind_w$}  {:-<40}  {:->6}  {:->6}  {:->6}  {:->5}  {:->5}\n",
        "", "", "", "", "", "", "", "", "",
    ));

    for (i, r) in results.iter().enumerate() {
        let short_file = shorten_path(&r.file_path, 40);
        let lines = if r.start_line == r.end_line {
            format!("{}", r.start_line)
        } else {
            format!("{}-{}", r.start_line, r.end_line)
        };
        let bm25 = r
            .bm25_rank
            .map(|r| format!("#{}", r))
            .unwrap_or_else(|| "-".into());
        let vec = r
            .vector_rank
            .map(|r| format!("#{}", r))
            .unwrap_or_else(|| "-".into());
        let rerank = r
            .rerank_score
            .map(|s| format!("{:.2}", s))
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            " {:<3} {:<name_w$}  {:<kind_w$}  {:<40}  {:>6}  {:>6.2}  {:>6}  {:>5}  {:>5}\n",
            i + 1,
            truncate(&r.name, name_w),
            truncate(&r.kind, kind_w),
            short_file,
            lines,
            r.combined_score,
            rerank,
            bm25,
            vec,
        ));
    }

    out
}

pub fn format_symbol_context(s: &SymbolContextOutput) -> String {
    let mut out = format!(
        "Symbol: {} ({})\n\
         File:   {}:{}-{}\n",
        s.name, s.kind, s.file_path, s.start_line, s.end_line,
    );

    if !s.signature.is_empty() {
        out.push_str(&format!("\nSignature: {}\n", s.signature));
    }

    if !s.callers.is_empty() {
        out.push_str(&format!(
            "\nCallers ({}): {}\n",
            s.callers.len(),
            s.callers.join(", ")
        ));
    } else {
        out.push_str("\nCallers: none\n");
    }

    if !s.callees.is_empty() {
        out.push_str(&format!(
            "Callees ({}): {}\n",
            s.callees.len(),
            s.callees.join(", ")
        ));
    } else {
        out.push_str("Callees: none\n");
    }

    // Show metadata (decorators, return type, superclasses, visibility)
    if let Some(ref meta_json) = s.metadata {
        if let Ok(meta) = serde_json::from_str::<serde_json::Value>(meta_json) {
            let mut meta_parts = Vec::new();
            if let Some(decorators) = meta.get("decorators").and_then(|v| v.as_array()) {
                if !decorators.is_empty() {
                    let dec_strs: Vec<&str> =
                        decorators.iter().filter_map(|d| d.as_str()).collect();
                    meta_parts.push(format!("Decorators: {}", dec_strs.join(", ")));
                }
            }
            if let Some(rt) = meta.get("return_type").and_then(|v| v.as_str()) {
                meta_parts.push(format!("Returns: {}", rt));
            }
            if let Some(supers) = meta.get("superclasses").and_then(|v| v.as_array()) {
                if !supers.is_empty() {
                    let sup_strs: Vec<&str> = supers.iter().filter_map(|s| s.as_str()).collect();
                    meta_parts.push(format!("Extends: {}", sup_strs.join(", ")));
                }
            }
            if let Some(vis) = meta.get("visibility").and_then(|v| v.as_str()) {
                meta_parts.push(format!("Visibility: {}", vis));
            }
            if !meta_parts.is_empty() {
                out.push_str(&format!("\n{}\n", meta_parts.join(" | ")));
            }
        }
    }

    if !s.content.is_empty() {
        out.push_str(&format!("\nSource:\n{}\n", s.content));
    }

    out
}

pub fn format_processes(processes: &[ProcessItem]) -> String {
    if processes.is_empty() {
        return "No execution flows found.".to_string();
    }

    let mut out = format!("Found {} execution flows:\n\n", processes.len());

    let name_w = processes
        .iter()
        .map(|p| p.entry_point.len())
        .max()
        .unwrap_or(5)
        .clamp(5, 30);

    out.push_str(&format!(
        " {:<3} {:<name_w$}  {:>5}  Description\n",
        "#", "Entry Point", "Steps",
    ));
    out.push_str(&format!(
        " {:-<3} {:-<name_w$}  {:->5}  -----------\n",
        "", "", "",
    ));

    for (i, p) in processes.iter().enumerate() {
        out.push_str(&format!(
            " {:<3} {:<name_w$}  {:>5}  {}\n",
            i + 1,
            truncate(&p.entry_point, name_w),
            p.step_count,
            truncate(&p.description, 80),
        ));
    }

    out
}

pub fn format_impact_report(report: &serde_json::Value) -> String {
    let mut out = String::from("Impact Analysis:\n\n");

    if let Some(changed) = report.get("changed_symbols").and_then(|v| v.as_array()) {
        out.push_str(&format!("Changed symbols ({}):\n", changed.len()));
        for s in changed {
            let name = s["name"].as_str().unwrap_or("?");
            let file = s["file_path"].as_str().unwrap_or("?");
            let short = shorten_path(file, 40);
            out.push_str(&format!("  * {} ({})\n", name, short));
        }
        out.push('\n');
    }

    if let Some(affected) = report.get("affected_symbols").and_then(|v| v.as_array()) {
        out.push_str(&format!("Affected symbols ({}):\n", affected.len()));
        for s in affected {
            let name = s["name"].as_str().unwrap_or("?");
            let file = s["file_path"].as_str().unwrap_or("?");
            let depth = s["depth"].as_u64().unwrap_or(0);
            let short = shorten_path(file, 40);
            out.push_str(&format!("  * {} ({}) [depth: {}]\n", name, short, depth));
        }
        out.push('\n');
    }

    if let Some(files) = report.get("affected_files").and_then(|v| v.as_array()) {
        out.push_str(&format!("Affected files ({}):\n", files.len()));
        for f in files {
            let path = f.as_str().unwrap_or("?");
            out.push_str(&format!("  * {}\n", path));
        }
    }

    if out == "Impact Analysis:\n\n" {
        // Fallback to pretty JSON if structure not recognized
        out.push_str(&serde_json::to_string_pretty(report).unwrap_or_default());
    }

    out
}

pub fn format_rename_plan(plan: &serde_json::Value) -> String {
    let mut out = String::new();

    let old = plan["old_name"].as_str().unwrap_or("?");
    let new = plan["new_name"].as_str().unwrap_or("?");
    out.push_str(&format!("Rename: {} -> {}\n\n", old, new));

    if let Some(edits) = plan.get("edits").and_then(|v| v.as_array()) {
        out.push_str(&format!("Edits ({}):\n", edits.len()));
        for e in edits {
            let file = e["file_path"].as_str().unwrap_or("?");
            let line = e["line"].as_u64().unwrap_or(0);
            let kind = e["kind"].as_str().unwrap_or("edit");
            out.push_str(&format!("  * {}:{} ({})\n", file, line, kind));
        }
    }

    if out.ends_with("\n\n") {
        // Fallback
        out.push_str(&serde_json::to_string_pretty(plan).unwrap_or_default());
    }

    out
}

pub fn format_cypher_results(query: &str, results: &serde_json::Value) -> String {
    let mut out = format!("Cypher: {}\n\n", query);

    match results {
        serde_json::Value::Array(rows) if !rows.is_empty() => {
            out.push_str(&format!("{} rows returned:\n\n", rows.len()));

            // Try to format as table if rows are objects with same keys
            if let Some(first) = rows.first().and_then(|r| r.as_object()) {
                let keys: Vec<&String> = first.keys().collect();
                let col_widths: Vec<usize> = keys
                    .iter()
                    .map(|k| {
                        rows.iter()
                            .map(|r| cell_str(r.get(k.as_str())).len())
                            .max()
                            .unwrap_or(0)
                            .max(k.len())
                            .min(40)
                    })
                    .collect();

                // Header
                for (i, key) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push_str("  ");
                    }
                    out.push_str(&format!("{:<w$}", key, w = col_widths[i]));
                }
                out.push('\n');
                for (i, w) in col_widths.iter().enumerate() {
                    if i > 0 {
                        out.push_str("  ");
                    }
                    out.push_str(&"-".repeat(*w));
                }
                out.push('\n');

                // Rows
                for row in rows {
                    for (i, key) in keys.iter().enumerate() {
                        if i > 0 {
                            out.push_str("  ");
                        }
                        let val = cell_str(row.get(key.as_str()));
                        out.push_str(&format!(
                            "{:<w$}",
                            truncate(&val, col_widths[i]),
                            w = col_widths[i]
                        ));
                    }
                    out.push('\n');
                }
            } else {
                out.push_str(&serde_json::to_string_pretty(results).unwrap_or_default());
            }
        }
        _ => {
            out.push_str(&serde_json::to_string_pretty(results).unwrap_or_default());
        }
    }

    out
}

// --- review context ---

pub struct ReviewSymbolEntry {
    pub signature: String,
    pub file_path: String,
    pub start_line: u32,
    pub kind: String,
    pub source: Option<String>,
}

pub struct ReviewContext {
    pub changed: Vec<ReviewSymbolEntry>,
    pub callers: Vec<ReviewSymbolEntry>,
    pub callees: Vec<ReviewSymbolEntry>,
    pub communities_touched: Vec<String>,
    pub changed_file_count: usize,
    pub full_source_tokens: usize,
}

pub fn format_review_context(ctx: &ReviewContext) -> String {
    let mut out = format!(
        "Review context for {} changed symbol{} across {} file{}:\n\n",
        ctx.changed.len(),
        if ctx.changed.len() == 1 { "" } else { "s" },
        ctx.changed_file_count,
        if ctx.changed_file_count == 1 { "" } else { "s" },
    );

    // Changed symbols
    out.push_str("Changed:\n");
    if ctx.changed.is_empty() {
        out.push_str("  (no symbols matched in the graph)\n");
    } else {
        for entry in &ctx.changed {
            let location = format!(
                "{}:{}",
                shorten_path(&entry.file_path, 40),
                entry.start_line
            );
            out.push_str(&format!(
                "  {} {:<50} {}\n",
                entry.kind, entry.signature, location
            ));
            if let Some(src) = &entry.source {
                for line in src.lines() {
                    out.push_str(&format!("    | {}\n", line));
                }
            }
        }
    }
    out.push('\n');

    // Callers
    out.push_str(&format!("Callers ({}):\n", ctx.callers.len()));
    if ctx.callers.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for entry in &ctx.callers {
            let location = format!(
                "{}:{}",
                shorten_path(&entry.file_path, 40),
                entry.start_line
            );
            out.push_str(&format!(
                "  {} {:<50} {}\n",
                entry.kind, entry.signature, location
            ));
        }
    }
    out.push('\n');

    // Callees
    out.push_str(&format!("Callees ({}):\n", ctx.callees.len()));
    if ctx.callees.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for entry in &ctx.callees {
            let location = format!(
                "{}:{}",
                shorten_path(&entry.file_path, 40),
                entry.start_line
            );
            out.push_str(&format!(
                "  {} {:<50} {}\n",
                entry.kind, entry.signature, location
            ));
        }
    }
    out.push('\n');

    // Communities
    if !ctx.communities_touched.is_empty() {
        out.push_str(&format!(
            "Communities touched: [{}]\n\n",
            ctx.communities_touched.join(", ")
        ));
    }

    // Token estimate
    let compact_chars: usize = out.len();
    let compact_tokens = compact_chars / 4;
    if ctx.full_source_tokens > 0 && compact_tokens > 0 {
        let reduction = ctx.full_source_tokens / compact_tokens.max(1);
        out.push_str(&format!(
            "Token estimate: ~{} tokens (vs ~{} for full source — {}x reduction)",
            compact_tokens, ctx.full_source_tokens, reduction,
        ));
    } else {
        out.push_str(&format!("Token estimate: ~{} tokens", compact_tokens));
    }

    out
}

// --- helpers ---

fn truncate(s: &str, max: usize) -> String {
    if s.len() > max && max > 3 {
        format!("{}...", &s[..max - 3])
    } else if s.len() > max {
        s[..max].to_string()
    } else {
        s.to_string()
    }
}

fn shorten_path(path: &str, max: usize) -> String {
    if path.len() <= max {
        return path.to_string();
    }
    // Try to show the most relevant part (from the right)
    let parts: Vec<&str> = path.split('/').collect();
    let mut result = String::new();
    for part in parts.iter().rev() {
        let candidate = if result.is_empty() {
            part.to_string()
        } else {
            format!("{}/{}", part, result)
        };
        if candidate.len() > max && !result.is_empty() {
            return format!(".../{}", result);
        }
        result = candidate;
    }
    truncate(&result, max)
}

pub fn format_god_nodes(output: &GodNodesOutput) -> String {
    if output.nodes.is_empty() {
        return "No symbols with CALLS edges found. The graph may contain only imports/containment relationships.".to_string();
    }

    let mut out = format!(
        "God nodes ({} of {} symbols, {} flagged as high-coupling):\n\n",
        output.nodes.len(),
        output.total_symbols,
        output.high_coupling_count,
    );

    let name_w = output
        .nodes
        .iter()
        .map(|n| n.name.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 35);
    let kind_w = output
        .nodes
        .iter()
        .map(|n| n.kind.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 12);

    out.push_str(&format!(
        " {:<3} {:<name_w$}  {:<kind_w$}  {:>6}  {:>4}  {:>4}  {:<40}\n",
        "#", "Name", "Kind", "Degree", "In", "Out", "File",
    ));
    out.push_str(&format!(
        " {:-<3} {:-<name_w$}  {:-<kind_w$}  {:->6}  {:->4}  {:->4}  {:-<40}\n",
        "", "", "", "", "", "", "",
    ));

    for (i, n) in output.nodes.iter().enumerate() {
        let flag = if n.is_high_coupling { " !" } else { "  " };
        let short_file = shorten_path(&n.file_path, 40);
        out.push_str(&format!(
            " {:<3} {:<name_w$}  {:<kind_w$}  {:>6}  {:>4}  {:>4}  {:<40}{}\n",
            i + 1,
            truncate(&n.name, name_w),
            truncate(&n.kind, kind_w),
            n.degree,
            n.in_degree,
            n.out_degree,
            short_file,
            flag,
        ));
    }

    if output.high_coupling_count > 0 {
        out.push_str("\n! = high-coupling node (degree > threshold); consider refactoring\n");
    }

    out
}

pub fn format_path(output: &FindPathOutput) -> String {
    if !output.found {
        return format!(
            "No path found between '{}' and '{}' within the depth limit.",
            output.from_symbol, output.to_symbol,
        );
    }

    let mut out = format!(
        "Shortest path from '{}' to '{}' ({} hops):\n\n",
        output.from_symbol, output.to_symbol, output.total_depth,
    );

    for (i, step) in output.steps.iter().enumerate() {
        let location = format!("{}:{}", shorten_path(&step.file_path, 40), step.start_line);
        if i == 0 {
            out.push_str(&format!(
                " [start] {} ({}) {}\n",
                step.symbol_name, step.kind, location,
            ));
        } else {
            out.push_str(&format!(
                "    --[{}]-->\n [{}] {} ({}) {}\n",
                step.edge_type, i, step.symbol_name, step.kind, location,
            ));
        }
    }

    out
}

pub fn format_surprising_connections(output: &SurprisingConnectionsOutput) -> String {
    if output.connections.is_empty() {
        return "No surprising cross-community connections found. Either all symbols are in \
                a single community, or no CALLS edges cross community boundaries above the \
                minimum surprise threshold."
            .to_string();
    }

    let mut out = format!(
        "Surprising cross-community connections ({} found):\n\n",
        output.connections.len(),
    );

    for (i, c) in output.connections.iter().enumerate() {
        out.push_str(&format!(
            " {}. {:.4}  {} → {}\n       ({} → {})\n",
            i + 1,
            c.surprise_score,
            truncate(&c.source_name, 30),
            truncate(&c.target_name, 30),
            truncate(&c.source_community, 25),
            truncate(&c.target_community, 25),
        ));
    }

    out.push_str(
        "\nScore interpretation: 1.0 = unique cross-community link; 0.0 = very common pair\n",
    );
    out
}

fn cell_str(v: Option<&serde_json::Value>) -> String {
    match v {
        None => String::new(),
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) => "null".into(),
        Some(serde_json::Value::Bool(b)) => b.to_string(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
    }
}

// --- New tool formatters (used when tools switch to TextOutput) ---

#[allow(dead_code)]
pub fn format_communities(output: &CommunitiesOutput) -> String {
    if output.communities.is_empty() {
        return "No communities detected.".to_string();
    }

    let mut out = format!("Communities ({}):\n\n", output.communities.len());

    for (idx, community) in output.communities.iter().enumerate() {
        out.push_str(&format!(
            "{}. {} ({}\n   Members: {}\n   Summary: {}\n\n",
            idx + 1,
            community.label,
            community.uid,
            community.member_count,
            community.summary
        ));
    }

    out
}
#[allow(dead_code)]
pub fn format_community_detail(output: &CommunityDetailOutput) -> String {
    let mut out = format!(
        "Community: {} ({})\nSummary: {}\nMembers: {}\nInternal edges: {}, External edges: {}\n\nSymbols:\n",
        output.label, output.uid, output.summary, output.member_count,
        output.internal_edge_count, output.external_edge_count
    );

    if output.symbols.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for symbol in &output.symbols {
            let location = format!(
                "{}:{}",
                shorten_path(&symbol.file_path, 40),
                symbol.start_line
            );
            out.push_str(&format!(
                "  {} {} {} {}\n",
                symbol.kind, symbol.name, location, symbol.signature
            ));
        }
    }

    out
}

#[allow(dead_code)]
pub fn format_symbol_definition(output: &SymbolDefinitionOutput) -> String {
    let mut out = format!(
        "Definition: {} ({})\nKind: {}\nLocation: {}:{}-{}\nSignature: {}\n\nSource:\n\n",
        output.name,
        output.qualified_name,
        output.kind,
        output.file_path,
        output.start_line,
        output.end_line,
        output.signature
    );

    let lines: Vec<&str> = output.content.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        let line_num = output.start_line as usize + idx;
        out.push_str(&format!("{:4} | {}\n", line_num, line));
    }

    out
}

#[allow(dead_code)]
pub fn format_dead_code(output: &DeadCodeOutput) -> String {
    if output.symbols.is_empty() {
        return "No dead code detected!".to_string();
    }

    let mut out = format!(
        "Dead code detected ({}/{} symbols have no callers):\n\n",
        output.symbols.len(),
        output.total_count
    );

    for (idx, symbol) in output.symbols.iter().enumerate() {
        let location = format!(
            "{}:{}",
            shorten_path(&symbol.file_path, 40),
            symbol.start_line
        );
        out.push_str(&format!(
            "{}. {} ({} in {})\n   {}\n\n",
            idx + 1,
            symbol.name,
            symbol.kind,
            location,
            symbol.signature
        ));
    }

    out
}

#[allow(dead_code)]
pub fn format_callers(output: &CallersOutput) -> String {
    let mut out = format!(
        "Callers of {} ({} found",
        output.symbol_name, output.total_count
    );

    if output.depth_limited {
        out.push_str(", depth limited");
    }
    out.push_str("):\n\n");

    if output.callers.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for caller in &output.callers {
            let location = format!(
                "{}:{}",
                shorten_path(&caller.file_path, 40),
                caller.start_line
            );
            out.push_str(&format!(
                "  [depth {}] {} {} {}\n",
                caller.depth, caller.kind, caller.name, location
            ));
        }
    }

    out
}

#[allow(dead_code)]
pub fn format_callees(output: &CalleesOutput) -> String {
    let mut out = format!(
        "Callees of {} ({} found",
        output.symbol_name, output.total_count
    );

    if output.depth_limited {
        out.push_str(", depth limited");
    }
    out.push_str("):\n\n");

    if output.callees.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for callee in &output.callees {
            let location = format!(
                "{}:{}",
                shorten_path(&callee.file_path, 40),
                callee.start_line
            );
            out.push_str(&format!(
                "  [depth {}] {} {} {}\n",
                callee.depth, callee.kind, callee.name, location
            ));
        }
    }

    out
}

#[allow(dead_code)]
pub fn format_file_symbols(output: &FileSymbolsOutput) -> String {
    let mut out = format!(
        "Symbols in {} ({} total):\n\n",
        output.file_path, output.total_count
    );

    if output.symbols.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for symbol in &output.symbols {
            out.push_str(&format!(
                "  {} {} (lines {}-{})\n     {}\n\n",
                symbol.kind, symbol.name, symbol.start_line, symbol.end_line, symbol.signature
            ));
        }
    }

    out
}

#[allow(dead_code)]
pub fn format_stats(output: &StatsOutput) -> String {
    let mut out = String::from("Codebase Statistics:\n\n");

    out.push_str(&format!(
        "Overview:\n  Total Symbols:       {}\n  Total Files:         {}\n  Total Relationships: {}\n  Communities:         {}\n  Processes:           {}\n\n",
        output.total_symbols, output.total_files, output.total_relationships,
        output.community_count, output.process_count
    ));

    out.push_str("Symbols by Kind:\n");
    let mut symbol_kinds: Vec<_> = output.symbol_counts.iter().collect();
    symbol_kinds.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (kind, count) in symbol_kinds {
        out.push_str(&format!("  {}: {}\n", kind, count));
    }

    out.push_str("\nRelationships by Type:\n");
    let mut rel_types: Vec<_> = output.relationship_counts.iter().collect();
    rel_types.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (rel_type, count) in rel_types {
        out.push_str(&format!("  {}: {}\n", rel_type, count));
    }

    out.push_str("\nFiles by Language:\n");
    let mut languages: Vec<_> = output.language_counts.iter().collect();
    languages.sort_by_key(|(_, count)| std::cmp::Reverse(**count));
    for (language, count) in languages {
        out.push_str(&format!("  {}: {}\n", language, count));
    }

    out
}

pub fn format_knowledge_gaps(output: &KnowledgeGapsOutput) -> String {
    if output.gaps.is_empty() {
        return "No knowledge gaps detected.".to_string();
    }

    let mut out = format!(
        "Knowledge gaps detected ({} total):\n\
         \n\
         Summary:\n\
         \x20 Untested code:            {}\n\
         \x20 Isolated modules:          {}\n\
         \x20 Undocumented files:        {}\n\
         \x20 Single points of failure:  {}\n\n",
        output.total_count,
        output.summary.untested_count,
        output.summary.isolated_count,
        output.summary.undocumented_count,
        output.summary.single_point_of_failure_count,
    );

    // Group by category for cleaner output
    let categories = [
        ("single_point_of_failure", "Single Points of Failure"),
        ("untested", "Untested Code"),
        ("isolated", "Isolated Modules"),
        ("undocumented", "Undocumented Files"),
    ];

    for (cat_key, cat_label) in &categories {
        let cat_gaps: Vec<_> = output
            .gaps
            .iter()
            .filter(|g| g.category == *cat_key)
            .collect();
        if cat_gaps.is_empty() {
            continue;
        }

        out.push_str(&format!("--- {} ({}) ---\n\n", cat_label, cat_gaps.len()));

        for (i, gap) in cat_gaps.iter().enumerate() {
            let severity_marker = match gap.severity.as_str() {
                "high" => "!!",
                "medium" => "! ",
                _ => "  ",
            };
            let location = format!("{}:{}", shorten_path(&gap.file_path, 40), gap.start_line);
            out.push_str(&format!(
                " {} {:<3} {} ({}) {}\n      {}\n      -> {}\n\n",
                severity_marker,
                i + 1,
                truncate(&gap.symbol_name, 35),
                gap.kind,
                location,
                gap.description,
                gap.suggestion,
            ));
        }
    }

    out.push_str("Severity: !! = high, ! = medium, (blank) = low\n");
    out
}

pub fn format_rationale(output: &RationaleOutput) -> String {
    if output.rationales.is_empty() {
        return "No rationale comments found.".to_string();
    }

    let mut out = format!("Found {} rationale comment(s):\n\n", output.total_count);

    for (i, r) in output.rationales.iter().enumerate() {
        let target = r.target_symbol.as_deref().unwrap_or("(no linked symbol)");
        out.push_str(&format!(
            " {}. [{}] {}:{} -> {}\n    {}\n\n",
            i + 1,
            r.prefix,
            shorten_path(&r.file_path, 40),
            r.line,
            target,
            r.text,
        ));
    }

    out
}

pub fn format_knowledge_query(output: &KnowledgeQueryOutput) -> String {
    if output.results.is_empty() {
        return format!(
            "Knowledge Query: \"{}\"\n\nNo mentions found.",
            output.query
        );
    }

    let mut out = format!(
        "Knowledge Query: \"{}\"\n\n         Found {} mentions across {} sources:\n\n",
        output.query, output.total_mentions, output.unique_sources
    );

    // Group results by source
    let mut by_source: std::collections::BTreeMap<String, Vec<_>> =
        std::collections::BTreeMap::new();
    for result in &output.results {
        let key = format!(
            "[{}] {} ({})",
            result.source_kind, result.source_name, result.source_file
        );
        by_source.entry(key).or_insert_with(Vec::new).push(result);
    }

    for (source_key, results) in by_source {
        out.push_str(&format!("{}\n", source_key));

        for result in results {
            out.push_str(&format!(
                "  • {} ({}) at {}:{}\n",
                result.mentioned_symbol,
                result.mentioned_kind,
                result.mentioned_file,
                result.mentioned_line
            ));
            out.push_str(&format!(
                "    Line {}: \"{}\"\n",
                result.match_line, result.match_context
            ));
            out.push_str(&format!("    Confidence: {:.1}\n", result.confidence));
        }
        out.push('\n');
    }

    out
}

pub fn format_suggested_questions(output: &SuggestedQuestionsOutput) -> String {
    if output.questions.is_empty() {
        return "No review questions generated.".to_string();
    }

    let mut out = format!(
        "Suggested review questions ({} total):\n\n",
        output.questions.len()
    );

    for (i, q) in output.questions.iter().enumerate() {
        out.push_str(&format!(
            " {}. [{}] {}\n",
            i + 1,
            q.severity.to_uppercase(),
            q.question
        ));
        out.push_str(&format!("    Category: {}\n", q.category));
        out.push_str(&format!("    Rationale: {}\n", q.rationale));

        if !q.references.is_empty() {
            out.push_str(&format!("    References: {}\n", q.references.join(", ")));
        }

        out.push('\n');
    }

    out
}

pub fn format_graph_diff(output: &GraphDiffOutput) -> String {
    let total = output.added_symbols.len()
        + output.removed_symbols.len()
        + output.added_edges.len()
        + output.removed_edges.len();

    if total == 0 {
        return format!(
            "No changes detected since last snapshot ({}).",
            output.previous_snapshot_at
        );
    }

    let mut out = format!(
        "Graph diff for '{}' (snapshot {} -> {}):\n\n",
        output.repo_id, output.previous_snapshot_at, output.current_snapshot_at,
    );

    if !output.added_symbols.is_empty() {
        out.push_str(&format!(
            "+ Added symbols ({}):\n",
            output.added_symbols.len()
        ));
        for entry in &output.added_symbols {
            out.push_str(&format!("    + {}\n", entry.label));
        }
        out.push('\n');
    }

    if !output.removed_symbols.is_empty() {
        out.push_str(&format!(
            "- Removed symbols ({}):\n",
            output.removed_symbols.len()
        ));
        for entry in &output.removed_symbols {
            out.push_str(&format!("    - {}\n", entry.label));
        }
        out.push('\n');
    }

    if !output.added_edges.is_empty() {
        out.push_str(&format!(
            "+ Added relationships ({}):\n",
            output.added_edges.len()
        ));
        for entry in &output.added_edges {
            out.push_str(&format!("    + {}\n", entry.label));
        }
        out.push('\n');
    }

    if !output.removed_edges.is_empty() {
        out.push_str(&format!(
            "- Removed relationships ({}):\n",
            output.removed_edges.len()
        ));
        for entry in &output.removed_edges {
            out.push_str(&format!("    - {}\n", entry.label));
        }
        out.push('\n');
    }

    out.push_str(&format!(
        "Summary: +{} -{} symbols, +{} -{} relationships",
        output.added_symbols.len(),
        output.removed_symbols.len(),
        output.added_edges.len(),
        output.removed_edges.len(),
    ));

    out
}

// ── Cross-repo comparison formatters (premium) ──────────────────────────

pub fn format_intent_slice(slice: &myceliums_core::cross_repo::IntentSlice) -> String {
    let mut out = format!(
        "Intent Slice: \"{}\"\n\
         Repository: {} ({})\n\
         Total symbols: {}\n\
         Languages: {}\n\
         Communities: {}\n\n",
        slice.intent_query,
        slice.repo_name,
        slice.repo_id,
        slice.total_symbols,
        if slice.languages.is_empty() {
            "unknown".to_string()
        } else {
            slice.languages.join(", ")
        },
        slice.community_labels.len(),
    );

    out.push_str("Seed symbols (direct matches):\n");
    for s in &slice.seed_symbols {
        out.push_str(&format!(
            "  {} {} [{}] {}:{}-{}\n",
            s.kind, s.name, s.qualified_name, s.file_path, s.start_line, s.end_line
        ));
        if !s.signature.is_empty() {
            out.push_str(&format!("    sig: {}\n", truncate(&s.signature, 80)));
        }
    }

    if !slice.expanded_symbols.is_empty() {
        out.push_str(&format!(
            "\nExpanded symbols (via call graph, {} total):\n",
            slice.expanded_symbols.len()
        ));
        for s in &slice.expanded_symbols {
            out.push_str(&format!(
                "  {} {} [{}] {}:{}-{}\n",
                s.kind, s.name, s.qualified_name, s.file_path, s.start_line, s.end_line
            ));
        }
    }

    if !slice.internal_relationships.is_empty() {
        out.push_str(&format!(
            "\nInternal relationships ({}):\n",
            slice.internal_relationships.len()
        ));
        for r in slice.internal_relationships.iter().take(30) {
            out.push_str(&format!(
                "  {} --[{}]--> {}\n",
                r.source_name, r.kind, r.target_name
            ));
        }
        if slice.internal_relationships.len() > 30 {
            out.push_str(&format!(
                "  ... and {} more\n",
                slice.internal_relationships.len() - 30
            ));
        }
    }

    out
}

pub fn format_differentiation_report(
    report: &myceliums_core::cross_repo::DifferentiationReport,
) -> String {
    let mut out = format!(
        "Differentiation Report: \"{}\"\n\
         Source: {} ({} symbols)\n\
         Target: {} ({} symbols)\n\n",
        report.intent_query,
        report.source_slice.repo_name,
        report.source_slice.total_symbols,
        report.target_slice.repo_name,
        report.target_slice.total_symbols,
    );

    // Alignments
    out.push_str(&format!(
        "Symbol Alignments ({} matched):\n",
        report.alignments.len()
    ));
    for a in &report.alignments {
        out.push_str(&format!(
            "  {} <-> {} [{}, score: {:.2}]\n    src: {} ({}:{})\n    tgt: {} ({}:{})\n",
            a.source_symbol.name,
            a.target_symbol.name,
            a.match_type,
            a.similarity_score,
            a.source_symbol.qualified_name,
            a.source_symbol.file_path,
            a.source_symbol.start_line,
            a.target_symbol.qualified_name,
            a.target_symbol.file_path,
            a.target_symbol.start_line,
        ));
    }

    // Unmatched
    if !report.unmatched.is_empty() {
        out.push_str(&format!(
            "\nUnmatched Symbols ({}):\n",
            report.unmatched.len()
        ));
        for u in &report.unmatched {
            let label = if u.side == "source" {
                &report.source_slice.repo_name
            } else {
                &report.target_slice.repo_name
            };
            out.push_str(&format!(
                "  [{}] {} {} ({}:{})\n",
                label, u.symbol.kind, u.symbol.name, u.symbol.file_path, u.symbol.start_line
            ));
        }
    }

    // Structural differences
    if !report.structural_differences.is_empty() {
        out.push_str("\nStructural Differences:\n");
        for d in &report.structural_differences {
            let icon = match d.significance.as_str() {
                "critical" => "!!",
                "notable" => "! ",
                _ => "  ",
            };
            out.push_str(&format!(
                "  [{}] {}: {} vs {}\n",
                icon, d.dimension, d.source_value, d.target_value
            ));
        }
    }

    out
}

pub fn format_adaptation_plan(plan: &myceliums_core::cross_repo::AdaptationPlan) -> String {
    let mut out = format!(
        "Adaptation Plan: \"{}\"\n\
         Direction: {} -> {}\n\
         Effort: {}\n\n",
        plan.intent_query, plan.source_repo, plan.target_repo, plan.effort_estimate,
    );

    // Steps
    out.push_str(&format!("Steps ({}):\n", plan.steps.len()));
    for step in &plan.steps {
        let prereqs = if step.prerequisite_steps.is_empty() {
            String::new()
        } else {
            format!(
                " (after: {})",
                step.prerequisite_steps
                    .iter()
                    .map(|s| format!("#{}", s))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        out.push_str(&format!(
            "\n  Step #{} [{}]{}\n  {}\n",
            step.order, step.category, prereqs, step.action
        ));
        if !step.file_paths.is_empty() {
            out.push_str(&format!("  Files: {}\n", step.file_paths.join(", ")));
        }
        if !step.symbols_affected.is_empty() {
            let shown: Vec<&str> = step
                .symbols_affected
                .iter()
                .take(5)
                .map(|s| s.as_str())
                .collect();
            let suffix = if step.symbols_affected.len() > 5 {
                format!(" (+{} more)", step.symbols_affected.len() - 5)
            } else {
                String::new()
            };
            out.push_str(&format!("  Symbols: {}{}\n", shown.join(", "), suffix));
        }
    }

    // Summary
    out.push_str(&format!(
        "\nSummary:\n  Create: {} symbols\n  Adapt: {} symbols\n  Remove: {} symbols\n",
        plan.symbols_to_create.len(),
        plan.symbols_to_adapt.len(),
        plan.symbols_to_remove.len(),
    ));

    // Risks
    if !plan.risks.is_empty() {
        out.push_str("\nRisks:\n");
        for risk in &plan.risks {
            out.push_str(&format!("  - {}\n", risk));
        }
    }

    out
}

pub fn format_centrality(output: &CentralityOutput) -> String {
    if output.nodes.is_empty() {
        return "No symbols found for centrality analysis.".to_string();
    }

    let mut out = format!(
        "Centrality report ({} of {} nodes, sorted by {}):\n\n",
        output.nodes.len(),
        output.total_nodes,
        output.metric,
    );

    let name_w = output
        .nodes
        .iter()
        .map(|n| n.name.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 30);

    out.push_str(&format!(
        " {:<3} {:<name_w$}  {:>8}  {:>8}  {:>8}  {:>8}  File\n",
        "#", "Name", "Degree", "Between", "Close", "Eigen",
    ));
    out.push_str(&format!(
        " {:-<3} {:-<name_w$}  {:->8}  {:->8}  {:->8}  {:->8}  ----\n",
        "", "", "", "", "", "",
    ));

    for (i, n) in output.nodes.iter().enumerate() {
        let short_file = shorten_path(&n.file_path, 40);
        out.push_str(&format!(
            " {:<3} {:<name_w$}  {:>8.4}  {:>8.4}  {:>8.4}  {:>8.4}  {}\n",
            i + 1,
            truncate(&n.name, name_w),
            n.degree,
            n.betweenness,
            n.closeness,
            n.eigenvector,
            short_file,
        ));
    }

    out
}

pub fn format_community_metrics(output: &CommunityMetricsOutput) -> String {
    let mut out = format!(
        "Community metrics ({} communities, modularity: {:.4}):\n\n",
        output.community_count, output.modularity,
    );

    if output.modularity > 0.4 {
        out.push_str("Modularity > 0.4: good community structure detected.\n\n");
    } else if output.modularity > 0.2 {
        out.push_str("Modularity 0.2-0.4: moderate community structure.\n\n");
    } else {
        out.push_str(
            "Modularity < 0.2: weak community structure — code may be tightly coupled.\n\n",
        );
    }

    // Cohesion table
    if !output.cohesion.is_empty() {
        out.push_str("Cohesion (internal edge density per community):\n\n");
        let name_w = output
            .cohesion
            .iter()
            .map(|c| c.community.len())
            .max()
            .unwrap_or(4)
            .clamp(4, 30);

        out.push_str(&format!(" {:<name_w$}  {:>8}\n", "Community", "Density"));
        out.push_str(&format!(" {:-<name_w$}  {:->8}\n", "", ""));

        for c in &output.cohesion {
            out.push_str(&format!(
                " {:<name_w$}  {:>8.4}\n",
                truncate(&c.community, name_w),
                c.density,
            ));
        }
        out.push('\n');
    }

    // Coupling table
    if !output.coupling.is_empty() {
        out.push_str("Inter-community coupling (cross-community edge counts):\n\n");
        let a_w = output
            .coupling
            .iter()
            .map(|c| c.community_a.len())
            .max()
            .unwrap_or(4)
            .clamp(4, 20);
        let b_w = output
            .coupling
            .iter()
            .map(|c| c.community_b.len())
            .max()
            .unwrap_or(4)
            .clamp(4, 20);

        out.push_str(&format!(
            " {:<a_w$}  {:<b_w$}  {:>6}\n",
            "Community A", "Community B", "Edges",
        ));
        out.push_str(&format!(" {:-<a_w$}  {:-<b_w$}  {:->6}\n", "", "", ""));

        for c in output.coupling.iter().take(20) {
            out.push_str(&format!(
                " {:<a_w$}  {:<b_w$}  {:>6}\n",
                truncate(&c.community_a, a_w),
                truncate(&c.community_b, b_w),
                c.edge_count,
            ));
        }
    }

    out
}

pub fn format_cycles(output: &CyclesOutput) -> String {
    if output.cycles.is_empty() {
        return "No circular dependencies detected. The codebase has a clean dependency structure."
            .to_string();
    }

    let mut out = format!(
        "Found {} circular dependency cycle(s):\n\n",
        output.total_count,
    );

    for (i, cycle) in output.cycles.iter().enumerate() {
        out.push_str(&format!(
            "Cycle {} ({} symbols, {} files):\n",
            i + 1,
            cycle.size,
            cycle.files.len(),
        ));
        out.push_str(&format!("  Symbols: {}\n", cycle.members.join(" -> ")));
        out.push_str(&format!("  Files:   {}\n\n", cycle.files.join(", ")));
    }

    out
}

pub fn format_dependencies(output: &DependenciesOutput) -> String {
    let mut out = format!("Dependencies for {}:\n\n", output.file_path);

    out.push_str(&format!(
        "Direct dependencies ({}):\n",
        output.direct_deps.len()
    ));
    for dep in &output.direct_deps {
        out.push_str(&format!("  {}\n", dep));
    }

    out.push_str(&format!(
        "\nTransitive dependencies ({}):\n",
        output.transitive_deps.len()
    ));
    for dep in &output.transitive_deps {
        out.push_str(&format!("  {}\n", dep));
    }

    out.push_str(&format!(
        "\nReverse dependents ({}):\n",
        output.dependents.len()
    ));
    for dep in &output.dependents {
        out.push_str(&format!("  {}\n", dep));
    }

    out
}

pub fn format_module_coupling(output: &ModuleCouplingOutput) -> String {
    if output.modules.is_empty() {
        return "No module coupling data. The graph may lack IMPORTS relationships.".to_string();
    }

    let mut out = format!(
        "Module coupling ({} of {} modules, sorted by instability):\n\n",
        output.modules.len(),
        output.total_count,
    );

    let path_w = output
        .modules
        .iter()
        .map(|m| m.module_path.len())
        .max()
        .unwrap_or(4)
        .clamp(4, 45);

    out.push_str(&format!(
        " {:<path_w$}  {:>4}  {:>4}  {:>10}\n",
        "Module", "Ca", "Ce", "Instability",
    ));
    out.push_str(&format!(
        " {:-<path_w$}  {:->4}  {:->4}  {:->10}\n",
        "", "", "", "",
    ));

    for m in &output.modules {
        let short_path = truncate(&m.module_path, path_w);
        out.push_str(&format!(
            " {:<path_w$}  {:>4}  {:>4}  {:>10.4}\n",
            short_path, m.afferent, m.efferent, m.instability,
        ));
    }

    out.push_str("\nCa = afferent (incoming deps), Ce = efferent (outgoing deps)\n");
    out.push_str("Instability = Ce/(Ca+Ce): 0 = stable, 1 = unstable\n");

    out
}

pub fn format_hotspots(hotspots: &[HotspotItem]) -> String {
    if hotspots.is_empty() {
        return "No hotspots found.".to_string();
    }

    let mut out = format!("Top {} quality hotspots:\n\n", hotspots.len());

    let name_w = hotspots
        .iter()
        .map(|h| h.name.len())
        .max()
        .unwrap_or(10)
        .max(4);

    out.push_str(&format!(
        "  {:>3}  {:<name_w$}  {:>8}  {:>12}  {:>8}  {:>11}  {}\n",
        "#",
        "Name",
        "Score",
        "Betweenness",
        "Churn",
        "Instability",
        "File",
        name_w = name_w,
    ));
    out.push_str(&format!(
        "  {:─>3}  {:─<name_w$}  {:─>8}  {:─>12}  {:─>8}  {:─>11}  {:─>20}\n",
        "",
        "",
        "",
        "",
        "",
        "",
        "",
        name_w = name_w,
    ));

    for (i, h) in hotspots.iter().enumerate() {
        out.push_str(&format!(
            "  {:>3}  {:<name_w$}  {:>8.2}  {:>12.4}  {:>8}  {:>11.3}  {}\n",
            i + 1,
            h.name,
            h.score,
            h.betweenness,
            h.commit_count,
            h.instability,
            h.file_path,
            name_w = name_w,
        ));
    }

    out
}

pub fn format_lint_results(report: &LintReport) -> String {
    if report.findings.is_empty() {
        return format!(
            "No issues found. Checked {} rules.",
            report.rules_checked.len()
        );
    }

    let mut out = format!(
        "Architecture lint: {} findings ({} errors, {} warnings, {} info)\n\nRules checked: {}\n\n",
        report.findings.len(),
        report.error_count,
        report.warning_count,
        report.info_count,
        report.rules_checked.join(", "),
    );

    for f in &report.findings {
        out.push_str(&format!(
            "  [{:>7}] {}: {}\n",
            f.severity, f.rule_id, f.message
        ));
        if !f.affected_entities.is_empty() {
            out.push_str(&format!(
                "           Affected: {}\n",
                f.affected_entities.join(", ")
            ));
        }
    }

    out
}

pub fn format_architecture_view(diagram: &ArchDiagram) -> String {
    let mut out = format!(
        "Architecture: {} services, {} connections\n\n",
        diagram.services.len(),
        diagram.connections.len()
    );

    out.push_str("Services:\n");
    for svc in &diagram.services {
        out.push_str(&format!("  {} ({} symbols)\n", svc.label, svc.member_count));
        if !svc.top_symbols.is_empty() {
            out.push_str(&format!("    Top: {}\n", svc.top_symbols.join(", ")));
        }
    }

    if !diagram.connections.is_empty() {
        out.push_str("\nConnections:\n");
        for conn in &diagram.connections {
            out.push_str(&format!(
                "  {} → {} ({} edges, {})\n",
                conn.source,
                conn.target,
                conn.edge_count,
                conn.relationship_types.join("+")
            ));
        }
    }

    out.push_str("\nMermaid diagram:\n```mermaid\n");
    out.push_str(&diagram.mermaid);
    out.push_str("```\n");

    out
}

pub fn format_drift_report(report: &DriftReport) -> String {
    let mut out = format!("{}\n\n", report.summary);

    if !report.community_changes.is_empty() {
        out.push_str("Community changes:\n");
        for change in &report.community_changes {
            match change.change_type.as_str() {
                "added" => out.push_str(&format!(
                    "  + {} (new, {} members)\n",
                    change.label,
                    change.new_count.unwrap_or(0)
                )),
                "removed" => out.push_str(&format!(
                    "  - {} (removed, was {} members)\n",
                    change.label,
                    change.old_count.unwrap_or(0)
                )),
                "grown" => out.push_str(&format!(
                    "  ↑ {} ({} → {} members)\n",
                    change.label,
                    change.old_count.unwrap_or(0),
                    change.new_count.unwrap_or(0)
                )),
                "shrunk" => out.push_str(&format!(
                    "  ↓ {} ({} → {} members)\n",
                    change.label,
                    change.old_count.unwrap_or(0),
                    change.new_count.unwrap_or(0)
                )),
                _ => out.push_str(&format!("  = {} (stable)\n", change.label)),
            }
        }
    }

    out
}

pub fn format_ownership(report: &OwnershipReport) -> String {
    if report.owned_files.is_empty() && report.unowned_files.is_empty() {
        return "No files found.".to_string();
    }

    let mut out = format!(
        "Ownership report ({} rules, {} owned, {} unowned):\n\n",
        report.total_rules,
        report.owned_files.len(),
        report.unowned_files.len()
    );

    if !report.owned_files.is_empty() {
        out.push_str("Owned files:\n");
        for f in &report.owned_files {
            out.push_str(&format!("  {} → {}\n", f.file_path, f.owners.join(", ")));
        }
    }

    if !report.unowned_files.is_empty() {
        out.push_str("\nUnowned files:\n");
        for f in &report.unowned_files {
            out.push_str(&format!("  {}\n", f));
        }
    }

    out
}

pub fn format_decisions(decisions: &[ArchDecisionRecord]) -> String {
    if decisions.is_empty() {
        return "No ADRs found.".to_string();
    }

    let mut out = format!("{} Architecture Decision Records:\n\n", decisions.len());

    for adr in decisions {
        out.push_str(&format!(
            "  [{}] {} ({})\n",
            adr.id.chars().take(8).collect::<String>(),
            adr.title,
            adr.status,
        ));
        out.push_str(&format!("    Context: {}\n", truncate(&adr.context, 80)));
        out.push_str(&format!("    Decision: {}\n", truncate(&adr.decision, 80)));
        if !adr.linked_symbols.is_empty() {
            out.push_str(&format!("    Linked: {}\n", adr.linked_symbols.join(", ")));
        }
        out.push('\n');
    }

    out
}

pub fn format_contracts(report: &ContractsReport) -> String {
    if report.contracts.is_empty() {
        return "No API contracts detected.".to_string();
    }

    let mut out = format!(
        "{} contracts, {} endpoints ({} linked, {} unlinked):\n\n",
        report.contracts.len(),
        report.total_endpoints,
        report.linked_count,
        report.unlinked_endpoints.len()
    );

    for contract in &report.contracts {
        out.push_str(&format!(
            "  {} ({}, {} endpoints)\n",
            contract.spec_file,
            contract.spec_type,
            contract.endpoints.len()
        ));
        for link in &contract.handler_links {
            out.push_str(&format!(
                "    {} → {} (confidence: {:.0}%)\n",
                link.endpoint_path,
                link.handler_name,
                link.confidence * 100.0
            ));
        }
    }

    if !report.unlinked_endpoints.is_empty() {
        out.push_str("\nUnlinked endpoints:\n");
        for ep in &report.unlinked_endpoints {
            out.push_str(&format!("  {}\n", ep));
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CentralityNodeOutput, CohesionEntry, CycleItemOutput, ModuleCouplingEntry};

    #[test]
    fn test_format_centrality_output() {
        let output = CentralityOutput {
            nodes: vec![CentralityNodeOutput {
                name: "process_request".to_string(),
                qualified_name: "api::process_request".to_string(),
                kind: "Function".to_string(),
                file_path: "src/api.rs".to_string(),
                degree: 0.5,
                betweenness: 0.8,
                closeness: 0.6,
                eigenvector: 0.3,
            }],
            total_nodes: 100,
            metric: "betweenness".to_string(),
        };
        let text = format_centrality(&output);
        assert!(text.contains("1 of 100 nodes"));
        assert!(text.contains("betweenness"));
        assert!(text.contains("process_request"));
        assert!(text.contains("0.8000")); // betweenness value
    }

    #[test]
    fn test_format_centrality_empty() {
        let output = CentralityOutput {
            nodes: vec![],
            total_nodes: 0,
            metric: "degree".to_string(),
        };
        let text = format_centrality(&output);
        assert!(text.contains("No symbols found"));
    }

    #[test]
    fn test_format_community_metrics_high_modularity() {
        let output = CommunityMetricsOutput {
            modularity: 0.55,
            community_count: 5,
            cohesion: vec![CohesionEntry {
                community: "auth".to_string(),
                density: 0.8,
            }],
            coupling: vec![],
        };
        let text = format_community_metrics(&output);
        assert!(text.contains("good community structure"));
        assert!(text.contains("0.55"));
    }

    #[test]
    fn test_format_community_metrics_low_modularity() {
        let output = CommunityMetricsOutput {
            modularity: 0.1,
            community_count: 2,
            cohesion: vec![],
            coupling: vec![],
        };
        let text = format_community_metrics(&output);
        assert!(text.contains("weak community structure"));
    }

    #[test]
    fn test_format_cycles_none_found() {
        let output = CyclesOutput {
            cycles: vec![],
            total_count: 0,
        };
        let text = format_cycles(&output);
        assert!(text.contains("No circular dependencies"));
    }

    #[test]
    fn test_format_cycles_with_results() {
        let output = CyclesOutput {
            cycles: vec![CycleItemOutput {
                members: vec!["foo".to_string(), "bar".to_string(), "baz".to_string()],
                size: 3,
                files: vec!["src/a.rs".to_string(), "src/b.rs".to_string()],
            }],
            total_count: 1,
        };
        let text = format_cycles(&output);
        assert!(text.contains("1 circular dependency"));
        assert!(text.contains("foo"));
        assert!(text.contains("bar"));
        assert!(text.contains("baz"));
        assert!(text.contains("src/a.rs"));
    }

    #[test]
    fn test_format_dependencies_output() {
        let output = DependenciesOutput {
            file_path: "src/main.rs".to_string(),
            direct_deps: vec!["src/utils.rs".to_string()],
            transitive_deps: vec!["src/utils.rs".to_string(), "src/core.rs".to_string()],
            dependents: vec!["src/test.rs".to_string()],
        };
        let text = format_dependencies(&output);
        assert!(text.contains("src/main.rs"));
        assert!(text.contains("Direct dependencies (1)"));
        assert!(text.contains("Transitive dependencies (2)"));
        assert!(text.contains("Reverse dependents (1)"));
        assert!(text.contains("src/utils.rs"));
        assert!(text.contains("src/test.rs"));
    }

    #[test]
    fn test_format_module_coupling_output() {
        let output = ModuleCouplingOutput {
            modules: vec![ModuleCouplingEntry {
                module_path: "src/api".to_string(),
                afferent: 5,
                efferent: 3,
                instability: 0.375,
            }],
            total_count: 10,
        };
        let text = format_module_coupling(&output);
        assert!(text.contains("1 of 10 modules"));
        assert!(text.contains("src/api"));
        assert!(text.contains("Ca"));
        assert!(text.contains("Ce"));
        assert!(text.contains("Instability"));
    }
}
