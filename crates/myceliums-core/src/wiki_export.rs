//! Generates an Obsidian-compatible wiki from the knowledge graph.
//!
//! Produces one markdown file per community with wikilinks for cross-references,
//! plus an index page listing all communities.

use anyhow::Result;
use myceliums_storage::{CodeSymbol, Community, Relationship};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::Path;

use crate::community::compute_uid_to_community_label;

/// Result summary returned after exporting.
pub struct WikiExportResult {
    pub community_count: usize,
    pub symbol_count: usize,
    pub relationship_count: usize,
}

/// Configuration for wiki export.
pub struct WikiExportConfig {
    /// Whether to generate an `.obsidian/` config stub.
    pub obsidian_vault: bool,
    /// Maximum lines of symbol content to include (0 = signature only).
    pub content_max_lines: usize,
}

impl Default for WikiExportConfig {
    fn default() -> Self {
        Self {
            obsidian_vault: false,
            content_max_lines: 10,
        }
    }
}

/// Sanitize a community label for use as a filename (no extension).
fn sanitize_filename(label: &str) -> String {
    label
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | ' ' => '-',
            _ => c,
        })
        .collect::<String>()
}

/// Deduplicate filenames by appending `-2`, `-3`, etc.
fn deduplicate_names(names: &[String]) -> Vec<String> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut result = Vec::with_capacity(names.len());
    for name in names {
        let count = seen.entry(name.clone()).or_insert(0);
        *count += 1;
        if *count == 1 {
            result.push(name.clone());
        } else {
            result.push(format!("{}-{}", name, count));
        }
    }
    result
}

/// Truncate content to at most `max_lines` lines, appending "[truncated]" if cut.
fn truncate_content(content: &str, max_lines: usize) -> String {
    if max_lines == 0 {
        return String::new();
    }
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() <= max_lines {
        content.to_string()
    } else {
        let mut out: String = lines[..max_lines].join("\n");
        out.push_str("\n// ... [truncated]");
        out
    }
}

/// Export the knowledge graph as an Obsidian wiki.
///
/// Creates one `.md` file per community plus an `index.md` in `output_dir`.
pub fn export_wiki(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    communities: &[Community],
    repo_name: &str,
    output_dir: &Path,
    config: &WikiExportConfig,
) -> Result<WikiExportResult> {
    fs::create_dir_all(output_dir)?;

    // If no communities were detected, create a single "All-Symbols" community page
    if communities.is_empty() {
        write_single_community_wiki(symbols, relationships, repo_name, output_dir, config)?;
        return Ok(WikiExportResult {
            community_count: 1,
            symbol_count: symbols.len(),
            relationship_count: relationships.len(),
        });
    }

    // Re-derive community membership from CALLS relationships
    let uid_to_community = compute_uid_to_community_label(symbols, relationships)?;

    // Build community label -> [symbols] mapping
    let uid_to_symbol: HashMap<&str, &CodeSymbol> =
        symbols.iter().map(|s| (s.uid.as_str(), s)).collect();

    // Use stored communities as the authoritative list, match symbols via label
    let mut community_members: BTreeMap<String, Vec<&CodeSymbol>> = BTreeMap::new();
    for community in communities {
        community_members
            .entry(community.label.clone())
            .or_default();
    }

    // Assign symbols to communities based on Leiden membership
    for (uid, label) in &uid_to_community {
        if let Some(sym) = uid_to_symbol.get(uid.as_str()) {
            community_members
                .entry(label.clone())
                .or_default()
                .push(sym);
        }
    }

    // Collect orphan symbols (not in any CALLS partition)
    let assigned_uids: HashSet<&str> = uid_to_community.keys().map(|s| s.as_str()).collect();
    let orphans: Vec<&CodeSymbol> = symbols
        .iter()
        .filter(|s| !assigned_uids.contains(s.uid.as_str()))
        .collect();
    if !orphans.is_empty() {
        community_members
            .entry("Uncategorized".to_string())
            .or_default()
            .extend(orphans);
    }

    // Build community label -> stored Community metadata lookup
    let community_meta: HashMap<&str, &Community> =
        communities.iter().map(|c| (c.label.as_str(), c)).collect();

    // Prepare sanitized filenames (dedup after sanitize)
    let labels: Vec<String> = community_members.keys().cloned().collect();
    let sanitized: Vec<String> = labels.iter().map(|l| sanitize_filename(l)).collect();
    let filenames = deduplicate_names(&sanitized);
    let label_to_filename: HashMap<&str, &str> = labels
        .iter()
        .zip(filenames.iter())
        .map(|(l, f)| (l.as_str(), f.as_str()))
        .collect();

    // Build a reverse lookup: symbol UID -> community label (for cross-community links)
    let uid_to_label: HashMap<&str, &str> = {
        let mut m = HashMap::new();
        for (label, members) in &community_members {
            for sym in members {
                m.insert(sym.uid.as_str(), label.as_str());
            }
        }
        m
    };

    // Index relationships by source and target UID for quick lookup
    let mut rels_by_source: HashMap<&str, Vec<&Relationship>> = HashMap::new();
    for rel in relationships {
        rels_by_source
            .entry(rel.source_uid.as_str())
            .or_default()
            .push(rel);
    }

    let mut total_symbols = 0usize;

    // Write one file per community
    for (label, members) in &community_members {
        let filename = label_to_filename
            .get(label.as_str())
            .copied()
            .unwrap_or(label.as_str());
        let file_path = output_dir.join(format!("{}.md", filename));

        let mut md = String::new();

        // YAML frontmatter
        let meta = community_meta.get(label.as_str());
        md.push_str("---\n");
        md.push_str(&format!("community: \"{}\"\n", label.replace('"', "\\\"")));
        md.push_str(&format!("member_count: {}\n", members.len()));
        if let Some(c) = meta {
            if !c.top_symbols.is_empty() {
                md.push_str(&format!("top_symbols: \"{}\"\n", c.top_symbols));
            }
        }
        md.push_str("---\n\n");

        // Heading
        md.push_str(&format!("# {}\n\n", label));

        // Summary
        if let Some(c) = meta {
            if !c.summary.is_empty() {
                md.push_str(&format!("{}\n\n", c.summary));
            }
        }

        // Symbol list, grouped by kind
        let mut by_kind: BTreeMap<String, Vec<&&CodeSymbol>> = BTreeMap::new();
        for sym in members {
            by_kind.entry(sym.kind.to_string()).or_default().push(sym);
        }

        md.push_str("## Symbols\n\n");
        for (kind, syms) in &by_kind {
            md.push_str(&format!("### {}\n\n", kind));
            for sym in syms {
                total_symbols += 1;
                md.push_str(&format!("#### `{}`\n\n", sym.name));
                if !sym.qualified_name.is_empty() && sym.qualified_name != sym.name {
                    md.push_str(&format!("- **Qualified name**: `{}`\n", sym.qualified_name));
                }
                if !sym.signature.is_empty() {
                    md.push_str(&format!("- **Signature**: `{}`\n", sym.signature));
                }
                md.push_str(&format!(
                    "- **File**: `{}` (L{}–L{})\n",
                    sym.file_path, sym.start_line, sym.end_line
                ));

                // Content excerpt
                if config.content_max_lines > 0 && !sym.content.is_empty() {
                    let excerpt = truncate_content(&sym.content, config.content_max_lines);
                    md.push_str(&format!("\n```\n{}\n```\n", excerpt));
                }

                md.push('\n');
            }
        }

        // Relationships section
        let member_uids: HashSet<&str> = members.iter().map(|s| s.uid.as_str()).collect();
        let mut internal_rels: Vec<String> = Vec::new();
        let mut cross_community_links: HashSet<String> = HashSet::new();

        for sym in members {
            if let Some(rels) = rels_by_source.get(sym.uid.as_str()) {
                for rel in rels {
                    let target_name = uid_to_symbol
                        .get(rel.target_uid.as_str())
                        .map(|s| s.name.as_str())
                        .unwrap_or("unknown");

                    if member_uids.contains(rel.target_uid.as_str()) {
                        // Internal relationship
                        internal_rels
                            .push(format!("- `{}` {} `{}`", sym.name, rel.kind, target_name));
                    } else {
                        // Cross-community
                        let target_label = uid_to_label
                            .get(rel.target_uid.as_str())
                            .copied()
                            .unwrap_or("Uncategorized");
                        let target_file = label_to_filename
                            .get(target_label)
                            .copied()
                            .unwrap_or(target_label);
                        internal_rels.push(format!(
                            "- `{}` {} `{}` ([[{}]])",
                            sym.name, rel.kind, target_name, target_file
                        ));
                        cross_community_links.insert(target_file.to_string());
                    }
                }
            }
        }

        if !internal_rels.is_empty() {
            md.push_str("## Relationships\n\n");
            // Deduplicate
            internal_rels.sort();
            internal_rels.dedup();
            for line in &internal_rels {
                md.push_str(line);
                md.push('\n');
            }
            md.push('\n');
        }

        // Related communities
        if !cross_community_links.is_empty() {
            md.push_str("## Related Communities\n\n");
            let mut sorted: Vec<&String> = cross_community_links.iter().collect();
            sorted.sort();
            for link in sorted {
                md.push_str(&format!("- [[{}]]\n", link));
            }
            md.push('\n');
        }

        fs::write(&file_path, md)?;
    }

    // Write index page
    write_index(
        output_dir,
        &community_members,
        &community_meta,
        &label_to_filename,
        repo_name,
    )?;

    // Optional: write .obsidian config stub
    if config.obsidian_vault {
        write_obsidian_config(output_dir)?;
    }

    Ok(WikiExportResult {
        community_count: community_members.len(),
        symbol_count: total_symbols,
        relationship_count: relationships.len(),
    })
}

fn write_index(
    output_dir: &Path,
    community_members: &BTreeMap<String, Vec<&CodeSymbol>>,
    community_meta: &HashMap<&str, &Community>,
    label_to_filename: &HashMap<&str, &str>,
    repo_name: &str,
) -> Result<()> {
    let mut md = String::new();
    md.push_str(&format!("# {} — Knowledge Graph Wiki\n\n", repo_name));
    md.push_str(&format!(
        "This wiki contains **{}** communities.\n\n",
        community_members.len()
    ));
    md.push_str("## Communities\n\n");
    md.push_str("| Community | Members | Summary |\n");
    md.push_str("|-----------|---------|----------|\n");

    for (label, members) in community_members {
        let filename = label_to_filename
            .get(label.as_str())
            .copied()
            .unwrap_or(label.as_str());
        let summary = community_meta
            .get(label.as_str())
            .map(|c| c.summary.as_str())
            .unwrap_or("");
        // Truncate summary for table
        let short_summary = if summary.len() > 80 {
            format!("{}...", &summary[..77])
        } else {
            summary.to_string()
        };
        md.push_str(&format!(
            "| [[{}]] | {} | {} |\n",
            filename,
            members.len(),
            short_summary,
        ));
    }

    md.push('\n');
    fs::write(output_dir.join("index.md"), md)?;
    Ok(())
}

fn write_obsidian_config(output_dir: &Path) -> Result<()> {
    let obsidian_dir = output_dir.join(".obsidian");
    fs::create_dir_all(&obsidian_dir)?;

    // Minimal app.json
    let app_json = r#"{
  "alwaysUpdateLinks": true,
  "newFileLocation": "current",
  "attachmentFolderPath": "./"
}"#;
    fs::write(obsidian_dir.join("app.json"), app_json)?;

    // Minimal workspace.json
    let workspace_json = r#"{
  "main": {
    "id": "main",
    "type": "split",
    "children": []
  }
}"#;
    fs::write(obsidian_dir.join("workspace.json"), workspace_json)?;

    Ok(())
}

/// Fallback when no communities are detected: write all symbols in one page.
fn write_single_community_wiki(
    symbols: &[CodeSymbol],
    relationships: &[Relationship],
    repo_name: &str,
    output_dir: &Path,
    config: &WikiExportConfig,
) -> Result<()> {
    let label = "All-Symbols";
    let mut md = String::new();
    md.push_str("---\n");
    md.push_str(&format!("community: \"{}\"\n", label));
    md.push_str(&format!("member_count: {}\n", symbols.len()));
    md.push_str("---\n\n");
    md.push_str(&format!("# {}\n\n", label));
    md.push_str("No communities were detected. All symbols are listed below.\n\n");
    md.push_str("## Symbols\n\n");

    let mut by_kind: BTreeMap<String, Vec<&CodeSymbol>> = BTreeMap::new();
    for sym in symbols {
        by_kind.entry(sym.kind.to_string()).or_default().push(sym);
    }
    for (kind, syms) in &by_kind {
        md.push_str(&format!("### {}\n\n", kind));
        for sym in syms {
            md.push_str(&format!("#### `{}`\n\n", sym.name));
            if !sym.qualified_name.is_empty() && sym.qualified_name != sym.name {
                md.push_str(&format!("- **Qualified name**: `{}`\n", sym.qualified_name));
            }
            if !sym.signature.is_empty() {
                md.push_str(&format!("- **Signature**: `{}`\n", sym.signature));
            }
            md.push_str(&format!(
                "- **File**: `{}` (L{}–L{})\n",
                sym.file_path, sym.start_line, sym.end_line
            ));
            if config.content_max_lines > 0 && !sym.content.is_empty() {
                let excerpt = truncate_content(&sym.content, config.content_max_lines);
                md.push_str(&format!("\n```\n{}\n```\n", excerpt));
            }
            md.push('\n');
        }
    }

    fs::write(output_dir.join(format!("{}.md", label)), md)?;

    // Write a minimal index
    let mut idx = String::new();
    idx.push_str(&format!("# {} — Knowledge Graph Wiki\n\n", repo_name));
    idx.push_str("This wiki contains **1** community.\n\n");
    idx.push_str("## Communities\n\n");
    idx.push_str("| Community | Members | Summary |\n");
    idx.push_str("|-----------|---------|----------|\n");
    idx.push_str(&format!(
        "| [[{}]] | {} | All symbols (no community detection) |\n",
        label,
        symbols.len()
    ));
    fs::write(output_dir.join("index.md"), idx)?;

    if config.obsidian_vault {
        write_obsidian_config(output_dir)?;
    }

    let _ = relationships; // acknowledged but no community-level grouping
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use myceliums_storage::{RelationshipKind, SymbolKind};
    use tempfile::TempDir;

    fn make_symbol(uid: &str, name: &str, kind: SymbolKind, file_path: &str) -> CodeSymbol {
        CodeSymbol {
            uid: uid.to_string(),
            name: name.to_string(),
            qualified_name: format!("mod::{}", name),
            kind,
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: 10,
            signature: format!("fn {}()", name),
            content: format!("fn {}() {{\n  // body\n}}", name),
            repo_id: "test".to_string(),
            metadata: None,
        }
    }

    fn make_rel(source: &str, target: &str, kind: RelationshipKind) -> Relationship {
        Relationship {
            uid: format!("{}->{}", source, target),
            source_uid: source.to_string(),
            target_uid: target.to_string(),
            kind,
            repo_id: "test".to_string(),
            metadata: String::new(),
        }
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("hello world"), "hello-world");
        assert_eq!(sanitize_filename("a/b:c"), "a-b-c");
        assert_eq!(sanitize_filename("simple"), "simple");
    }

    #[test]
    fn test_deduplicate_names() {
        let names = vec!["a".into(), "b".into(), "a".into(), "a".into()];
        let deduped = deduplicate_names(&names);
        assert_eq!(deduped, vec!["a", "b", "a-2", "a-3"]);
    }

    #[test]
    fn test_truncate_content() {
        let content = "line1\nline2\nline3\nline4\nline5";
        assert_eq!(
            truncate_content(content, 3),
            "line1\nline2\nline3\n// ... [truncated]"
        );
        assert_eq!(truncate_content(content, 10), content);
        assert_eq!(truncate_content(content, 0), "");
    }

    #[test]
    fn test_export_empty_repo() {
        let dir = TempDir::new().unwrap();
        let result = export_wiki(
            &[],
            &[],
            &[],
            "test-repo",
            dir.path(),
            &WikiExportConfig::default(),
        )
        .unwrap();
        assert_eq!(result.community_count, 1); // single "All-Symbols" fallback
        assert!(dir.path().join("index.md").exists());
        assert!(dir.path().join("All-Symbols.md").exists());
    }

    #[test]
    fn test_export_with_communities() {
        let dir = TempDir::new().unwrap();

        let s1 = make_symbol("u1", "login", SymbolKind::Function, "src/auth.rs");
        let s2 = make_symbol("u2", "logout", SymbolKind::Function, "src/auth.rs");
        let s3 = make_symbol("u3", "pay", SymbolKind::Function, "src/payment.rs");

        let r1 = make_rel("u1", "u2", RelationshipKind::Calls);

        let communities = vec![Community {
            uid: "c1".into(),
            label: "login".into(),
            repo_id: "test".into(),
            member_count: 2,
            top_symbols: "login, logout".into(),
            summary: "Auth module".into(),
        }];

        let result = export_wiki(
            &[s1, s2, s3],
            &[r1],
            &communities,
            "test-repo",
            dir.path(),
            &WikiExportConfig::default(),
        )
        .unwrap();

        assert!(result.community_count >= 1);
        assert!(dir.path().join("index.md").exists());
    }

    #[test]
    fn test_export_obsidian_vault() {
        let dir = TempDir::new().unwrap();
        let config = WikiExportConfig {
            obsidian_vault: true,
            ..Default::default()
        };
        export_wiki(&[], &[], &[], "test-repo", dir.path(), &config).unwrap();
        assert!(dir.path().join(".obsidian").exists());
        assert!(dir.path().join(".obsidian/app.json").exists());
    }
}
